// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-directoryservice-directoryservice-20150416 service.
//!
//! This module owns the records the handlers in `src/ops/` share — the directories
//! `CreateDirectory` persists, the snapshots `CreateSnapshot` takes of them, the aliases
//! `CreateAlias` assigns, the computer accounts `CreateComputer` creates in them, the
//! Amazon SNS topics `RegisterEventTopic` associates, the log subscriptions
//! `CreateLogSubscription` creates, the trust relationships
//! `CreateTrust` establishes, the directory shares `ShareDirectory` creates, the schema
//! extensions `StartSchemaExtension` applies, the domain controllers `CreateMicrosoftAD`
//! places, the
//! assessments
//! `StartADAssessment` starts and the tags `AddTagsToResource` merges — plus
//! the pieces of behaviour that cannot live in any single operation file:
//!
//! * [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`], which
//!   materialises the status transitions the clock has already made due (a directory
//!   leaving `Requested`, a snapshot leaving `Creating`, an IP route leaving `Adding`
//!   (and a removed one leaving the store), a
//!   trust leaving `Creating` or `Verifying` (and a torn-down one settling on `Deleted`
//!   and then leaving the store), an
//!   assessment leaving `IN_PROGRESS`, a directory's data access leaving `Enabling`, a
//!   directory's LDAP security leaving `Enabling`, a
//!   registered certificate leaving `Registering`, an organization directory share
//!   leaving `Sharing` (and an unshared one leaving the store), a schema extension
//!   leaving `Initializing` or `CancelInProgress`, a domain controller leaving
//!   `Creating` (and a retired one
//!   leaving the store)).
//!   Every handler calls it before reading or writing, so a reporting operation reports
//!   what the clock already owed and nothing more.
//! * [`Directory::to_description`], the one place a stored directory is rendered as the
//!   modelled `DirectoryDescription`, so every operation that reports a directory
//!   reports the same derived values (`Alias`, `AccessUrl`), and
//!   [`SnapshotRecord::to_snapshot`] / [`EventTopicRegistration::to_event_topic`] /
//!   [`LogSubscriptionRecord::to_log_subscription`] /
//!   [`TrustRecord::to_trust`] / [`SharedDirectoryRecord::to_shared_directory`] /
//!   [`SchemaExtensionRecord::to_info`], the
//!   same for a stored snapshot, a stored topic association, a stored log subscription, a
//!   stored trust, a stored
//!   directory share and a stored schema extension.
//! * [`encode_cursor`] / [`decode_cursor`], the opaque `NextToken` the paginated
//!   operations of this slice (`DescribeDirectories`, `DescribeSnapshots`,
//!   `DescribeTrusts`, `DescribeSharedDirectories`, `ListCertificates`,
//!   `ListLogSubscriptions`, `ListSchemaExtensions`,
//!   `ListTagsForResource`) issue and
//!   resolve.
//! * [`refusal`], the one place a modelled refusal of this service is built: every error
//!   shape the model declares carries a `Message` and a UUID-shaped `RequestId`, and this
//!   is what mints the call's id, reports it in that member and appends it to the message
//!   — so no handler can answer a refusal without the correlation handle a client logs it
//!   by. [`invalid_resource_id`] and [`invalid_next_token`] are the two diagnostics a
//!   whole family of operations shares, worded once here beside [`no_such_directory`].
//! * [`security_log`], what a directory's audited acts DELIVER to the CloudWatch Logs log
//!   group its log subscription names — the forwarding half of
//!   `CreateLogSubscription`, reached by this service's own writes and by the Directory
//!   Service DATA plane's.
//! * [`directory_network`], where a directory's servers sit on the account's network: the
//!   subnets a create places them in, as the service that OWNS subnets reports them, and the
//!   address each server answers DNS at, counted out of its own subnet's CIDR block.
//! * [`subnet_zones`], the half of the two-zone placement rule ([`subnet_pair_defect`] is
//!   the other) that is a fact about that network rather than about the request, decided
//!   over the subnets a create already resolved.
//!
//! The administrator `Password` `CreateDirectory` is called with is deliberately NOT a
//! field here: the model marks it `@sensitive`, no operation reports it back, and state
//! this emulator does not need is state it must not hold. What a directory does hold for
//! the account that password belongs to is a VERIFIER of it ([`account_credentials`]),
//! which is what authenticating a credentialed `EnableSso`/`DisableSso` needs and which no
//! password can be read back out of.

/// The sentences this service words a refusal with, kept together so two operations
/// cannot word one failure two ways. Re-exported below, so every one of them is still
/// reached as `crate::state::<name>`.
mod diagnostics;
/// What a directory holds instead of an account's password, and how a credentialed call is
/// authenticated against it. Re-exported below, so both operations that take a credential
/// pair reach one decision as `crate::state::<name>`.
mod account_credentials;
/// The teardown `DeleteDirectory` starts, and the clock that finishes it. Re-exported
/// below, so its stages are reached as `crate::state::<name>` like every other stage.
mod directory_deletion;
pub mod directory_network;
pub mod directory_servers;
pub mod directory_updates;
pub mod ec2_subnets;
pub mod security_log;
pub mod subnet_zones;

pub use account_credentials::{credential_defect, PasswordVerifier};
pub use diagnostics::*;
pub use directory_deletion::{STAGE_DELETED, STAGE_DELETING};
pub use directory_updates::UpdateActivityRecord;

pub use directory_network::{server_address, SubnetNetwork};
pub use directory_servers::{os_version_of, MICROSOFT_AD_OS_VERSION};
pub use ec2_subnets::{resolve_subnet_networks, vpc_absence};
pub use subnet_zones::single_zone_defect;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Assessment, AssessmentSummary, Attribute, Certificate, CertificateInfo,
    ClientAuthenticationSettingInfo,
    ClientCertAuthSettings, Computer, ConditionalForwarder, DirectoryConnectSettingsDescription,
    DirectoryDescription, DirectoryLimits, DirectoryVpcSettings,
    DirectoryVpcSettingsDescription, DomainController, EventTopic, IpRouteInfo, LDAPSSettingInfo,
    LogSubscription, RegionDescription, SchemaExtensionInfo, SettingEntry, SharedDirectory,
    Snapshot, SnapshotLimits, Tag, Trust, UpdateInfoEntry, UpdateValue,
};

/// How long a status transition a create owes takes to fall due.
///
/// A create (`CreateDirectory`, `CreateMicrosoftAD`) answers while the directory is
/// still being built — the modelled
/// `DirectoryStage` enum's first value is `Requested` — `CreateSnapshot` answers
/// while the snapshot is still being taken — the modelled `SnapshotStatus` enum's first
/// value is `Creating` — and `StartADAssessment` answers while the run it started is
/// still `IN_PROGRESS`, so the move to `Active`/`Completed`/a terminal assessment status
/// is owed by the clock rather
/// than by any later call. The project's emulator contract requires such a transition to
/// fall due 250–750 ms after the call that starts it, so a caller acting immediately
/// after the create still observes the transient state. 500 ms sits in the middle of
/// that band.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The stage a directory is in between `CreateDirectory` and [`TRANSITION_SPAN`].
pub const STAGE_REQUESTED: &str = "Requested";

/// The stage a directory is in while its servers are being built: the `Creating` value of
/// the modelled `DirectoryStage` enum
/// (`smithy_query(query="shape",shape="DirectoryStage")`).
///
/// A create does NOT hand back a directory that is in service: the request is accepted
/// ([`STAGE_REQUESTED`]) and the directory then sits in THIS stage for as long as the
/// servers it is made of take to come up, reaching [`STAGE_ACTIVE`] only once they are.
/// So the stage a create owes is a CHAIN rather than one step, and every stage-gated
/// operation of a fresh directory is refused from this stage, which is the one a caller
/// polling a create actually observes (`advance_clock` materialises both steps).
pub const STAGE_CREATING: &str = "Creating";

/// How long a created directory is reported [`STAGE_CREATING`] before it reaches
/// [`STAGE_ACTIVE`].
///
/// Its own span rather than [`TRANSITION_SPAN`]: this is the window the directory's
/// servers come up in — Directory Service documents the build of a Managed Microsoft AD
/// directory as taking 20 to 45 minutes — and it is the stage a caller that polls a create
/// spends that whole build in, so it has to be observable by a client that polls at human
/// intervals rather than within one request. Compressed to the few seconds the project's
/// emulator contract allows a clock-owed transition, and the same for every directory
/// type: all of them answer their create before their servers exist.
pub const DIRECTORY_BUILD_SPAN: Duration = Duration::from_secs(3);

/// The instant a directory whose create ran at `now` reaches [`STAGE_ACTIVE`]: the end of
/// the chain that create owes, [`TRANSITION_SPAN`] in [`STAGE_REQUESTED`] followed by
/// [`DIRECTORY_BUILD_SPAN`] in [`STAGE_CREATING`].
///
/// What it is for: the servers a create PLACES are in service when the directory they
/// belong to is, so a create reads the instant its own directory settles on from here
/// rather than restating the chain.
#[must_use]
pub fn directory_active_at(now: SystemTime) -> SystemTime {
    now + TRANSITION_SPAN + DIRECTORY_BUILD_SPAN
}

/// The `IpRouteStatusMsg` an address block carries between `AddIpRoutes` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `IpRouteStatusMsg` enum.
///
/// `AddIpRoutes` answers before the block is in service, so the move to
/// [`IP_ROUTE_STATUS_ADDED`] is owed by the clock, on the same span a create's stage
/// transition uses.
pub const IP_ROUTE_STATUS_ADDING: &str = "Adding";

/// The `IpRouteStatusMsg` an address block reaches once [`TRANSITION_SPAN`] has
/// elapsed: the `Added` value of the modelled `IpRouteStatusMsg` enum.
pub const IP_ROUTE_STATUS_ADDED: &str = "Added";

/// The `IpRouteStatusMsg` an address block carries between `RemoveIpRoutes` and
/// [`TRANSITION_SPAN`]: the `Removing` value of the modelled `IpRouteStatusMsg` enum.
///
/// `RemoveIpRoutes` answers while the block is still being taken out of service, so the
/// block leaving the directory is owed by the clock, on the same span an add's status
/// transition uses. A block that has left is no longer one `ListIpRoutes` reports, which
/// is why no record rests on the enum's `Removed` value.
pub const IP_ROUTE_STATUS_REMOVING: &str = "Removing";

/// The status a snapshot is in between `CreateSnapshot` and [`TRANSITION_SPAN`]: the
/// first value of the modelled `SnapshotStatus` enum.
pub const SNAPSHOT_STATUS_CREATING: &str = "Creating";

/// The status a snapshot reaches once [`TRANSITION_SPAN`] has elapsed: the `Completed`
/// value of the modelled `SnapshotStatus` enum.
pub const SNAPSHOT_STATUS_COMPLETED: &str = "Completed";

/// The `SnapshotType` of a snapshot a caller asked for: the `Manual` value of the
/// modelled `SnapshotType` enum. The enum's other value, `Auto`, is for the snapshots
/// Directory Service takes on its own schedule, which no caller of this emulator starts.
pub const SNAPSHOT_TYPE_MANUAL: &str = "Manual";

/// The `DirectoryType` values a snapshot can be taken of.
///
/// From `CreateSnapshot`'s own documentation: "Creates a snapshot of a Simple AD or
/// Microsoft AD directory in the Amazon Web Services cloud. You cannot take snapshots of
/// AD Connector directories." The modelled `DirectoryType` enum's remaining value,
/// `ADConnector`, is therefore refused, and `SharedMicrosoftAD` — a directory shared from
/// another account, which the owner snapshots — with it.
pub const SNAPSHOTTABLE_DIRECTORY_TYPES: [&str; 2] = [TYPE_SIMPLE_AD, "MicrosoftAD"];

/// The stage a created directory reaches once [`TRANSITION_SPAN`] has elapsed.
pub const STAGE_ACTIVE: &str = "Active";

/// The stage a directory that is in service but degraded reports: the `Impaired` value of
/// the modelled `DirectoryStage` enum
/// (`smithy_query(query="shape",shape="DirectoryStage")`).
///
/// Nothing in this emulator puts a directory in it — no path degrades a directory — but it
/// is one of the two stages that SERVE a mutation whose subject is the directory's DNS
/// configuration ([`CONDITIONAL_FORWARDER_SERVING_STAGES`]), so the guard names it rather
/// than admitting `Active` alone.
pub const STAGE_IMPAIRED: &str = "Impaired";

/// The `DirectoryType` of a directory created by `CreateDirectory`: Simple AD.
///
/// `CreateDirectory`'s own documentation ("Creates a Simple AD directory"), rendered
/// with the `SimpleAD` value of the modelled `DirectoryType` enum.
pub const TYPE_SIMPLE_AD: &str = "SimpleAD";

/// The `DirectoryType` of a directory created by `CreateMicrosoftAD`: AWS Managed
/// Microsoft AD.
///
/// `CreateMicrosoftAD`'s own documentation ("Creates a Microsoft AD directory ... see
/// Managed Microsoft AD"), rendered with the `MicrosoftAD` value of the modelled
/// `DirectoryType` enum.
pub const TYPE_MICROSOFT_AD: &str = "MicrosoftAD";

/// The smaller of the two directory capacities Directory Service publishes: the `Small`
/// value of the modelled `DirectorySize` enum
/// (`smithy_query(query="shape",shape="DirectorySize")` answers `["Small", "Large"]`),
/// which `DirectoryDescription$Size` ("The directory size") reports.
pub const DIRECTORY_SIZE_SMALL: &str = "Small";

/// The larger of those two capacities: the `Large` value of the same modelled enum.
pub const DIRECTORY_SIZE_LARGE: &str = "Large";

/// The `Standard` edition of Managed Microsoft AD, from
/// `CreateMicrosoftADRequest$Edition`'s own documentation (see [`MICROSOFT_AD_EDITIONS`]).
pub const MICROSOFT_AD_EDITION_STANDARD: &str = "Standard";

/// The editions a Managed Microsoft AD directory may be created with.
///
/// From `CreateMicrosoftADRequest$Edition`'s own documentation: "Managed Microsoft AD is
/// available in two editions: `Standard` and `Enterprise`." The third value of the
/// modelled `DirectoryEdition` enum, `Hybrid`, belongs to a hybrid directory and is
/// therefore refused by `CreateMicrosoftAD`.
pub const MICROSOFT_AD_EDITIONS: [&str; 2] =
    [MICROSOFT_AD_EDITION_STANDARD, DEFAULT_MICROSOFT_AD_EDITION];

/// The edition a Managed Microsoft AD directory is created with when the request names
/// none, from the same sentence: "`Enterprise` is the default."
pub const DEFAULT_MICROSOFT_AD_EDITION: &str = "Enterprise";

/// The `DirectorySize` a Managed Microsoft AD directory of `edition` is run at.
///
/// `CreateMicrosoftAD` takes no `Size` member, so a Managed Microsoft AD's size is not
/// echoed from the request: it follows from the edition the directory was created with,
/// which is what Directory Service sizes the directory by — the `Standard` edition is the
/// one for small and midsize organizations, and `Enterprise` the one for organizations
/// with a large directory. The two editions therefore map onto the two values the modelled
/// `DirectorySize` enum declares.
pub fn microsoft_ad_size(edition: &str) -> &'static str {
    if edition == MICROSOFT_AD_EDITION_STANDARD {
        DIRECTORY_SIZE_SMALL
    } else {
        DIRECTORY_SIZE_LARGE
    }
}

/// The number of subnets a directory's servers are placed across.
///
/// From `DirectoryVpcSettings$SubnetIds`' own documentation: "The identifiers of the
/// subnets for the directory servers. The two subnets must be in different Availability
/// Zones. Directory Service creates a directory server and a DNS server in each of these
/// subnets." `DirectoryConnectSettings$SubnetIds` places an AD Connector's servers the
/// same way.
pub const REQUIRED_SUBNETS: usize = 2;

/// The most `SubnetIds` entries a placement may name.
///
/// NOT an AWS quota: a resource guard, and the one that makes resolving a placement's zones
/// against the service that owns subnets ([`subnet_zones::single_zone_defect`]) cost ONE hop
/// whatever the request body carries — the work of that hop is (identifiers asked x subnets
/// the account stores), so the identifiers have to be bounded here, BEFORE any of them is
/// resolved, rather than per hop. Deliberately well above any servable placement — a
/// directory's servers sit in [`REQUIRED_SUBNETS`] subnets — so no request a caller has
/// reason to send reaches it.
pub const MAX_PLACEMENT_SUBNETS: usize = 16;

/// Why the `SubnetIds` of a create's settings block cannot carry a directory's servers,
/// or `None` when they can.
///
/// One rule for every create that places directory servers in a VPC, so `CreateDirectory`,
/// `CreateMicrosoftAD`, `ConnectDirectory` and `AddRegion` cannot disagree about which
/// settings block is servable: a directory's servers sit in [`REQUIRED_SUBNETS`] subnets in
/// two different Availability Zones, so a list naming fewer than two DISTINCT subnets
/// cannot be two zones and is refused before anything is persisted.
///
/// It is also where nothing caller-sized gets past the placement checks: a list longer than
/// [`MAX_PLACEMENT_SUBNETS`] is refused here, before the zones of any of its entries are
/// resolved, so the resolution that follows costs one hop whatever the request carries.
///
/// Whether two DIFFERENT subnets are in two different zones is a fact about the account's
/// NETWORK rather than about the request, so it is not decided here: it is decided against
/// what the service that owns subnets answers ([`subnet_zones::single_zone_defect`] over
/// [`resolve_subnet_networks`], which every placement runs after this check), and refused as
/// the resource-semantics failure it is. This one is refused with [`INVALID_SUBNET_IDS`], the
/// sentence Directory Service states the request-level half of the rule in.
pub fn subnet_pair_defect(subnet_ids: &[String]) -> Option<String> {
    // The guard comes first, so a list no placement could accept is refused on its SIZE
    // before anything is allocated from it or resolved against another service.
    if subnet_ids.len() > MAX_PLACEMENT_SUBNETS {
        return Some(format!(
            "SubnetIds may name at most {MAX_PLACEMENT_SUBNETS} subnets, got {got}",
            got = subnet_ids.len()
        ));
    }
    // Whether the list names two DIFFERENT subnets, decided by a scan rather than by
    // collecting the caller's list into a set: nothing caller-sized is allocated to
    // answer it.
    let two_distinct = subnet_ids
        .first()
        .is_some_and(|first| subnet_ids.iter().any(|subnet_id| subnet_id != first));
    if two_distinct {
        return None;
    }
    Some(INVALID_SUBNET_IDS.to_string())
}

/// The diagnostic Directory Service refuses a settings block whose subnets cannot carry a
/// directory's servers with.
///
/// FIXED text, the rule the subnets broke: it quotes none of them, so the message names
/// neither the ids it rejected nor how many were sent. [`refusal`] appends the request id,
/// once.
pub const INVALID_SUBNET_IDS: &str =
    "Invalid subnet ID(s). They must correspond to two subnets in different Availability \
     Zones.";

/// The diagnostic Directory Service refuses a placement whose VPC the account does not have
/// with.
///
/// FIXED text, the rule the placement broke: it quotes neither the identifier it rejected nor
/// the account, so every caller that names a VPC that is not there reads the same sentence —
/// the wording a recorded refusal of a create against an absent VPC carries.
/// [`refusal`] appends the request id, once. It is the resource-semantics failure class
/// (`ClientException`), the class [`subnet_zones::single_zone_defect`] is refused in, and not
/// the validation refusal a malformed identifier gets: the identifier is well formed and the
/// resource it names does not exist.
pub const INVALID_VPC_ID: &str = "Invalid VPC ID.";

/// The number of domain controllers a Managed Microsoft AD directory is created with.
///
/// The modelled `DesiredNumberOfDomainControllers` carries `range { min: 2 }`, and
/// `DirectoryVpcSettings$SubnetIds`' own documentation says "Directory Service creates a
/// directory server and a DNS server in each of these subnets" — two subnets in
/// different Availability Zones — so a freshly created Managed Microsoft AD directory
/// has exactly the modelled minimum of two. `UpdateNumberOfDomainControllers` is what
/// changes it afterwards.
pub const MICROSOFT_AD_DOMAIN_CONTROLLERS: i32 = 2;

/// The directory types whose domain controllers this service manages and reports.
///
/// A domain controller is a server of the directory's own domain, which only AWS Managed
/// Microsoft AD runs for the caller: `DirectoryDescription$DesiredNumberOfDomainControllers`
/// is documented as "the desired number of domain controllers in the directory IF the
/// directory is Microsoft AD", and `UpdateNumberOfDomainControllers` is what changes that
/// number. So a directory of any other type has no domain controller for
/// `DescribeDomainControllers` to report, and the call is refused with the modelled
/// `UnsupportedOperationException` rather than served as an empty page.
pub const DOMAIN_CONTROLLER_DIRECTORY_TYPES: [&str; 1] = [TYPE_MICROSOFT_AD];

/// The `DomainControllerStatus` a controller carries between its placement and
/// [`TRANSITION_SPAN`]: the first value of the modelled `DomainControllerStatus` enum
/// (`smithy_query(query="shape",shape="DomainControllerStatus")`).
///
/// A create answers before the controllers it places are in service, so the move to
/// [`DOMAIN_CONTROLLER_STATUS_ACTIVE`] is owed by the clock rather than by any later
/// call.
pub const DOMAIN_CONTROLLER_STATUS_CREATING: &str = "Creating";

/// The `DomainControllerStatus` a placed controller reaches once the transition its
/// placement owes falls due: the second value of the same modelled enum, the one a
/// controller in service carries.
pub const DOMAIN_CONTROLLER_STATUS_ACTIVE: &str = "Active";

/// The `DomainControllerStatus` a controller carries while it is being taken out of the
/// directory: the `Deleting` value of the same modelled enum.
///
/// A resize down answers before the controllers it retires are gone, so the removal is
/// owed by the clock one [`TRANSITION_SPAN`] later and the controller is reported in this
/// transient status until then — the same shape as an address block leaving a directory.
pub const DOMAIN_CONTROLLER_STATUS_DELETING: &str = "Deleting";

/// The most `DomainControllerIds` entries a single `DescribeDomainControllers` will
/// resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds a handful of controllers, so a longer list can only
/// repeat ids — and it is what keeps the caller-chosen filter from being allocated and
/// scanned unbounded.
pub const MAX_DOMAIN_CONTROLLER_IDS_PER_REQUEST: usize = 100;

/// Directory Service's default quota of domain controllers per directory, which
/// `UpdateNumberOfDomainControllers` enforces.
///
/// From the modelled `DomainControllerLimitExceededException`'s own documentation: "The
/// maximum allowed number of domain controllers per directory was exceeded. The default
/// limit per directory is 20 domain controllers." A `DesiredNumber` above it is refused
/// with that error, and
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::place_domain_controllers`]
/// clamps what it places to it, so it is also the ceiling of what the store holds for one
/// directory.
pub const MAX_DOMAIN_CONTROLLERS_PER_DIRECTORY: usize = 20;

/// The `DirectoryType` of an AD Connector: the `ADConnector` value of the modelled
/// `DirectoryType` enum, which `ConnectDirectory` is what persists.
pub const TYPE_AD_CONNECTOR: &str = "ADConnector";

/// The DNS suffix a directory's access URL is built on, from the `AccessUrl` member's
/// documentation: "The access URL for the directory, such as
/// `http://<alias>.awsapps.com`".
///
/// The member's documented EXAMPLE carries a scheme; the value Directory Service reports
/// is the bare host it is built on, which is what [`Directory::access_url`] composes.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// The network types a Simple AD directory may be created with.
///
/// From `CreateDirectoryRequest$NetworkType`'s own documentation: "The network type for
/// your directory. Simple AD supports IPv4 and Dual-stack only." The third value of the
/// modelled `NetworkType` enum, `IPv6`, is therefore refused for this directory type.
pub const SIMPLE_AD_NETWORK_TYPES: [&str; 2] = ["IPv4", "Dual-stack"];

/// Directory Service's default quota of Simple AD directories per account per Region,
/// which `CreateDirectory` draws on.
///
/// From the Directory Service quotas the service publishes — "Simple AD directories:
/// 10" per Region — which is also the limit `GetDirectoryLimits` reports as
/// `CloudOnlyDirectoriesLimit` ("The maximum number of cloud directories allowed in the
/// Region").
pub const MAX_CLOUD_ONLY_DIRECTORIES: usize = 10;

/// Directory Service's default quota of AWS Managed Microsoft AD directories per account
/// per Region, which `CreateMicrosoftAD` draws on.
///
/// From the same published quotas — "AWS Managed Microsoft AD directories: 20" per
/// Region — which is the limit `GetDirectoryLimits` reports as
/// `CloudOnlyMicrosoftADLimit` ("The maximum number of Managed Microsoft AD directories
/// allowed in the region"). It is DOUBLE the Simple AD allowance: each category of
/// directory carries its own quota, so no counter of the modelled `DirectoryLimits` can
/// be reported from another's.
pub const MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES: usize = 20;

/// Directory Service's default quota of AD Connector directories per account per Region,
/// which `ConnectDirectory` draws on.
///
/// From the same published quotas — "AD Connector directories: 10" per Region — which is
/// the limit `GetDirectoryLimits` reports as `ConnectedDirectoriesLimit` ("The maximum
/// number of connected directories allowed in the Region").
pub const MAX_CONNECTED_DIRECTORIES: usize = 10;

/// The most directories one Region can hold at once, whatever their type: the three
/// published per-category quotas together.
///
/// NOT a quota of its own — every create counts against ITS OWN category's quota
/// ([`DirectoryLimitCategory::quota`]) — but the ceiling of the directory store, and so
/// the figure the resource guards derived from that store's size are sized against.
pub const MAX_DIRECTORIES: usize = MAX_CLOUD_ONLY_DIRECTORIES
    + MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES
    + MAX_CONNECTED_DIRECTORIES;

/// Which of the three counters the modelled `DirectoryLimits` structure carries a
/// stored directory counts towards, decided by its `DirectoryType`.
///
/// `DirectoryLimits` reports a (limit, current count, limit reached) triple for
/// cloud-only directories, for cloud-only Managed Microsoft AD directories and for
/// connected directories, while the modelled `DirectoryType` enum carries four values.
/// The fourth, `SharedMicrosoftAD`, belongs to none of the three: it is a directory
/// another account shared into this one rather than one created here, and the structure
/// models no counter for it — hence [`Self::of`] answering `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryLimitCategory {
    /// A cloud-only directory — [`TYPE_SIMPLE_AD`].
    CloudOnly,
    /// A cloud-only AWS Managed Microsoft AD directory — [`TYPE_MICROSOFT_AD`].
    CloudOnlyMicrosoftAd,
    /// A directory connected to an on-premises one — [`TYPE_AD_CONNECTOR`].
    Connected,
}

impl DirectoryLimitCategory {
    /// The counter a stored `DirectoryType` counts towards, or `None` when
    /// `DirectoryLimits` models no counter for that type.
    pub fn of(directory_type: &str) -> Option<Self> {
        match directory_type {
            TYPE_SIMPLE_AD => Some(Self::CloudOnly),
            TYPE_MICROSOFT_AD => Some(Self::CloudOnlyMicrosoftAd),
            TYPE_AD_CONNECTOR => Some(Self::Connected),
            _ => None,
        }
    }

    /// This category's own published Region quota: the limit `GetDirectoryLimits`
    /// reports for its counter, and the bound a create of that category is refused past
    /// with `DirectoryLimitExceededException`.
    pub fn quota(self) -> usize {
        match self {
            Self::CloudOnly => MAX_CLOUD_ONLY_DIRECTORIES,
            Self::CloudOnlyMicrosoftAd => MAX_CLOUD_ONLY_MICROSOFT_AD_DIRECTORIES,
            Self::Connected => MAX_CONNECTED_DIRECTORIES,
        }
    }
}

/// AWS's tagging limit of 50 tags per resource, the bound
/// `TagLimitExceededException` reports.
pub const MAX_TAGS: usize = 50;

/// The account quota on directory assessments, the bound
/// `ADAssessmentLimitExceededException` reports.
///
/// From `StartADAssessment`'s own documentation: "Your Amazon Web Services account has a
/// limit of 100 CUSTOMER directory assessments."
pub const MAX_CUSTOMER_ASSESSMENTS: usize = 100;

/// The status an assessment carries while the run it started is still owed by the
/// clock, from `Assessment$Status`'s documentation: "Valid values include `SUCCESS`,
/// `FAILED`, `PENDING`, and `IN_PROGRESS`."
pub const ASSESSMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status an assessment reaches in this emulator, from the same
/// `Assessment$Status` value list. See [`ASSESSMENT_FAILED_REASON`] for why it is this
/// value and not `SUCCESS`.
pub const ASSESSMENT_STATUS_FAILED: &str = "FAILED";

/// Why every assessment this emulator starts ends `FAILED`.
///
/// An assessment "checks compatibility and connectivity of the self-managed AD
/// environment" (`StartADAssessment`'s documentation) by probing the customer's DNS
/// servers and managed nodes. This emulator makes no outbound connection, so that run
/// cannot be performed: the assessment resource is real — it is minted, listed,
/// described and counted against the account quota — and the run it starts ends in the
/// modelled failure status carrying this reason, rather than reporting validations that
/// nothing performed. It is reported through `Assessment$StatusReason`, documented as
/// "A human-readable description of the current assessment status, including any error
/// details or progress information."
pub const ASSESSMENT_FAILED_REASON: &str = "The assessment could not be run: this emulator has no network path to the self-managed AD environment, so no connectivity or compatibility validation was performed.";

/// The `ReportType` of an assessment started by `StartADAssessment`.
///
/// From `Assessment$ReportType`'s documentation ("Valid values are `CUSTOMER` and
/// `SYSTEM`") and `StartADAssessment`'s ("A directory assessment is automatically
/// created when you create a hybrid directory. There are two types of assessments:
/// CUSTOMER and SYSTEM") — a caller-started assessment is the CUSTOMER kind, and it is
/// the CUSTOMER ones [`MAX_CUSTOMER_ASSESSMENTS`] bounds.
pub const ASSESSMENT_REPORT_TYPE_CUSTOMER: &str = "CUSTOMER";

/// The number of IP address blocks a directory may carry, from
/// `IpRouteLimitExceededException`'s own documentation: "The maximum allowed number of
/// IP addresses was exceeded. The default limit is 100 IP address blocks."
pub const MAX_IP_ROUTES: usize = 100;

/// The first RID this emulator hands to a computer account it creates in a directory.
///
/// A Windows account's SID is its domain's SID plus a relative identifier, and Active
/// Directory reserves the RIDs below 1000 for the built-in principals every domain is
/// created with, handing the ones from 1000 up to the accounts created in it. So the
/// first computer account of a directory is `<domain SID>-1000` and each later one takes
/// the next RID, which is what makes a `ComputerId` unique within its directory.
pub const FIRST_COMPUTER_RID: u32 = 1000;

/// The prefix every Active Directory domain account's SID carries: the NT authority
/// (`S-1-5`) and the `SECURITY_NT_NON_UNIQUE` sub-authority (`21`) that marks the three
/// sub-authorities after it as one domain's own identifier. See
/// [`Directory::domain_sid`], which is what appends those three.
pub const DOMAIN_SID_PREFIX: &str = "S-1-5-21";

/// The most computer accounts this emulator keeps in one directory.
///
/// NOT an AWS quota: a resource guard, like [`MAX_SNAPSHOTS`]. `CreateComputer` models no
/// limit-exceeded error and AWS publishes a directory's capacity as an approximate object
/// count rather than an API limit, so this is not an emulated quota — it is what keeps a
/// loop of creates from growing the store without end, set at the smallest capacity AWS
/// publishes for a directory (a Simple AD `Small` directory, documented as up to 500
/// users / roughly 2,000 directory objects). Past it `CreateComputer` is refused with the
/// generic `ClientException` the model offers, since the model declares no bound-specific
/// error for this operation.
pub const MAX_COMPUTERS_PER_DIRECTORY: usize = 2_000;

/// Directory Service's default quota "Manual snapshots: 5" per directory, the bound
/// `SnapshotLimitExceededException` reports — the same bound `GetSnapshotLimits`
/// describes as `SnapshotLimits$ManualSnapshotsLimit`.
pub const MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY: usize = 5;

/// The most snapshots this emulator keeps in total, across every directory.
///
/// NOT an AWS quota: a resource guard. [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] bounds one
/// directory's snapshots and `DeleteDirectory` drops the snapshots of the directory it
/// deletes, so this is the absolute ceiling of the snapshot store — exactly as many as a
/// Region's worth of directories can hold at once — and it is what keeps a
/// create-directory / snapshot / delete-directory cycle from growing the store without
/// end. Past it `CreateSnapshot` is refused.
pub const MAX_SNAPSHOTS: usize = MAX_DIRECTORIES * MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY;

/// The `SchemaExtensionStatus` an extension carries between `StartSchemaExtension` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `SchemaExtensionStatus` enum.
///
/// `StartSchemaExtension` answers while the extension is still being applied, so the
/// move to [`SCHEMA_EXTENSION_STATUS_COMPLETED`] is owed by the clock rather than by any
/// later call, on the same span a create's stage transition uses.
pub const SCHEMA_EXTENSION_STATUS_INITIALIZING: &str = "Initializing";

/// The `SchemaExtensionStatus` an extension reaches once [`TRANSITION_SPAN`] has
/// elapsed: the `Completed` value of the modelled `SchemaExtensionStatus` enum.
pub const SCHEMA_EXTENSION_STATUS_COMPLETED: &str = "Completed";

/// The `SchemaExtensionStatus` an extension carries between `CancelSchemaExtension` and
/// [`TRANSITION_SPAN`]: the `CancelInProgress` value of the modelled
/// `SchemaExtensionStatus` enum.
///
/// The cancel answers while the extension is still being wound back — the enum carries
/// both the transient `CancelInProgress` and the settled
/// [`SCHEMA_EXTENSION_STATUS_CANCELLED`] — so the move to the settled status is owed by
/// the clock rather than by any later call, on the same span the start's transition uses.
pub const SCHEMA_EXTENSION_STATUS_CANCEL_IN_PROGRESS: &str = "CancelInProgress";

/// The `SchemaExtensionStatus` a cancelled extension settles on once
/// [`TRANSITION_SPAN`] has elapsed: the `Cancelled` value of the modelled
/// `SchemaExtensionStatus` enum.
pub const SCHEMA_EXTENSION_STATUS_CANCELLED: &str = "Cancelled";

/// The `SchemaExtensionStatus` values an extension can be cancelled in.
///
/// `CancelSchemaExtension`'s own documentation: "A schema extension can be canceled
/// during any of the following states; Initializing, CreatingSnapshot, and
/// UpdatingSchema" — and "Once a schema extension has started replicating to all domain
/// controllers, the task can no longer be canceled", so every other value of the
/// modelled enum is past the point of no return (or already finished).
pub const CANCELLABLE_SCHEMA_EXTENSION_STATUSES: [&str; 3] = [
    SCHEMA_EXTENSION_STATUS_INITIALIZING,
    "CreatingSnapshot",
    "UpdatingSchema",
];

/// The most schema extensions this emulator keeps for one directory.
///
/// NOT an AWS quota: a resource guard, like [`MAX_COMPUTERS_PER_DIRECTORY`]. AWS
/// publishes no limit on how many schema extensions a directory may have been through,
/// and `StartSchemaExtension` models no limit-exceeded error of its own, so this is not
/// an emulated quota — it is what keeps a loop of extensions from growing the store
/// without end. Past it `StartSchemaExtension` is refused with the generic
/// `ClientException` the model offers.
pub const MAX_SCHEMA_EXTENSIONS_PER_DIRECTORY: usize = 100;

/// The `TopicStatus` of an association this emulator has just made.
///
/// The first value of the modelled `TopicStatus` enum, and the one the `Status`
/// member ("The topic registration status") reports for a registration that holds:
/// the other values describe a topic the SNS data plane this emulator does not have
/// would have to report on.
pub const TOPIC_STATUS_REGISTERED: &str = "Registered";

/// The `DataAccessStatus` of a directory nothing has enabled the Directory Service Data
/// API for: the `Disabled` value of the modelled `DataAccessStatus` enum
/// (`smithy_query(query="shape", shape="DataAccessStatus")` answers
/// `["Disabled", "Disabling", "Enabled", "Enabling", "Failed"]`), and the status
/// `DescribeDirectoryDataAccess` therefore reports for every freshly created directory.
pub const DATA_ACCESS_DISABLED: &str = "Disabled";

/// The `DataAccessStatus` a directory carries between `EnableDirectoryDataAccess` and
/// [`TRANSITION_SPAN`]: the modelled enum's `Enabling` value.
///
/// The enable answers before data access is in service — the enum carries both the
/// transient `Enabling` and the settled `Enabled` — so the move to
/// [`DATA_ACCESS_ENABLED`] is owed by the clock, on the same span a create's stage
/// transition uses.
pub const DATA_ACCESS_ENABLING: &str = "Enabling";

/// The `DataAccessStatus` a directory reaches once the enable's [`TRANSITION_SPAN`] has
/// elapsed: the modelled enum's `Enabled` value.
pub const DATA_ACCESS_ENABLED: &str = "Enabled";

/// The `DataAccessStatus` a directory carries between `DisableDirectoryDataAccess` and
/// [`TRANSITION_SPAN`]: the modelled enum's `Disabling` value.
///
/// The disable answers before data access has actually been withdrawn — the enum carries
/// both the transient `Disabling` and the settled [`DATA_ACCESS_DISABLED`] — so the move
/// back to [`DATA_ACCESS_DISABLED`] is owed by the clock, on the same span the enable's
/// transition uses.
pub const DATA_ACCESS_DISABLING: &str = "Disabling";

/// The `CertificateState` a certificate carries between `RegisterCertificate` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `CertificateState` enum
/// (`smithy_query(query="shape", shape="CertificateState")` answers `["Registering",
/// "Registered", "RegisterFailed", "Deregistering", "Deregistered",
/// "DeregisterFailed"]`).
///
/// The register answers before the certificate is in service — the enum carries both
/// the transient `Registering` and the settled `Registered` — so the move to
/// [`CERTIFICATE_STATE_REGISTERED`] is owed by the clock, on the same span a create's
/// stage transition uses.
pub const CERTIFICATE_STATE_REGISTERING: &str = "Registering";

/// The `CertificateState` a certificate reaches once the register's
/// [`TRANSITION_SPAN`] has elapsed: the modelled enum's `Registered` value.
pub const CERTIFICATE_STATE_REGISTERED: &str = "Registered";

/// The `CertificateState` a certificate carries between `DeregisterCertificate` and
/// [`TRANSITION_SPAN`]: the `Deregistering` value of the modelled `CertificateState`
/// enum.
///
/// The deregister answers while the certificate is still being taken out of the system,
/// so the registration leaving the directory is owed by the clock, on the same span a
/// register's state transition uses. A registration that has left is no longer one
/// `DescribeCertificate` or `ListCertificates` reports, which is why no record rests on
/// the enum's `Deregistered` value — the same construction as
/// [`IP_ROUTE_STATUS_REMOVING`].
pub const CERTIFICATE_STATE_DEREGISTERING: &str = "Deregistering";

/// The `CertificateType` a certificate that secures LDAP calls performs: the
/// `ClientLDAPS` value of the modelled `CertificateType` enum
/// (`smithy_query(query="shape", shape="CertificateType")` answers `["ClientCertAuth",
/// "ClientLDAPS"]`).
///
/// This is the certificate a directory has to hold before LDAP secure calls can be
/// activated for it, so `EnableLDAPS` resolves the registrations of this type and
/// `RegisterCertificate` performs it by default.
pub const CERTIFICATE_TYPE_CLIENT_LDAPS: &str = "ClientLDAPS";

/// The `CertificateType` a registration that names none performs.
///
/// From the `Type` member's own documentation: "Valid values include `ClientLDAPS` or
/// `ClientCertAuth`. The default value is `ClientLDAPS`."
pub const DEFAULT_CERTIFICATE_TYPE: &str = CERTIFICATE_TYPE_CLIENT_LDAPS;

/// The `LDAPSStatus` a directory's LDAP security carries between `EnableLDAPS` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `LDAPSStatus` enum
/// (`smithy_query(query="shape", shape="LDAPSStatus")` answers `["Enabling", "Enabled",
/// "EnableFailed", "Disabled"]`).
///
/// `EnableLDAPS` "activates the switch for the specific directory to always use LDAP
/// secure calls" and answers before that switch is in service — the enum carries both
/// the transient `Enabling` and the settled `Enabled` — so the move to
/// [`LDAPS_STATUS_ENABLED`] is owed by the clock, on the same span a create's stage
/// transition uses.
pub const LDAPS_STATUS_ENABLING: &str = "Enabling";

/// The `LDAPSStatus` a directory's LDAP security reaches once the enable's
/// [`TRANSITION_SPAN`] has elapsed: the modelled enum's `Enabled` value.
pub const LDAPS_STATUS_ENABLED: &str = "Enabled";

/// The `LDAPSStatus` LDAP security carries once `DisableLDAPS` has deactivated it: the
/// modelled enum's `Disabled` value.
///
/// The enum carries no transient value on the way down — there is no `Disabling` — so a
/// deactivation settles the instant it is recorded and owes the clock nothing.
pub const LDAPS_STATUS_DISABLED: &str = "Disabled";

/// The `DirectoryType` values LDAP secure calls can be activated for.
///
/// LDAPS secures the LDAP traffic between a directory and its clients, which is a
/// feature of the directory types that expose a directory server to them: AWS Managed
/// Microsoft AD (server-side LDAPS) and AD Connector (client-side LDAPS). A Simple AD
/// directory serves no LDAPS endpoint, so activating it there asks for something that
/// directory type does not support — which is what `EnableLDAPS` and `DisableLDAPS`
/// refuse with the modelled `UnsupportedOperationException`.
pub const LDAPS_SUPPORTED_DIRECTORY_TYPES: [&str; 2] = [TYPE_MICROSOFT_AD, TYPE_AD_CONNECTOR];

/// The most certificates this emulator will register with one directory.
///
/// NOT an AWS quota: a resource guard, the same construction as
/// [`MAX_EVENT_TOPICS_PER_DIRECTORY`]. `RegisterCertificate` is a write a caller can
/// repeat with fresh certificate data forever, so the per-directory index carries its
/// own bound, enforced at the single insert site in `register_certificate.rs` — which
/// is what the modelled `CertificateLimitExceededException` ("the certificate limit has
/// been reached") then refuses.
pub const MAX_CERTIFICATES_PER_DIRECTORY: usize = 10;

/// The directory types the function a certificate performs is offered for, by
/// `CertificateType`.
///
/// Which certificates a directory can register follows from what the certificate DOES:
/// client-side LDAPS, which a [`CERTIFICATE_TYPE_CLIENT_LDAPS`] certificate secures, is
/// offered for the types in [`LDAPS_SUPPORTED_DIRECTORY_TYPES`] — the same set
/// `EnableLDAPS` serves — while client authentication, which a
/// [`CERTIFICATE_TYPE_CLIENT_CERT_AUTH`] certificate configures, is offered for the types
/// in [`CLIENT_AUTHENTICATION_DIRECTORY_TYPES`]. A registration asking for a function the
/// directory's own type does not offer is refused with the modelled
/// `UnsupportedOperationException`.
pub fn certificate_function_directory_types(certificate_type: &str) -> &'static [&'static str] {
    if certificate_type == CERTIFICATE_TYPE_CLIENT_CERT_AUTH {
        &CLIENT_AUTHENTICATION_DIRECTORY_TYPES
    } else {
        &LDAPS_SUPPORTED_DIRECTORY_TYPES
    }
}

/// The `TrustState` a trust relationship carries between `CreateTrust` and
/// [`TRANSITION_SPAN`]: the first value of the modelled `TrustState` enum
/// (`smithy_query(query="shape",shape="TrustState")`).
///
/// `CreateTrust` "initiates the creation of the Amazon Web Services side of a trust
/// relationship", so it answers while the relationship is still being established and the
/// move to [`TRUST_STATE_CREATED`] is owed by the clock rather than by any later call, on
/// the same span a directory's stage transition uses.
pub const TRUST_STATE_CREATING: &str = "Creating";

/// The `TrustState` a trust relationship reaches once its [`TRANSITION_SPAN`] has
/// elapsed: the `Created` value of the modelled `TrustState` enum.
pub const TRUST_STATE_CREATED: &str = "Created";

/// The `TrustState` a trust relationship carries between `VerifyTrust` and
/// [`TRANSITION_SPAN`]: the `Verifying` value of the modelled `TrustState` enum.
///
/// `VerifyTrust` starts the verification rather than reporting its outcome — the enum
/// carries both the transient `Verifying` and the settled `Verified` — so the move to
/// [`TRUST_STATE_VERIFIED`] is owed by the clock, on the same span a create's transition
/// uses.
pub const TRUST_STATE_VERIFYING: &str = "Verifying";

/// The `TrustState` a trust relationship reaches once its verification's
/// [`TRANSITION_SPAN`] has elapsed: the `Verified` value of the modelled `TrustState`
/// enum.
pub const TRUST_STATE_VERIFIED: &str = "Verified";

/// The `TrustState` a trust relationship whose establishment did not succeed settles on:
/// the `Failed` value of the modelled `TrustState` enum.
///
/// The Amazon Web Services side of a trust can only be established against a peer the
/// remote domain actually has, so a create naming a domain no peer answers for settles here
/// instead of on [`TRUST_STATE_CREATED`] — see [`remote_domain_has_a_peer`] and
/// [`TRUST_CREATE_FAILURE_REASON`].
pub const TRUST_STATE_FAILED: &str = "Failed";

/// The `TrustStateReason` ("The reason for the TrustState") a trust that settled on
/// [`TRUST_STATE_FAILED`] carries: the sentence Directory Service states for a trust
/// whose creation did not complete. FIXED text with no interpolated values.
pub const TRUST_CREATE_FAILURE_REASON: &str =
    "An internal service error has been encountered while creating the trust. Please retry \
     the operation.";

/// The top-level domains the IETF reserves as names that are NEVER installed in the
/// global DNS, so a domain under one of them has no peer anything can reach: `invalid`,
/// `test` and `localhost` are the special-use names RFC 6761 reserves for exactly that,
/// and `example` is the TLD RFC 2606 reserves for documentation (the delegated
/// `example.com`, `example.net` and `example.org` are ordinary resolvable names and are
/// NOT in this set).
///
/// This is the set [`remote_domain_has_a_reserved_top_level_domain`] decides an external
/// domain by: this emulator resolves no name and opens no socket — the decision is taken
/// from the stored name's own last label and nothing else — so the names the IETF
/// guarantees are never installed are the ones a trust cannot be established with.
pub const NON_RESOLVABLE_TOP_LEVEL_DOMAINS: [&str; 4] =
    ["invalid", "test", "localhost", "example"];

/// Whether `remote_domain_name` sits under one of [`NON_RESOLVABLE_TOP_LEVEL_DOMAINS`] —
/// a name the IETF reserves as never installed in the global DNS, so no peer can exist at
/// it and a trust to it cannot be established.
///
/// A STRING comparison and nothing else: the name's last label is compared against that
/// fixed set, case-insensitively and without the trailing root dot, the way [`domain_key`]
/// compares one domain name with another. No name resolution is performed, no resolver is
/// consulted and no socket is opened — this emulator reaches nothing off its own process,
/// so the outcome is the same on every host and cannot block a caller.
pub fn remote_domain_has_a_reserved_top_level_domain(remote_domain_name: &str) -> bool {
    let key = domain_key(remote_domain_name);
    let top_level = key.rsplit('.').next().unwrap_or_default();
    NON_RESOLVABLE_TOP_LEVEL_DOMAINS.contains(&top_level)
}

/// Whether a peer answers for `remote_domain_name`, which is what decides whether the
/// Amazon Web Services side of a trust with it can be established at all.
///
/// Two ways a peer can be there, and the order matters:
///
/// * `domains_served` holds the domains this service itself serves — the domain name of
///   every directory it stores
///   ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::domains_served`]) — and a
///   directory IS a peer at its own domain, answering DNS at the addresses
///   `DirectoryDescription$DnsIpAddrs` reports for it. A trust pointed at one of those
///   domains therefore has a peer whatever the name looks like, which is what makes a trust
///   between two directories of this service establish;
/// * otherwise the name is external, and one under a top-level domain the IETF reserves as
///   never installed ([`remote_domain_has_a_reserved_top_level_domain`]) can have no peer
///   anywhere, so a trust to it cannot be established;
/// * every other external name is taken to have one.
///
/// A STRING comparison over stored state and nothing else: no name is resolved, no resolver
/// is consulted and no socket is opened, so the outcome is the same on every host.
pub fn remote_domain_has_a_peer(
    remote_domain_name: &str,
    domains_served: &BTreeSet<String>,
) -> bool {
    domains_served.contains(&domain_key(remote_domain_name))
        || !remote_domain_has_a_reserved_top_level_domain(remote_domain_name)
}

/// The `TrustState` a trust relationship carries between `DeleteTrust` and
/// [`TRANSITION_SPAN`], after which the relationship is gone: the `Deleting` value of the
/// modelled `TrustState` enum.
///
/// `DeleteTrust` answers while the Amazon Web Services side is still being torn down, so a
/// caller acting straight after it still sees the trust in this state, and the removal is
/// owed by the clock rather than by any later call.
pub const TRUST_STATE_DELETING: &str = "Deleting";

/// The `TrustState` a torn-down trust relationship settles on before it leaves this
/// service: the `Deleted` value of the modelled `TrustState` enum.
///
/// The teardown `DeleteTrust` starts has a settled state of its own — the enum carries
/// `Deleted` beside the transient `Deleting` — so a trust reports it once the teardown has
/// fallen due, and the record leaves the store one span later, because a relationship the
/// service no longer holds is not one `DescribeTrusts` reports. A trust in either of these
/// two states is already torn down, so it is not one
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::trust_of`] resolves: the
/// directory can establish a trust with that remote domain again.
pub const TRUST_STATE_DELETED: &str = "Deleted";

/// The `TrustState` values of a relationship that is being, or has been, torn down —
/// [`TRUST_STATE_DELETING`] and [`TRUST_STATE_DELETED`].
///
/// A trust in one of them is still reported while the clock works through the teardown,
/// but it is no longer the relationship the directory has with that remote domain.
pub const TORN_DOWN_TRUST_STATES: [&str; 2] = [TRUST_STATE_DELETING, TRUST_STATE_DELETED];

/// The `TrustState` values of a relationship the directory no longer HOLDS: the teardown
/// states ([`TORN_DOWN_TRUST_STATES`]) and [`TRUST_STATE_FAILED`], where nothing was
/// established at all.
///
/// Such a trust is still reported, and still counted against the per-directory trust
/// quota while the store holds it
/// ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::trust_count`]), but it is
/// not the relationship the directory HAS with that remote domain — so it is not what a
/// second `CreateTrust` for that domain collides with
/// ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::trust_of`]).
pub const TRUST_STATES_NOT_HELD: [&str; 3] = [
    TRUST_STATE_DELETING,
    TRUST_STATE_DELETED,
    TRUST_STATE_FAILED,
];

/// The `TrustState` values a trust relationship can be verified from: the states in
/// which the relationship EXISTS and nothing has failed or torn it down.
///
/// `VerifyTrust` "verifies a trust relationship between your Managed Microsoft AD
/// directory and an external domain", so it needs a relationship there is something to
/// verify about. The remaining values of the modelled `TrustState` enum are the failures
/// ([`TRUST_STATE_FAILED`], `VerifyFailed`, `UpdateFailed`) and the teardown
/// ([`TRUST_STATE_DELETING`], [`TRUST_STATE_DELETED`]), and a verification asked for from
/// one of those is refused with [`cannot_verify_trust`].
pub const VERIFIABLE_TRUST_STATES: [&str; 6] = [
    TRUST_STATE_CREATING,
    TRUST_STATE_CREATED,
    TRUST_STATE_VERIFYING,
    TRUST_STATE_VERIFIED,
    "Updating",
    "Updated",
];

/// The `TrustType` a request that names none gets: the `Forest` value of the modelled
/// `TrustType` enum, from `CreateTrustRequest$TrustType`'s own documentation — "The trust
/// relationship type. `Forest` is the default."
pub const TRUST_TYPE_FOREST: &str = "Forest";

/// The `SelectiveAuth` a trust relationship whose request asked for none carries: the
/// `Disabled` value of the modelled `SelectiveAuth` enum.
///
/// `CreateTrustRequest$SelectiveAuth` is the "optional parameter to enable selective
/// authentication for the trust", and `Trust$SelectiveAuth` reports the "current state of
/// selective authentication" — a state every stored trust has, so a request that enabled
/// nothing leaves it at the enum's not-enabled value.
pub const SELECTIVE_AUTH_DISABLED: &str = "Disabled";

/// The ARN service segment of an Amazon SNS topic, so a reported `TopicArn` is built
/// through [`::engine::emulator_arn`] rather than written as a literal.
const SNS_ARN_SERVICE: &str = "sns";

/// The Region an ARN this service reports carries, and the Region a request that acts
/// per Region is taken to name, when the request carries no sigv4 credential scope to
/// read one from — the default `emulator_arn` documents for generated emulators.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most Amazon SNS topics this emulator will associate with one directory.
///
/// NOT an AWS quota: a resource guard. `RegisterEventTopic` is the one write a caller
/// can repeat with a fresh name forever, so the per-directory index carries its own
/// bound, enforced at the single insert site in `register_event_topic.rs`.
pub const MAX_EVENT_TOPICS_PER_DIRECTORY: usize = 50;

/// The `ReplicationScope` every stored conditional forwarder carries: the only value the
/// modelled `ReplicationScope` enum declares, as the
/// `ConditionalForwarder$ReplicationScope` member's own documentation states — "The only
/// allowed value is `Domain`, which will replicate the conditional forwarder to all of
/// the domain controllers for your Amazon Web Services directory."
pub const REPLICATION_SCOPE_DOMAIN: &str = "Domain";

/// The most conditional forwarders this emulator will store on one directory.
///
/// NOT an AWS quota: a resource guard. `CreateConditionalForwarder` is a write a caller
/// can repeat with a fresh remote domain name forever, so the per-directory index carries
/// its own bound, enforced at the single insert site in
/// `create_conditional_forwarder.rs`.
pub const MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY: usize = 50;

/// The most `RemoteDomainNames` entries a single `DescribeConditionalForwarders` will
/// resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most
/// [`MAX_CONDITIONAL_FORWARDERS_PER_DIRECTORY`] forwarders, so a longer list can only
/// repeat names — and it is what keeps the caller-chosen filter from being allocated and
/// scanned unbounded.
pub const MAX_REMOTE_DOMAIN_NAMES_PER_REQUEST: usize = 100;

/// The most `TopicNames` entries a single `DescribeEventTopics` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most [`MAX_EVENT_TOPICS_PER_DIRECTORY`]
/// associations, so a longer list can only repeat names — and it is what keeps the
/// caller-chosen filter from being allocated and scanned unbounded.
pub const MAX_TOPIC_NAMES_PER_REQUEST: usize = 100;

/// The most entries this emulator keeps in the alias reservation index.
///
/// NOT an AWS quota: a resource guard. An alias "cannot be deleted or reused", so an
/// entry outlives the directory it was created for and the index is the one store here
/// that a caller can grow without deleting anything (create, alias, delete, repeat).
/// Stated as its own figure rather than derived from a quota, so it does not move when a
/// published per-category quota does: it is well above the [`MAX_DIRECTORIES`]
/// directories a Region can hold at once, and past it `CreateAlias` is refused.
pub const MAX_RESERVED_ALIASES: usize = 100;

/// The most deleted `DirectoryId`s this emulator remembers (see
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::deleted_directories`]).
///
/// NOT an AWS quota: a resource guard, the twin of [`MAX_RESERVED_ALIASES`] over the other
/// store a create/delete cycle grows. Well above the [`MAX_DIRECTORIES`] directories a
/// Region can hold at once, so every id a client could still be polling for after its own
/// deletions is remembered; past it the earliest-created id is dropped.
pub const MAX_REMEMBERED_DELETIONS: usize = 1_000;

/// The most `DirectoryIds` entries a single `DescribeDirectories` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a Region holds at most [`MAX_DIRECTORIES`] directories, so a longer
/// list can only repeat ids — and it is what keeps the caller-chosen list from being
/// allocated and scanned unbounded.
pub const MAX_DIRECTORY_IDS_PER_REQUEST: usize = 100;

/// The most `SnapshotIds` entries a single `DescribeSnapshots` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — an account holds at most
/// [`MAX_DIRECTORIES`] × [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] snapshots, so a longer
/// list can only repeat ids — and it is what keeps the caller-chosen list from being
/// allocated and scanned unbounded.
pub const MAX_SNAPSHOT_IDS_PER_REQUEST: usize = 100;

/// The most `Settings` entries a single `UpdateSettings` will apply.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory carries [`DIRECTORY_SETTINGS`]`.len()` settings, so a
/// longer list can only repeat names — and it is what keeps the caller-chosen list from
/// being scanned unbounded, and from being scanned at all while the store lock is held.
pub const MAX_SETTINGS_PER_REQUEST: usize = 100;

/// The most trust relationships this emulator will store on one directory.
///
/// NOT an AWS quota: a resource guard. `CreateTrust` is a write a caller can repeat with
/// a fresh remote domain name forever, so the trust store carries its own per-directory
/// bound, enforced at the single insert site in `create_trust.rs`.
pub const MAX_TRUSTS_PER_DIRECTORY: usize = 50;

/// The most `TrustIds` entries a single `DescribeTrusts` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most [`MAX_TRUSTS_PER_DIRECTORY`] trusts, so a
/// longer list can only repeat ids — and it is what keeps the caller-chosen filter from
/// being allocated and scanned unbounded.
pub const MAX_TRUST_IDS_PER_REQUEST: usize = 100;

/// The `ShareMethod` that shares a directory inside the caller's organization: the
/// `ORGANIZATIONS` value of the modelled `ShareMethod` enum. Such a share needs no
/// acceptance from the consumer account, so it starts at [`SHARE_STATUS_SHARING`].
pub const SHARE_METHOD_ORGANIZATIONS: &str = "ORGANIZATIONS";

/// The `ShareMethod` that sends a sharing request to the consumer account: the
/// `HANDSHAKE` value of the modelled `ShareMethod` enum. Such a share waits for the
/// consumer to answer, so it rests at [`SHARE_STATUS_PENDING_ACCEPTANCE`].
pub const SHARE_METHOD_HANDSHAKE: &str = "HANDSHAKE";

/// The `ShareStatus` a share waiting for the consumer account to answer carries: the
/// `PendingAcceptance` value of the modelled `ShareStatus` enum.
///
/// Not a transient the clock settles: only `AcceptSharedDirectory` or
/// `RejectSharedDirectory` moves a share out of it.
pub const SHARE_STATUS_PENDING_ACCEPTANCE: &str = "PendingAcceptance";

/// The `ShareStatus` an organization share is in between `ShareDirectory` and
/// [`TRANSITION_SPAN`]: the `Sharing` value of the modelled `ShareStatus` enum.
pub const SHARE_STATUS_SHARING: &str = "Sharing";

/// The `ShareStatus` a share reaches once it is in service: the `Shared` value of the
/// modelled `ShareStatus` enum.
pub const SHARE_STATUS_SHARED: &str = "Shared";

/// The `ShareStatus` a share carries between `UnshareDirectory` and [`TRANSITION_SPAN`]:
/// the `Deleting` value of the modelled `ShareStatus` enum.
///
/// `UnshareDirectory` answers while the shared directory in the consumer account is still
/// being torn down, so the move out of this status is owed by the clock. There is no
/// settled status to move to: unsharing takes the consumer's shared directory away, so
/// once the teardown falls due the share is gone from this service — the same way
/// [`TrustRecord`] handles [`TRUST_STATE_DELETING`].
pub const SHARE_STATUS_DELETING: &str = "Deleting";

/// The most consumer accounts one directory can be shared with in this emulator.
///
/// NOT an AWS quota: a resource guard. `ShareDirectory` is a write a caller can repeat
/// with a fresh consumer account forever, so the share store carries its own
/// per-directory bound, enforced at the single insert site in `share_directory.rs`.
pub const MAX_SHARES_PER_DIRECTORY: usize = 50;

/// The most `SharedDirectoryIds` entries a single `DescribeSharedDirectories` will
/// resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory is shared with at most [`MAX_SHARES_PER_DIRECTORY`]
/// accounts, so a longer list can only repeat ids — and it is what keeps the
/// caller-chosen filter from being allocated and scanned unbounded.
pub const MAX_SHARED_DIRECTORY_IDS_PER_REQUEST: usize = 100;

/// The page size a paginated operation uses when the request declares none.
///
/// NOT an AWS quota: `Limit`'s own documentation says "If this value is zero, the
/// maximum number of items is specified by the limitations of the operation", and this
/// is that limitation for this emulator. It is above [`MAX_DIRECTORIES`] and
/// [`MAX_TAGS`], so an unpaginated caller always sees everything stored.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The longest `NextToken` this service will decode.
///
/// NOT an AWS quota: a resource guard. Every token this service issues is far shorter
/// (the kind tag, a creation rank or a tag key, and the listing's filter, hex-encoded),
/// so a longer one cannot be one of ours and is refused BEFORE anything is allocated
/// from it.
pub const MAX_NEXT_TOKEN_CHARS: usize = 1024;

/// The version tag every [`encode_cursor`] token carries, so a token minted by an older
/// build is refused rather than misread.
const CURSOR_VERSION: &str = "v1";

/// One IP address block `AddIpRoutes` added to a directory, as `ListIpRoutes` reports
/// it.
///
/// A block is identified by the CIDR it routes — exactly one of `CidrIp` (IPv4) or
/// `CidrIpv6` — which is why [`Self::block`] is what the per-directory map is keyed by:
/// a second `AddIpRoutes` naming a block already routed is a duplicate, not an update.
#[derive(Debug, Clone)]
pub struct IpRouteRecord {
    /// The IPv4 block this route carries, when the caller gave one.
    pub cidr_ip: Option<String>,
    /// The IPv6 block this route carries, when the caller gave one.
    pub cidr_ipv6: Option<String>,
    /// The caller's description of the block, absent when the request carried none.
    pub description: Option<String>,
    /// One of the modelled `IpRouteStatusMsg` values.
    pub status: String,
    /// When the status transition the add owes, or the removal `RemoveIpRoutes` owes,
    /// falls due; `None` once there is none left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// When `AddIpRoutes` added the block, as `AddedDateTime` reports it.
    pub added_at: SystemTime,
}

impl IpRouteRecord {
    /// The address block this route routes: the IPv4 CIDR when it carries one, the
    /// IPv6 CIDR otherwise. This is the identity of the route within a directory.
    pub fn block(&self) -> &str {
        self.cidr_ip
            .as_deref()
            .or(self.cidr_ipv6.as_deref())
            .unwrap_or_default()
    }

    /// This route as the modelled `IpRouteInfo` of `directory_id`.
    ///
    /// `IpRouteStatusReason` ("The reason for the IpRouteStatusMsg") stays absent: a
    /// route this emulator adds never fails, so there is no reason to report.
    pub fn to_info(&self, directory_id: &str) -> IpRouteInfo {
        IpRouteInfo {
            directory_id: Some(directory_id.to_string()),
            cidr_ip: self.cidr_ip.clone(),
            cidr_ipv6: self.cidr_ipv6.clone(),
            ip_route_status_msg: Some(self.status.clone()),
            added_date_time: Some(as_timestamp(self.added_at)),
            ip_route_status_reason: None,
            description: self.description.clone(),
        }
    }
}

/// The LDAPS security recorded for one `LDAPSType` of a directory, as
/// `DescribeLDAPSSettings` reports it.
///
/// `EnableLDAPS` and `DisableLDAPS` are what record and change one, so a directory
/// nothing has enabled LDAPS for carries no entry at all — which is what
/// `DescribeLDAPSSettings` then reports: an empty list.
#[derive(Debug, Clone)]
pub struct LdapsSetting {
    /// One of the modelled `LDAPSStatus` values.
    pub status: String,
    /// The modelled `LDAPSStatusReason`, "Describes a state change for LDAPS".
    pub status_reason: Option<String>,
    /// When this setting last changed, as `LastUpdatedDateTime` reports it.
    pub last_updated: SystemTime,
    /// When the status transition the enable owes falls due; `None` once there is none
    /// left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
}

impl LdapsSetting {
    /// This setting as the modelled `LDAPSSettingInfo`.
    pub fn to_info(&self) -> LDAPSSettingInfo {
        LDAPSSettingInfo {
            ldaps_status: Some(self.status.clone()),
            ldaps_status_reason: self.status_reason.clone(),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

/// The client authentication recorded for one `ClientAuthenticationType` of a
/// directory, as `DescribeClientAuthenticationSettings` reports it.
///
/// `EnableClientAuthentication` and `DisableClientAuthentication` (both still
/// scaffolded) are what record and change one, so a directory nothing has configured
/// client authentication for carries no entry and reports an empty list.
#[derive(Debug, Clone)]
pub struct ClientAuthenticationSetting {
    /// One of the modelled `ClientAuthenticationStatus` values.
    pub status: String,
    /// When this setting last changed, as `LastUpdatedDateTime` reports it.
    pub last_updated: SystemTime,
}

impl ClientAuthenticationSetting {
    /// This setting as the modelled `ClientAuthenticationSettingInfo` for
    /// `authentication_type`.
    pub fn to_info(&self, authentication_type: &str) -> ClientAuthenticationSettingInfo {
        ClientAuthenticationSettingInfo {
            r#type: Some(authentication_type.to_string()),
            status: Some(self.status.clone()),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

/// One registered certificate: what `RegisterCertificate` persisted and
/// `DescribeCertificate` reports.
///
/// Named `CertificateRecord` rather than `Certificate` so it does not collide with the
/// modelled `Certificate` shape [`Self::to_certificate`] renders it into — the same
/// reason [`ComputerRecord`] is named the way it is.
#[derive(Debug, Clone)]
pub struct CertificateRecord {
    /// The id this registration was minted, as the modelled `CertificateId` reports it.
    pub certificate_id: String,
    /// One of the modelled `CertificateState` values.
    pub state: String,
    /// When the state transition the register owes falls due; `None` once there is none
    /// left to make. See [`TRANSITION_SPAN`].
    pub state_due_at: Option<SystemTime>,
    /// The modelled `CertificateStateReason`, "Describes a state change for the
    /// certificate": absent while nothing has changed the state but the clock.
    pub state_reason: Option<String>,
    /// The certificate's own subject common name, as the modelled `CommonName` reports
    /// it: read out of the registered certificate by [`parse_certificate`], and absent
    /// when its subject carries no common-name attribute.
    pub common_name: Option<String>,
    /// "The date and time that the certificate was registered".
    pub registered_at: SystemTime,
    /// "The date and time when the certificate will expire": the registered
    /// certificate's own `notAfter` validity bound, not a span this emulator chose.
    pub expires_at: SystemTime,
    /// One of the modelled `CertificateType` values; [`DEFAULT_CERTIFICATE_TYPE`] when
    /// the register named none.
    pub certificate_type: String,
    /// The `ClientCertAuthSettings` the register was called with, reported back
    /// unchanged; absent when it carried none.
    pub client_cert_auth_settings: Option<ClientCertAuthSettings>,
    /// The registered certificate itself, in DER form — [`parse_certificate`] is what
    /// decodes it out of the PEM the caller sent. Held because it is the certificate,
    /// rather than the text it arrived as, that a directory registers at most once: this
    /// is what the duplicate check in `register_certificate.rs` compares, so two
    /// certificates that differ only in the common name they state are two
    /// registrations, and one certificate sent twice is one.
    pub der: Vec<u8>,
    /// Registration rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl CertificateRecord {
    /// This registration as the modelled `Certificate` structure.
    pub fn to_certificate(&self) -> Certificate {
        Certificate {
            certificate_id: Some(self.certificate_id.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            common_name: self.common_name.clone(),
            registered_date_time: Some(as_timestamp(self.registered_at)),
            expiry_date_time: Some(as_timestamp(self.expires_at)),
            r#type: Some(self.certificate_type.clone()),
            client_cert_auth_settings: self.client_cert_auth_settings.clone(),
        }
    }

    /// This registration as the modelled `CertificateInfo` structure: the "general
    /// information about a certificate" a listing reports, which is this same stored
    /// record narrowed to the members that shape declares — so a listed entry and a
    /// described one can never disagree.
    pub fn to_certificate_info(&self) -> CertificateInfo {
        CertificateInfo {
            certificate_id: Some(self.certificate_id.clone()),
            common_name: self.common_name.clone(),
            state: Some(self.state.clone()),
            expiry_date_time: Some(as_timestamp(self.expires_at)),
            r#type: Some(self.certificate_type.clone()),
        }
    }
}

/// What a registered certificate itself states: the values a handler must read out of
/// the certificate rather than invent for it.
#[derive(Debug, Clone)]
pub struct ParsedCertificate {
    /// The subject's common-name attribute, when it carries one.
    pub common_name: Option<String>,
    /// The certificate's `notAfter` validity bound.
    pub expires_at: SystemTime,
    /// The certificate in DER form, so two registrations of one certificate compare
    /// equal whatever their PEM framing.
    pub der: Vec<u8>,
}

/// The object identifier of the X.509 `commonName` attribute type, `2.5.4.3`
/// (RFC 4519 section 2.3). Stated here rather than taken from an OID database so the
/// lookup needs no optional feature of the parser.
const COMMON_NAME_OID: ::x509_cert::der::asn1::ObjectIdentifier =
    ::x509_cert::der::asn1::ObjectIdentifier::new_unwrap("2.5.4.3");

/// One subject attribute as a string, whichever of the ASN.1 string types X.509 allows
/// for it the certificate used; `None` when it is not a string at all.
fn attribute_string(attribute: &::x509_cert::attr::AttributeTypeAndValue) -> Option<String> {
    use ::x509_cert::der::asn1::{Ia5StringRef, PrintableStringRef, TeletexStringRef, Utf8StringRef};

    if let Ok(value) = attribute.value.decode_as::<Utf8StringRef<'_>>() {
        return Some(value.as_str().to_string());
    }
    if let Ok(value) = attribute.value.decode_as::<PrintableStringRef<'_>>() {
        return Some(value.as_str().to_string());
    }
    if let Ok(value) = attribute.value.decode_as::<Ia5StringRef<'_>>() {
        return Some(value.as_str().to_string());
    }
    attribute
        .value
        .decode_as::<TeletexStringRef<'_>>()
        .ok()
        .map(|value| value.as_str().to_string())
}

/// The certificate `pem` carries, or `None` when `pem` is not a PEM-encoded X.509
/// certificate — which is what the modelled `InvalidCertificateException` ("The
/// certificate PEM that was provided has incorrect encoding") refuses.
///
/// The PEM and the DER inside it are decoded by the `x509-cert` parser rather than by
/// anything written here, so the reported `CommonName` and `ExpiryDateTime` are the
/// certificate's own values and a certificate this emulator cannot parse is refused
/// instead of accepted with invented ones.
pub fn parse_certificate(pem: &str) -> Option<ParsedCertificate> {
    use ::x509_cert::der::{DecodePem, Encode};

    let certificate = ::x509_cert::Certificate::from_pem(pem.as_bytes()).ok()?;
    let common_name = certificate
        .tbs_certificate
        .subject
        .0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .find(|attribute| attribute.oid == COMMON_NAME_OID)
        .and_then(attribute_string);
    let expires_at = SystemTime::UNIX_EPOCH
        + certificate
            .tbs_certificate
            .validity
            .not_after
            .to_unix_duration();
    Some(ParsedCertificate {
        common_name,
        expires_at,
        der: certificate.to_der().ok()?,
    })
}

/// The LDAP attribute names this service recognizes, as `(the name folded to lower case,
/// the one spelling this service reports it under)`.
///
/// LDAP attribute names are case-insensitive, so a request may write one in any casing;
/// the directory, however, holds each attribute of an object under the single display
/// spelling of its name and reports the object's attributes under that spelling. So a
/// recognized name is canonicalized through this table when it is applied, and the
/// response describes the object rather than repeating the request's spelling.
///
/// The entries are the attributes of an Active Directory computer object, named as the
/// Directory Service APIs name them (the SAM name is reported as
/// [`ATTRIBUTE_WINDOWS_SAM_NAME`], the way this service spells it, rather than as the raw
/// LDAP `sAMAccountName`). A name this table does not list is one this service does not
/// recognize: it is kept exactly as the request spelled it, since there is no other
/// spelling to report it under.
pub const CANONICAL_LDAP_ATTRIBUTE_NAMES: &[(&str, &str)] = &[
    ("description", "Description"),
    ("displayname", "DisplayName"),
    ("distinguishedname", ATTRIBUTE_DISTINGUISHED_NAME),
    ("dnshostname", "DnsHostName"),
    ("location", "Location"),
    ("managedby", "ManagedBy"),
    ("name", "Name"),
    ("objectclass", "ObjectClass"),
    ("objectsid", "ObjectSid"),
    ("operatingsystem", "OperatingSystem"),
    ("operatingsystemservicepack", "OperatingSystemServicePack"),
    ("operatingsystemversion", "OperatingSystemVersion"),
    ("samaccountname", ATTRIBUTE_WINDOWS_SAM_NAME),
    ("serviceprincipalname", "ServicePrincipalName"),
    ("userprincipalname", "UserPrincipalName"),
    ("whenchanged", "WhenChanged"),
    ("whencreated", "WhenCreated"),
];

/// The name this service reports a machine account's SAM (pre-Windows 2000) account name
/// under. See [`windows_sam_name`], which is what derives its value.
pub const ATTRIBUTE_WINDOWS_SAM_NAME: &str = "WindowsSamName";

/// The name this service reports a directory object's distinguished name under. See
/// [`Directory::computer_distinguished_name`], which is what derives its value for a
/// computer account.
pub const ATTRIBUTE_DISTINGUISHED_NAME: &str = "DistinguishedName";

/// The attributes a domain derives for a computer account itself, rather than taking them
/// from whoever created it: its SAM account name, which follows from the account name,
/// and its distinguished name, which follows from where the account was placed. They are
/// reported from the stored account ([`ComputerRecord::to_computer`]), so a create that
/// lists one of them under any casing does not get to set it.
pub const DERIVED_COMPUTER_ATTRIBUTES: &[&str] =
    &[ATTRIBUTE_WINDOWS_SAM_NAME, ATTRIBUTE_DISTINGUISHED_NAME];

/// `name` in the one spelling this service reports it under: its entry in
/// [`CANONICAL_LDAP_ATTRIBUTE_NAMES`] when it names a recognized LDAP attribute (in any
/// casing, since LDAP attribute names are case-insensitive), and `name` unchanged when it
/// does not.
pub fn canonical_ldap_attribute_name(name: &str) -> String {
    let folded = name.to_lowercase();
    CANONICAL_LDAP_ATTRIBUTE_NAMES
        .iter()
        .find(|(ldap_name, _)| *ldap_name == folded)
        .map_or_else(|| name.to_string(), |(_, reported)| (*reported).to_string())
}

/// The attributes an object carries once `applied` has been applied to it: every
/// recognized name canonicalized through [`canonical_ldap_attribute_name`], in the order
/// they were applied, and the ones the domain derives for the object
/// ([`DERIVED_COMPUTER_ATTRIBUTES`]) dropped, because those describe the object itself and
/// no caller sets them.
pub fn applied_attributes(applied: Vec<Attribute>) -> Vec<Attribute> {
    applied
        .into_iter()
        .filter_map(|attribute| {
            // An entry carrying no name at all names no LDAP attribute: there is nothing
            // to canonicalize and nothing it can collide with, so it is kept as sent.
            let Some(name) = attribute.name.as_deref() else {
                return Some(attribute);
            };
            let name = canonical_ldap_attribute_name(name);
            if DERIVED_COMPUTER_ATTRIBUTES.contains(&name.as_str()) {
                return None;
            }
            Some(Attribute {
                name: Some(name),
                value: attribute.value,
            })
        })
        .collect()
}

/// The SAM account name of the machine account `computer_name`: the account name with the
/// `$` suffix every Active Directory machine account's SAM name carries, which is what
/// separates the machine accounts of a domain from its user accounts.
pub fn windows_sam_name(computer_name: &str) -> String {
    format!("{computer_name}$")
}

/// One attribute of a directory object, as the modelled `Attribute` reports it.
fn reported_attribute(name: &str, value: String) -> Attribute {
    Attribute {
        name: Some(name.to_string()),
        value: Some(value),
    }
}

/// One stored computer account: the Active Directory computer object `CreateComputer`
/// created in a directory.
///
/// Named `ComputerRecord` rather than `Computer` so it does not collide with the modelled
/// `Computer` shape [`Self::to_computer`] renders it into.
///
/// The one-time join `Password` the create was called with is deliberately NOT a field:
/// the model marks it `@sensitive`, no operation reports it back, and state this emulator
/// does not need is state it must not hold \u2014 the same reason the directory record holds
/// no administrator password.
#[derive(Debug, Clone)]
pub struct ComputerRecord {
    /// The account's SID, as the modelled `Computer$ComputerId` reports it: this
    /// directory's [`Directory::domain_sid`] plus the RID the directory handed out. See
    /// [`FIRST_COMPUTER_RID`].
    pub computer_id: String,
    /// The account name as the request spelled it. The lookup key folds it to lower case
    /// (see [`Directory::computers`]); this is what is reported back.
    pub computer_name: String,
    /// The organizational unit the account was placed in, when the create named one.
    /// `None` means the directory's own default computers container, which is what
    /// `OrganizationalUnitDistinguishedName`'s documentation describes as the placement
    /// when the member is absent.
    pub organizational_unit_distinguished_name: Option<String>,
    /// The LDAP attributes the create applied to the account, in the order the request
    /// listed them and under the names the directory holds them by
    /// ([`applied_attributes`]). The attributes the domain derives for the account itself
    /// are NOT among them: they follow from the fields beside this one, and
    /// [`Self::to_computer`] is what reports them.
    pub attributes: Vec<Attribute>,
    /// The account's own distinguished name: its `CN=` component under the container it
    /// was placed in, built once at creation through
    /// [`Directory::computer_distinguished_name`] so every read reports the same one.
    pub distinguished_name: String,
    /// When the account was created.
    pub created_at: SystemTime,
}

impl ComputerRecord {
    /// This account as the modelled `Computer`.
    ///
    /// `ComputerAttributes` is documented as "the LDAP attributes that belong to the
    /// computer account", so it describes the ACCOUNT: the attributes the domain derives
    /// for it — its SAM account name and its distinguished name — are reported alongside
    /// the ones a create applied, which is why an account nothing was applied to still
    /// reports a list.
    pub fn to_computer(&self) -> Computer {
        let mut attributes = Vec::with_capacity(self.attributes.len() + 2);
        attributes.push(reported_attribute(
            ATTRIBUTE_WINDOWS_SAM_NAME,
            windows_sam_name(&self.computer_name),
        ));
        attributes.extend(self.attributes.iter().cloned());
        attributes.push(reported_attribute(
            ATTRIBUTE_DISTINGUISHED_NAME,
            self.distinguished_name.clone(),
        ));
        Computer {
            computer_id: Some(self.computer_id.clone()),
            computer_name: Some(self.computer_name.clone()),
            computer_attributes: Some(attributes),
        }
    }
}

/// Separates the creation rank a cursor resumes after from the filter it was issued
/// under, in every paginated operation of this service. Not a character a `DirectoryId`,
/// a `SnapshotId` or a tag key the listings filter on can contain, so the two halves are
/// unambiguous — and it lives here, beside [`encode_cursor`], because the listings must
/// agree on it for a token to mean one thing.
pub const CURSOR_SEPARATOR: char = '|';

/// Separates the parts of one listing's filter inside a cursor. Not a character a
/// `DirectoryId`, a `SnapshotId` or a tag key can contain, so the parts stay
/// unambiguous.
const CURSOR_FILTER_SEPARATOR: char = '/';

/// How a listing records the filter its page was issued under, inside the cursor: its
/// parts joined by [`CURSOR_FILTER_SEPARATOR`], each part being that member as the
/// request listed it (or the empty string when the request omitted it).
///
/// One encoding for every paginated operation, so the filter half of a token cannot
/// drift between listings: a later page asked for under a different filter decodes to a
/// different string and is refused with [`INVALID_NEXT_TOKEN`].
pub fn cursor_filter(parts: &[&str]) -> String {
    parts.join(&CURSOR_FILTER_SEPARATOR.to_string())
}

/// One stored association between a directory and an Amazon SNS topic: what
/// `RegisterEventTopic` persisted and `DescribeEventTopics` reports.
#[derive(Debug, Clone)]
pub struct EventTopicRegistration {
    pub topic_name: String,
    /// The topic's ARN, built once at registration through [`sns_topic_arn`] so every
    /// later read reports the same one.
    pub topic_arn: String,
    /// When the association was made — the `CreatedDateTime` member's "the date and
    /// time of when you associated your directory with the Amazon SNS topic".
    pub created_at: SystemTime,
    /// One of the modelled `TopicStatus` values; [`TOPIC_STATUS_REGISTERED`] here.
    pub status: String,
}

impl EventTopicRegistration {
    /// This association as the modelled `EventTopic`, reported for `directory_id`.
    ///
    /// The one place a stored registration is rendered, so `DescribeEventTopics`
    /// reports exactly what was persisted.
    pub fn to_event_topic(&self, directory_id: &str) -> EventTopic {
        EventTopic {
            directory_id: Some(directory_id.to_string()),
            topic_name: Some(self.topic_name.clone()),
            topic_arn: Some(self.topic_arn.clone()),
            created_date_time: Some(as_timestamp(self.created_at)),
            status: Some(self.status.clone()),
        }
    }
}

/// One stored log subscription of a directory: what `CreateLogSubscription` persisted,
/// `ListLogSubscriptions` reports and `DeleteLogSubscription` removes.
///
/// A directory forwards its domain controller security logs to ONE log group at a time —
/// `DeleteLogSubscription` identifies the subscription to remove by `DirectoryId` alone,
/// and `ListLogSubscriptions$DirectoryId`'s documentation calls what a directory id
/// narrows the listing to "the log subscription associated with that directory" — so this
/// record hangs off the directory as an `Option` rather than as an index of many.
#[derive(Debug, Clone)]
pub struct LogSubscriptionRecord {
    /// The CloudWatch log group the directory's logs are forwarded to, as the request
    /// spelled it.
    pub log_group_name: String,
    /// When the subscription was created — the `SubscriptionCreatedDateTime` member's
    /// "the date and time that the log subscription was created".
    pub created_at: SystemTime,
}

impl LogSubscriptionRecord {
    /// This subscription as the modelled `LogSubscription`, reported for `directory_id`.
    ///
    /// The one place a stored subscription is rendered, so every operation that reports
    /// one reports exactly what was persisted.
    pub fn to_log_subscription(&self, directory_id: &str) -> LogSubscription {
        LogSubscription {
            directory_id: Some(directory_id.to_string()),
            log_group_name: Some(self.log_group_name.clone()),
            subscription_created_date_time: Some(as_timestamp(self.created_at)),
        }
    }
}

/// One stored conditional forwarder of a directory: what `CreateConditionalForwarder`
/// persisted and `DescribeConditionalForwarders` reports.
///
/// A forwarder is identified by the remote domain it points to, which is why the
/// per-directory index is keyed by [`Directory::conditional_forwarder_key`] of that name
/// rather than by this record.
#[derive(Debug, Clone)]
pub struct ConditionalForwarderRecord {
    /// The remote domain name as the request spelled it, so the report gives the caller
    /// back its own spelling even though the lookup folds it.
    pub remote_domain_name: String,
    /// The IPv4 addresses of the remote DNS server the forwarder points to, in the order
    /// the request listed them.
    pub dns_ip_addrs: Vec<String>,
    /// The IPv6 addresses of the remote DNS server the forwarder points to, in the order
    /// the request listed them.
    pub dns_ipv6_addrs: Vec<String>,
    /// One of the modelled `ReplicationScope` values; [`REPLICATION_SCOPE_DOMAIN`] here.
    pub replication_scope: String,
}

impl ConditionalForwarderRecord {
    /// This forwarder as the modelled `ConditionalForwarder`.
    ///
    /// The one place a stored forwarder is rendered, so every operation that reports one
    /// reports exactly what was persisted.
    pub fn to_conditional_forwarder(&self) -> ConditionalForwarder {
        ConditionalForwarder {
            remote_domain_name: Some(self.remote_domain_name.clone()),
            dns_ip_addrs: optional_list(&self.dns_ip_addrs),
            dns_ipv6_addrs: optional_list(&self.dns_ipv6_addrs),
            replication_scope: Some(self.replication_scope.clone()),
        }
    }
}

/// One domain controller of a directory: a server this service placed in one of the
/// directory's subnets, as `DescribeDomainControllers` reports it.
///
/// Named `DomainControllerRecord` rather than `DomainController` so it does not collide
/// with the modelled `DomainController` shape [`Self::to_domain_controller`] renders it
/// into.
#[derive(Debug, Clone)]
pub struct DomainControllerRecord {
    /// The id of the shape the model declares, `^dc-[0-9a-f]{10}$`, minted by
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_domain_controller_id`].
    pub domain_controller_id: String,
    /// The subnet of the directory's VPC settings this controller was placed in; `None`
    /// only for a directory stored without VPC settings, which has no subnet to name.
    pub subnet_id: Option<String>,
    /// The address this controller answers DNS at: one out of its own subnet's CIDR block,
    /// counted at placement time ([`Directory::server_address_in`]) from the record the
    /// create resolved against the service that owns subnets. `None` for a controller whose
    /// subnet could not be resolved, which then serves no address rather than an invented
    /// one.
    pub dns_ip_addr: Option<String>,
    /// One of the modelled `DomainControllerStatus` values.
    pub status: String,
    /// When the status transition the placement owes falls due; `None` once there is
    /// none left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    pub launch_time: SystemTime,
    pub status_last_updated: SystemTime,
    /// Placement rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`].
    /// `DescribeDomainControllers` pages over a directory's controllers sorted by this,
    /// and its opaque `NextToken` carries the rank the next page resumes after, so a page
    /// boundary survives a controller leaving the directory between pages.
    pub seq: u64,
}

impl DomainControllerRecord {
    /// This controller as the modelled `DomainController`, reported for the `directory` it
    /// resides in.
    ///
    /// Its network comes from the directory's own placement: the VPC the create named, the
    /// subnet this controller was placed in, that subnet's Availability Zone and the address
    /// counted out of its block — so what `DescribeDomainControllers` reports about a
    /// controller agrees with the `DnsIpAddrs` the directory serves.
    ///
    /// The members this slice has no record behind are left absent rather than invented:
    /// `DnsIpv6Addr` ("the IPv6 address of the domain controller"), which belongs to a
    /// directory placed on an IPv6 block, and `StatusReason` ("a description of the domain
    /// controller state"), which describes a controller whose status needs explaining, which
    /// no status this emulator records does.
    pub fn to_domain_controller(&self, directory: &Directory) -> DomainController {
        DomainController {
            directory_id: Some(directory.directory_id.clone()),
            domain_controller_id: Some(self.domain_controller_id.clone()),
            dns_ip_addr: self.dns_ip_addr.clone(),
            dns_ipv6_addr: None,
            vpc_id: directory.vpc_id().map(str::to_string),
            subnet_id: self.subnet_id.clone(),
            availability_zone: directory
                .subnet_network(self.subnet_id.as_deref())
                .and_then(|network| network.availability_zone.clone()),
            status: Some(self.status.clone()),
            status_reason: None,
            launch_time: Some(as_timestamp(self.launch_time)),
            status_last_updated_date_time: Some(as_timestamp(self.status_last_updated)),
        }
    }
}

/// The administrator account name a Simple AD directory is created with.
///
/// From `CreateDirectoryRequest$Password`'s own documentation: "The password for the
/// directory administrator. The directory creation process creates a directory
/// administrator account with the user name `Administrator` and this password."
pub const SIMPLE_AD_ADMINISTRATOR: &str = "Administrator";

/// The administrator account name an AWS Managed Microsoft AD directory is created with.
///
/// From `CreateMicrosoftADRequest$Password`'s own documentation: "The password for the
/// default administrative user named `Admin`."
pub const MICROSOFT_AD_ADMINISTRATOR: &str = "Admin";

/// The directory types whose user accounts live in THIS service's directories, and whose
/// passwords `ResetUserPassword` can therefore reset.
///
/// From `ResetUserPassword`'s own documentation: "Resets the password for any user in your
/// Managed Microsoft AD or Simple AD directory." An AD Connector holds no accounts of its
/// own — it connects to a self-managed directory, whose accounts that directory keeps — so
/// the call is refused for one with the modelled `UnsupportedOperationException`.
pub const USER_DIRECTORY_TYPES: [&str; 2] = [TYPE_SIMPLE_AD, TYPE_MICROSOFT_AD];

/// The administrator account name a directory of `directory_type` is created with, or
/// `None` for a type that holds no account of its own ([`USER_DIRECTORY_TYPES`]).
pub fn administrator_user_name(directory_type: &str) -> Option<&'static str> {
    match directory_type {
        TYPE_SIMPLE_AD => Some(SIMPLE_AD_ADMINISTRATOR),
        TYPE_MICROSOFT_AD => Some(MICROSOFT_AD_ADMINISTRATOR),
        _ => None,
    }
}

/// The accounts a freshly created directory of `directory_type` holds: the administrator
/// account its create made ([`administrator_user_name`]), and nothing else.
///
/// `administrator_password` is the `Password` that create was called with — the password
/// the administrator account it makes is created WITH — and what is kept of it is a
/// verifier a presented password can be checked against ([`PasswordVerifier`]), never the
/// password. A create that makes no account of its own (an AD Connector, whose accounts
/// live in the self-managed directory it connects to) passes `None` and holds nothing.
pub fn initial_users(
    directory_type: &str,
    administrator_password: Option<&str>,
    now: SystemTime,
) -> BTreeMap<String, UserRecord> {
    let mut users = BTreeMap::new();
    if let Some(administrator) = administrator_user_name(directory_type) {
        users.insert(
            Directory::user_key(administrator),
            UserRecord {
                user_name: administrator.to_string(),
                administrator: true,
                password: administrator_password
                    .map(|password| PasswordVerifier::of(administrator, password)),
                password_last_set: now,
            },
        );
    }
    users
}

/// One user account of a directory: the administrative account the create made, which
/// `ResetUserPassword` resolves and resets.
///
/// The password itself is deliberately NOT a field: `CreateDirectoryRequest$Password` and
/// `ResetUserPasswordRequest$NewPassword` are both `@sensitive`, no operation reports
/// either back, and state this emulator does not need is state it must not hold. What IS
/// held is a [`PasswordVerifier`] — what the account's password was set to can be CHECKED,
/// which is what authenticating a credentialed call needs, and cannot be read back out —
/// and when that password was last set, because that is what a reset changes about the
/// account.
#[derive(Debug, Clone)]
pub struct UserRecord {
    /// The account name, spelled the way the account was created with it — the spelling a
    /// report gives back, even though [`Directory::user_key`] folds the lookup.
    pub user_name: String,
    /// Whether this is the directory's own administrative account.
    pub administrator: bool,
    /// What this account's password was last set TO, as a verifier that admits it and
    /// discloses nothing ([`PasswordVerifier`]); `None` for an account this store holds no
    /// password for at all, which no credential pair can then authenticate as.
    pub password: Option<PasswordVerifier>,
    /// When this account's password was last set: at the create that made the account,
    /// and at every `ResetUserPassword` since.
    pub password_last_set: SystemTime,
}

/// The `DirectoryConfigurationStatus` a setting nothing has changed carries: the `Default`
/// value of the modelled enum (`smithy_query(query="shape",
/// shape="DirectoryConfigurationStatus")` answers `["Requested", "Updating", "Updated",
/// "Failed", "Default"]`), the status of a setting still at the value the directory was
/// created with.
pub const DIRECTORY_CONFIGURATION_STATUS_DEFAULT: &str = "Default";

/// The `DirectoryConfigurationStatus` a setting an update has just been requested for
/// carries: the first value of the modelled enum, the status of a request the directory
/// has accepted and not carried out yet.
pub const DIRECTORY_CONFIGURATION_STATUS_REQUESTED: &str = "Requested";

/// The `DirectoryConfigurationStatus` a setting reaches once the transition its update
/// owes has fallen due: the `Updated` value of the modelled enum, the status of a request
/// the directory has carried out.
pub const DIRECTORY_CONFIGURATION_STATUS_UPDATED: &str = "Updated";

/// The directory types that carry configurable settings for `DescribeSettings` to report.
///
/// The settings this service publishes ([`DIRECTORY_SETTINGS`]) are the security protocol
/// switches of a directory whose domain controllers Directory Service runs, which is AWS
/// Managed Microsoft AD alone: a Simple AD directory and an AD Connector expose none, so
/// the call is refused for them with the modelled `UnsupportedOperationException`, the way
/// `DescribeDomainControllers` and `EnableLDAPS` refuse a type that does not have the
/// feature they report on.
pub const SETTINGS_DIRECTORY_TYPES: [&str; 1] = [TYPE_MICROSOFT_AD];

/// One configurable directory setting: what `DescribeSettings` reports an entry for.
#[derive(Debug, Clone, Copy)]
pub struct SettingDefinition {
    /// The modelled `SettingEntry$Name`.
    pub name: &'static str,
    /// The modelled `SettingEntry$Type` — "the type, or category, of a directory setting".
    pub setting_type: &'static str,
    /// The modelled `SettingEntry$DataType`, which "is used to define the `AllowedValues`
    /// of a setting".
    pub data_type: &'static str,
    /// The modelled `SettingEntry$AllowedValues`, "the valid range of values for the
    /// directory setting" — the values themselves, which
    /// [`allowed_values_encoding`] is what renders into that member.
    pub allowed_values: &'static [&'static str],
    /// The value the setting carries until `UpdateSettings` changes it.
    pub default_value: &'static str,
}

/// The `SettingEntry$Type` of the settings below: "Similar settings have the same type.
/// For example, `Protocol`, `Cipher`, or `Certificate-Based Authentication`" — the
/// member's own documentation, whose first example is the category a protocol switch
/// falls in.
const SETTING_TYPE_PROTOCOL: &str = "Protocol";

/// The `SettingEntry$DataType` of a switch: "The data type of a directory setting. This is
/// used to define the `AllowedValues` of a setting. For example a data type can be
/// `Boolean`, `DurationInSeconds`, or `Enum`" — the member's own documentation.
const SETTING_DATA_TYPE_BOOLEAN: &str = "Boolean";

/// The two values a switch admits. Both spellings satisfy the modelled
/// `DirectoryConfigurationSettingValue` pattern `^[a-zA-Z0-9_]*$`, and `Setting$Value`'s
/// own documentation names them: "for `TLS_1_0`, the valid values are: `Enable` and
/// `Disable`".
const SETTING_VALUES_ENABLE_DISABLE: [&str; 2] = [SETTING_VALUE_ENABLE, "Disable"];

/// The value a protocol switch of a freshly created directory carries: the protocol is in
/// service until an administrator turns it off.
const SETTING_VALUE_ENABLE: &str = "Enable";

/// The configurable settings a Managed Microsoft AD directory of this emulator carries.
///
/// One entry per directory security switch this service reports, each as the model
/// describes one: `SettingEntry$Name`'s own documentation gives
/// `TLS_1_0` as the name of a directory setting, `SettingEntry$Type`'s gives `Protocol` as
/// the category such a switch falls in, and `SettingEntry$DataType`'s gives `Boolean` as
/// the data type that defines its `AllowedValues`.
///
/// The two names here are the legacy TLS protocol versions AWS publishes as directory
/// security settings an administrator turns off to harden the directory — each in service
/// on a fresh directory, each a `Boolean` switch of the `Protocol` category, and each
/// independently settable. They are what this emulator publishes, NOT the whole of AWS's
/// published set: every other switch AWS documents is a name no recording of this corpus
/// shows this service reporting, so the catalogue grows as one confirms another rather than
/// carrying a name a caller could not use. Recorded behaviour does require MORE THAN ONE
/// entry: the settings units of this service's behavioural corpus read `DescribeSettings`
/// on an Active Managed Microsoft AD, pick one entry to update, and then assert the entries
/// they did NOT name are untouched — an assertion a one-entry catalogue leaves without a
/// subject. `UpdateSettings` is what requests another value for one of them, and a setting
/// name outside this catalogue is one this service does not support, which is what it
/// refuses.
pub const DIRECTORY_SETTINGS: [SettingDefinition; 2] = [
    SettingDefinition {
        name: "TLS_1_0",
        setting_type: SETTING_TYPE_PROTOCOL,
        data_type: SETTING_DATA_TYPE_BOOLEAN,
        allowed_values: &SETTING_VALUES_ENABLE_DISABLE,
        default_value: SETTING_VALUE_ENABLE,
    },
    SettingDefinition {
        name: "TLS_1_1",
        setting_type: SETTING_TYPE_PROTOCOL,
        data_type: SETTING_DATA_TYPE_BOOLEAN,
        allowed_values: &SETTING_VALUES_ENABLE_DISABLE,
        default_value: SETTING_VALUE_ENABLE,
    },
];

/// The `AllowedValues` encoding of the values a setting admits: a JSON array of strings.
///
/// `SettingEntry$AllowedValues` is a single string member ("the valid range of values for
/// the directory setting"), and the range is reported in it as a JSON-encoded array — so a
/// client enumerates the legal values of a setting by decoding the member, rather than by
/// guessing a separator. THE one place that encoding is produced, so what `DescribeSettings`
/// reports and what `UpdateSettings` accepts are the same list. Rendered through the JSON
/// value's own `Display`, which cannot fail for a list of strings, so the member never
/// degrades into something a client's decoder would reject.
pub fn allowed_values_encoding(values: &[String]) -> String {
    ::serde_json::Value::from(values.to_vec()).to_string()
}

/// The `UnsupportedOperationException` message for a directory whose TYPE carries no
/// configurable settings at all.
///
/// Shared by the settings operations (`DescribeSettings`, `UpdateSettings`), so reading and
/// writing a directory that has no settings state the same thing about the same failure:
/// the id the request asked for, the type that directory is of, and the types that DO carry
/// settings ([`SETTINGS_DIRECTORY_TYPES`]), which is what tells a caller the feature belongs
/// to another directory type rather than that something is missing. [`refusal`] appends the
/// request id, once.
pub fn settings_unsupported_for_type(directory_id: &str, directory_type: &str) -> String {
    format!(
        "DirectoryId {directory_id} names a {directory_type} directory; configurable \
         settings are supported for {supported} directories only",
        supported = SETTINGS_DIRECTORY_TYPES.join(", ")
    )
}

/// The `UnsupportedSettingsException` message for a setting name this service publishes no
/// setting under.
///
/// The failure Directory Service states, with the name the request sent as its only
/// variable part: it is what tells a caller which of the settings it submitted to drop,
/// and the names that ARE supported are what `DescribeSettings` reports. Restated behind
/// the inner exception that produced it ([`restating_its_inner_exception`]). [`refusal`]
/// appends the request id, once.
pub fn unsupported_setting(name: &str) -> String {
    restating_its_inner_exception(
        &format!("Setting name: {name} is not currently supported"),
        UNSUPPORTED_SETTING_EXCEPTION,
    )
}

/// The exception class Directory Service names as the inner one of that refusal — the
/// error shape's own name in the SINGULAR, which is how the recorded message spells it.
const UNSUPPORTED_SETTING_EXCEPTION: &str = "UnsupportedSettingException";

/// The `InvalidParameterException` message for a value outside the values a setting admits.
///
/// Names the setting and the value the request sent — the two halves of the pair that
/// cannot be stored — and nothing about which values WOULD be admitted, since that is what
/// `SettingEntry$AllowedValues` reports for the setting. Restated behind the inner
/// exception that produced it ([`restating_its_inner_exception`]), which for this family is
/// the refusal's own class. [`refusal`] appends the request id, once.
pub fn invalid_setting_value(name: &str, value: &str) -> String {
    restating_its_inner_exception(
        &format!("Invalid setting value found for Setting name: {name} and value: {value}"),
        INVALID_PARAMETER_EXCEPTION,
    )
}

/// The exception class Directory Service names as the inner one of that refusal: the
/// modelled error the call is refused with.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// A settings diagnostic as Directory Service states it: the clause, then the SAME clause
/// again behind the name of the exception class that raised it.
///
/// The settings family words its refusals that way — the caller reads the failure once as
/// the service states it and once as the component that produced it did — and both
/// occurrences are the same sentence, so the two cannot disagree. The envelope's request-id
/// suffix follows, once ([`refusal`]).
///
/// The form is the RECORDED one, from this service's behavioural corpus (the settings units
/// that drive `UpdateSettings` with a name no catalogue publishes and with a value outside a
/// setting's allowed values), which is also where the inner class's SINGULAR spelling comes
/// from. It is the same shape [`refusal_restating_its_request_id`] documents for the tag
/// family: a recorded message that states its own diagnostic twice. Should a later recording
/// show a single-clause form for this family, this helper and both callers lose the
/// restatement.
fn restating_its_inner_exception(stated: &str, exception: &str) -> String {
    format!("{stated} with Inner Exception: {exception}: {stated}")
}

/// The settings a freshly created directory of `directory_type` carries: every entry of
/// [`DIRECTORY_SETTINGS`] at its default value, for a type that has settings at all
/// ([`SETTINGS_DIRECTORY_TYPES`]).
pub fn initial_settings(
    directory_type: &str,
    now: SystemTime,
) -> BTreeMap<String, DirectorySettingRecord> {
    if !SETTINGS_DIRECTORY_TYPES.contains(&directory_type) {
        return BTreeMap::new();
    }
    DIRECTORY_SETTINGS
        .iter()
        .map(|definition| {
            (
                definition.name.to_string(),
                DirectorySettingRecord {
                    name: definition.name.to_string(),
                    setting_type: definition.setting_type.to_string(),
                    data_type: definition.data_type.to_string(),
                    allowed_values: definition
                        .allowed_values
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                    applied_value: definition.default_value.to_string(),
                    requested_value: definition.default_value.to_string(),
                    status: DIRECTORY_CONFIGURATION_STATUS_DEFAULT.to_string(),
                    status_due_at: None,
                    last_updated: now,
                    last_requested: None,
                },
            )
        })
        .collect()
}

/// One Region a directory is replicated to beside its primary one: what `AddRegion`
/// persisted and `DescribeRegions` reports.
#[derive(Debug, Clone)]
pub struct RegionReplication {
    /// The Region's name, as the request spelled it.
    pub region_name: String,
    /// The VPC the directory's servers in this Region are placed in, from the
    /// `VPCSettings` the add named.
    pub vpc_settings: Option<DirectoryVpcSettingsDescription>,
    /// How many domain controllers this Region holds: `AddRegion` "adds two domain
    /// controllers in the specified Region", the modelled minimum.
    pub desired_number_of_domain_controllers: Option<i32>,
    /// One of the modelled `DirectoryStage` values — "the status of the replication
    /// process for the specified Region".
    pub status: String,
    /// When the status transition the add owes falls due; `None` once there is none left
    /// to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// "Specifies when the Region replication began."
    pub launch_time: SystemTime,
    /// "The date and time that the Region status was last updated."
    pub status_last_updated: SystemTime,
}

/// One stored directory setting: what the create persisted and `DescribeSettings` reports.
#[derive(Debug, Clone)]
pub struct DirectorySettingRecord {
    /// The modelled `SettingEntry$Name` this record is the setting for.
    pub name: String,
    /// The modelled `SettingEntry$Type`.
    pub setting_type: String,
    /// The modelled `SettingEntry$DataType`.
    pub data_type: String,
    /// The modelled `SettingEntry$AllowedValues`: the values this setting admits, which
    /// [`allowed_values_encoding`] renders into that member.
    pub allowed_values: Vec<String>,
    /// "The value of the directory setting that is applied to the directory."
    pub applied_value: String,
    /// "The value that was last requested for the directory setting": the applied value
    /// until `UpdateSettings` requests another one for this setting.
    pub requested_value: String,
    /// One of the modelled `DirectoryConfigurationStatus` values:
    /// [`DIRECTORY_CONFIGURATION_STATUS_DEFAULT`] until `UpdateSettings` requests another
    /// value for this setting, then [`DIRECTORY_CONFIGURATION_STATUS_REQUESTED`] until the
    /// transition that update owes falls due and
    /// [`DIRECTORY_CONFIGURATION_STATUS_UPDATED`] after it.
    pub status: String,
    /// When the transition an update owes falls due; `None` once there is none left to
    /// make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// "The date and time when the directory setting was last updated."
    pub last_updated: SystemTime,
    /// "The date and time when the request to update a directory setting was last
    /// submitted" — `None` for a setting no update was ever requested for.
    pub last_requested: Option<SystemTime>,
}

impl DirectorySettingRecord {
    /// This setting as the modelled `SettingEntry`.
    ///
    /// The one place a stored setting is rendered, so every operation that reports one
    /// reports exactly what was persisted. `RequestedValue` is the applied value while the
    /// setting rests at its default — nothing else has been requested for it — and
    /// `LastRequestedDateTime` stays absent for the same reason.
    /// `RequestDetailedStatus` stays absent because it reports the status of the request
    /// "in each region" a setting is deployed in, which the directory's own Region
    /// replications are not, and `RequestStatusMessage` because no update of this
    /// emulator fails.
    pub fn to_setting_entry(&self) -> SettingEntry {
        SettingEntry {
            r#type: Some(self.setting_type.clone()),
            name: Some(self.name.clone()),
            allowed_values: Some(allowed_values_encoding(&self.allowed_values)),
            applied_value: Some(self.applied_value.clone()),
            requested_value: Some(self.requested_value.clone()),
            request_status: Some(self.status.clone()),
            request_detailed_status: None,
            request_status_message: None,
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
            last_requested_date_time: self.last_requested.map(as_timestamp),
            data_type: Some(self.data_type.clone()),
        }
    }

    /// Record the update `value` requests for this setting: the value is the one last
    /// REQUESTED and the setting moves to [`DIRECTORY_CONFIGURATION_STATUS_REQUESTED`],
    /// while the value APPLIED to the directory stays as it is until the transition the
    /// update owes falls due one [`TRANSITION_SPAN`] later — which `advance_clock` is what
    /// materialises, so a caller that reads the setting straight after the update sees the
    /// request it made rather than a change nothing has carried out yet.
    pub fn request_value(&mut self, value: &str, now: SystemTime) {
        self.requested_value = value.to_string();
        self.status = DIRECTORY_CONFIGURATION_STATUS_REQUESTED.to_string();
        self.status_due_at = Some(now + TRANSITION_SPAN);
        self.last_requested = Some(now);
    }

    /// Why `value` is not one this setting can carry, or `None` when it is one of
    /// [`Self::allowed_values`] — the same list [`allowed_values_encoding`] reports, so a
    /// client that enumerates the legal values of a setting and sends one of them is
    /// never refused.
    pub fn value_defect(&self, value: &str) -> Option<String> {
        if self.allowed_values.iter().any(|allowed| allowed == value) {
            return None;
        }
        Some(invalid_setting_value(&self.name, value))
    }
}

/// One stored directory: what `CreateDirectory` persisted and `DescribeDirectories`
/// reports.
#[derive(Debug, Clone)]
pub struct Directory {
    pub directory_id: String,
    pub name: String,
    pub short_name: Option<String>,
    pub description: Option<String>,
    /// The `Size` the create echoed, for a directory type whose create takes one
    /// (`CreateDirectory`, `ConnectDirectory`). It stays `None` for a Managed Microsoft
    /// AD directory, whose create takes no `Size` member and which carries an
    /// [`Self::edition`] the size is derived from instead — see [`Self::reported_size`],
    /// the one place a directory's reported size is decided.
    pub size: Option<String>,
    /// Only AWS Managed Microsoft AD has an edition, so this stays `None` for a
    /// Simple AD directory and `CreateMicrosoftAD` sets it.
    pub edition: Option<String>,
    /// One of the modelled `DirectoryType` values: [`TYPE_SIMPLE_AD`] for a directory
    /// `CreateDirectory` persisted, [`TYPE_MICROSOFT_AD`] for one `CreateMicrosoftAD`
    /// persisted.
    pub directory_type: String,
    /// One of the modelled `DirectoryStage` values.
    pub stage: String,
    /// When the stage transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub stage_due_at: Option<SystemTime>,
    pub launch_time: SystemTime,
    pub stage_last_updated: SystemTime,
    /// The alias `CreateAlias` assigned, if any. `None` means the directory id stands
    /// in for it, as the `Alias` member's documentation describes.
    pub alias: Option<String>,
    pub vpc_settings: Option<DirectoryVpcSettingsDescription>,
    /// The connection settings `ConnectDirectory` persisted for an AD Connector — the
    /// VPC its connector servers are placed in and the service account it reaches the
    /// self-managed directory with. `None` for every other directory type, which carries
    /// [`Self::vpc_settings`] alone.
    pub connect_settings: Option<DirectoryConnectSettingsDescription>,
    /// The network of each subnet this directory's servers are placed in, keyed by subnet
    /// id, as Amazon EC2 — the service that owns subnets — reported it to the create (see
    /// [`directory_network`]). The blocks the directory's addresses are counted out of
    /// ([`Self::server_addresses`]) and the zones it reports; a subnet the owner of subnets
    /// could not be asked about is simply absent, and its server then holds no address.
    pub subnet_networks: BTreeMap<String, SubnetNetwork>,
    /// The DNS servers of the SELF-MANAGED directory an AD Connector forwards to, as
    /// `ConnectDirectory` was called with them (`DirectoryConnectSettings$CustomerDnsIps`):
    /// what `DirectoryDescription$DnsIpAddrs` reports for a connector, whose DNS is served
    /// by the directory it connects to rather than by any server of this account. Empty for
    /// every other directory type, which serves its own ([`Self::dns_ip_addrs`]).
    pub customer_dns_ips: Vec<String>,
    pub network_type: Option<String>,
    pub sso_enabled: bool,
    /// Reported only for AWS Managed Microsoft AD, so it stays `None` for Simple AD:
    /// `CreateMicrosoftAD` sets it to [`MICROSOFT_AD_DOMAIN_CONTROLLERS`] and
    /// `UpdateNumberOfDomainControllers` is what changes it.
    pub desired_number_of_domain_controllers: Option<i32>,
    /// The domain controllers this service placed in the directory, keyed by
    /// `DomainControllerId`; `DescribeDomainControllers` reports them. Only a directory
    /// of a type in [`DOMAIN_CONTROLLER_DIRECTORY_TYPES`] has any: `CreateMicrosoftAD`
    /// places [`Directory::desired_number_of_domain_controllers`] of them and
    /// `UpdateNumberOfDomainControllers` adds or retires them, so the records
    /// live on the directory record and deleting the directory takes them with it.
    pub domain_controllers: BTreeMap<String, DomainControllerRecord>,
    /// This directory's tags, keyed by tag key: a second `AddTagsToResource` with a key
    /// already present overwrites that key's value.
    pub tags: BTreeMap<String, String>,
    /// The Region this directory was created in: the one `RegionType` `Primary` names,
    /// which only `DeleteDirectory` removes. Read from the create request's sigv4
    /// credential scope, the only Region a request carries.
    pub primary_region: String,
    /// The Regions this directory is replicated to beside [`Self::primary_region`] —
    /// the `Additional` ones — keyed by Region name. `AddRegion` is what adds one, and
    /// `RemoveRegion` is what removes one; `DescribeRegions` reports them beside the
    /// primary one.
    pub additional_regions: BTreeMap<String, RegionReplication>,
    /// The IP address blocks `AddIpRoutes` routed to this directory, keyed by
    /// [`IpRouteRecord::block`].
    pub ip_routes: BTreeMap<String, IpRouteRecord>,
    /// The LDAPS security recorded for this directory, keyed by the modelled
    /// `LDAPSType`; `DescribeLDAPSSettings` reports it.
    pub ldaps_settings: BTreeMap<String, LdapsSetting>,
    /// The client authentication recorded for this directory, keyed by the modelled
    /// `ClientAuthenticationType`; `DescribeClientAuthenticationSettings` reports it.
    pub client_authentication_settings: BTreeMap<String, ClientAuthenticationSetting>,
    /// The certificates `RegisterCertificate` registered with this directory, keyed by
    /// `CertificateId`; `DescribeCertificate` reports one and `ListCertificates` pages
    /// over them all. A certificate is registered
    /// FOR a directory, so the registrations live on the directory record and deleting
    /// the directory takes them with it.
    pub certificates: BTreeMap<String, CertificateRecord>,
    /// The schema extensions `StartSchemaExtension` applied to this directory, keyed by
    /// `SchemaExtensionId`; `ListSchemaExtensions` reports them. An extension is applied
    /// TO one directory — which is why that listing requires a `DirectoryId` — so the
    /// records live on the directory record and deleting the directory takes them with
    /// it.
    pub schema_extensions: BTreeMap<String, SchemaExtensionRecord>,
    /// The Amazon SNS topics `RegisterEventTopic` associated with this directory, keyed
    /// by topic name — a directory publishes to a given topic at most once, and the
    /// associations live on the directory record, so deleting the directory takes its
    /// associations with it.
    pub event_topics: BTreeMap<String, EventTopicRegistration>,
    /// The log subscription `CreateLogSubscription` created for this directory, if any:
    /// a directory forwards its logs to at most one log group, and the subscription lives
    /// on the directory record, so deleting the directory takes it with it.
    /// `DeleteLogSubscription` is what puts this back to `None`.
    pub log_subscription: Option<LogSubscriptionRecord>,
    /// The conditional forwarders `CreateConditionalForwarder` created on this directory,
    /// keyed by [`Directory::conditional_forwarder_key`] — a directory points at a given
    /// remote domain at most once. The forwarders live on the directory record, so
    /// deleting the directory takes them with it, and two directories can each point at
    /// the same remote domain.
    pub conditional_forwarders: BTreeMap<String, ConditionalForwarderRecord>,
    /// The computer accounts `CreateComputer` created in this directory, keyed by the
    /// account name folded to lower case: an Active Directory account name is
    /// case-insensitive, so that fold is what makes the duplicate check agree with the
    /// directory it emulates. The accounts live on the directory record, so deleting the
    /// directory takes them with it, and two directories can each hold an account of the
    /// same name.
    pub computers: BTreeMap<String, ComputerRecord>,
    /// The user accounts this directory holds, keyed by [`Self::user_key`]: the
    /// administrative account the create made ([`initial_users`]), whose password
    /// `ResetUserPassword` resets. A directory type that keeps its accounts elsewhere
    /// ([`USER_DIRECTORY_TYPES`]) holds none.
    pub users: BTreeMap<String, UserRecord>,
    /// This directory's configurable settings, keyed by the modelled
    /// `DirectoryConfigurationSettingName`; `DescribeSettings` reports them and
    /// `UpdateSettings` is what changes one. A directory type that has no settings
    /// ([`SETTINGS_DIRECTORY_TYPES`]) carries none.
    pub settings: BTreeMap<String, DirectorySettingRecord>,
    /// The updates made to this directory's setup, oldest first:
    /// `DescribeUpdateDirectory` reports the ones of the `UpdateType` it is asked about
    /// ([`Directory::update_activities_of`]). The activities live on the directory record,
    /// so deleting the directory takes its history with it.
    pub update_activities: Vec<UpdateActivityRecord>,
    /// The next RID this directory's domain hands to an account created in it, starting
    /// at [`FIRST_COMPUTER_RID`]. A pool rather than `computers.len()`: a RID a domain has
    /// handed out is never handed out again.
    pub next_computer_rid: u32,
    /// Whether this directory's data is reachable through the Directory Service Data
    /// API, as `DescribeDirectoryDataAccess` reports it: one of the modelled
    /// `DataAccessStatus` values, [`DATA_ACCESS_DISABLED`] until
    /// `EnableDirectoryDataAccess` records otherwise and again once
    /// `DisableDirectoryDataAccess` has withdrawn it.
    pub data_access_status: String,
    /// When the data-access status transition an enable or a disable owes falls due;
    /// `None` once there is none left to make. See [`TRANSITION_SPAN`].
    pub data_access_due_at: Option<SystemTime>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`]. A
    /// listing pages over directories sorted by this, and its opaque `NextToken`
    /// carries the rank the next page resumes after, so a page boundary survives a
    /// directory being deleted between pages.
    pub seq: u64,
}

impl Directory {
    /// The `Alias` this directory reports: the alias `CreateAlias` assigned, or the
    /// directory id when none was.
    ///
    /// From the `Alias` member's documentation: "If no alias exists, the alias is the
    /// directory identifier, such as `d-XXXXXXXXXX`."
    pub fn effective_alias(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.directory_id)
    }

    /// The `AccessUrl` this directory reports, derived from [`Self::effective_alias`].
    ///
    /// From the `AccessUrl` member's documentation: "The access URL for the directory,
    /// such as `http://<alias>.awsapps.com`. If no alias exists, `<alias>` is the
    /// directory identifier". The documented example shows a scheme, but the value the
    /// service reports is the HOST alone — the alias in the
    /// [`ACCESS_URL_SUFFIX`] domain and nothing else — which is what a client uses as a
    /// host name, so no scheme and no path are composed into it. THE one place a stored
    /// directory's access URL is rendered, so every operation that reports one reports this.
    pub fn access_url(&self) -> String {
        format!("{alias}{ACCESS_URL_SUFFIX}", alias = self.effective_alias())
    }

    /// The DNS server that answers for this directory, as a diagnostic names it: the DNS
    /// domain the directory serves, which is the zone its own servers are authoritative
    /// for. Stored state, and nothing invented; the ADDRESSES those servers answer at are
    /// [`Self::dns_ip_addrs`].
    pub fn dns_server(&self) -> &str {
        &self.name
    }

    /// The network of the subnet `subnet_id`, as the create resolved it against the service
    /// that owns subnets; `None` for a subnet this directory was not placed in, or one that
    /// could not be resolved.
    pub fn subnet_network(&self, subnet_id: Option<&str>) -> Option<&SubnetNetwork> {
        self.subnet_networks.get(subnet_id?)
    }

    /// The address the `ordinal`-th server this directory places in the subnet `subnet_id`
    /// holds: an address out of THAT subnet's own CIDR block ([`server_address`]).
    ///
    /// `None` when the subnet was not resolved — a directory created where the owner of
    /// subnets could not be asked serves no address rather than an invented one.
    pub fn server_address_in(&self, subnet_id: Option<&str>, ordinal: u32) -> Option<String> {
        let network = self.subnet_network(subnet_id)?;
        server_address(network.cidr_block.as_deref()?, ordinal)
    }

    /// The addresses of the servers this directory places ITSELF: one per subnet of its
    /// settings block, in the order the create named them, each out of that subnet's own
    /// CIDR block.
    ///
    /// `DirectoryVpcSettings$SubnetIds`' own documentation: "Directory Service creates a
    /// directory server and a DNS server in each of these subnets" — so a directory whose
    /// servers this service runs holds one server per chosen subnet, and the first
    /// assignable address of each block is that server's.
    pub fn server_addresses(&self) -> Vec<String> {
        self.subnet_ids()
            .iter()
            .filter_map(|subnet_id| self.server_address_in(Some(subnet_id), 0))
            .collect()
    }

    /// The addresses this directory's DNS servers answer at, as
    /// `DirectoryDescription$DnsIpAddrs` reports them.
    ///
    /// THE one place that member is derived, so every operation that reports a directory
    /// reports the same addresses. From the member's own documentation: "The IP addresses of
    /// the DNS servers for the directory. For a Simple AD or Microsoft AD directory, these
    /// are the IP addresses of the Simple AD or Microsoft AD directory servers. For an AD
    /// Connector directory, these are the IP addresses of self-managed directory to which
    /// the AD Connector is connected." So:
    ///
    /// * a directory whose servers are reported INDIVIDUALLY as domain controllers serves
    ///   DNS at those controllers' own addresses, in placement order — which is why adding
    ///   one with `UpdateNumberOfDomainControllers` adds a DNS address and retiring one
    ///   takes its address away;
    /// * a directory whose servers this service runs but does not report one by one serves
    ///   DNS at [`Self::server_addresses`], one per subnet of its settings block;
    /// * an AD Connector serves no DNS of its own: it answers with the self-managed
    ///   directory's servers, which is what its create named ([`Self::customer_dns_ips`]).
    pub fn dns_ip_addrs(&self) -> Vec<String> {
        if self.directory_type == TYPE_AD_CONNECTOR {
            return self.customer_dns_ips.clone();
        }
        if self.manages_domain_controllers() {
            return self
                .domain_controllers_in_placement_order()
                .into_iter()
                .filter(|controller| controller.status != DOMAIN_CONTROLLER_STATUS_DELETING)
                .filter_map(|controller| controller.dns_ip_addr.clone())
                .collect();
        }
        self.server_addresses()
    }

    /// The `OSVersion` this directory reports, or `None` for a type whose operating system
    /// Directory Service does not name ([`directory_servers::os_version_of`]).
    pub fn os_version(&self) -> Option<&'static str> {
        directory_servers::os_version_of(&self.directory_type)
    }

    /// The Availability Zones this directory is in, as the zones of the subnets its servers
    /// were placed in: `DirectoryVpcSettingsDescription$AvailabilityZones` and
    /// `DirectoryConnectSettingsDescription$AvailabilityZones`.
    ///
    /// Read off the records the owner of subnets answered the create with, in zone order,
    /// each named once — a directory's servers sit in two zones, and that is the list rather
    /// than one entry per subnet.
    pub fn availability_zones(&self) -> Vec<String> {
        let mut zones: Vec<String> = self
            .subnet_ids()
            .iter()
            .filter_map(|subnet_id| {
                self.subnet_network(Some(subnet_id))?
                    .availability_zone
                    .clone()
            })
            .collect();
        zones.sort();
        zones.dedup();
        zones
    }

    /// This directory's associations as the modelled `EventTopics` list, in topic-name
    /// order, narrowed to `wanted` when the request asked for particular topic names.
    pub fn event_topic_list(&self, wanted: Option<&BTreeSet<&String>>) -> Vec<EventTopic> {
        self.event_topics
            .values()
            .filter(|registration| {
                wanted.is_none_or(|names| names.contains(&registration.topic_name))
            })
            .map(|registration| registration.to_event_topic(&self.directory_id))
            .collect()
    }

    /// This directory's schema extensions in the order they were started \u2014 the order
    /// `ListSchemaExtensions` pages over, which the records' creation rank gives, since
    /// they are keyed by id rather than by rank.
    pub fn schema_extensions_in_creation_order(&self) -> Vec<&SchemaExtensionRecord> {
        let mut ordered: Vec<&SchemaExtensionRecord> = self.schema_extensions.values().collect();
        ordered.sort_by_key(|extension| extension.seq);
        ordered
    }

    /// This directory's certificates in the order they were registered — the order
    /// `ListCertificates` pages over, which the records' registration rank gives, since
    /// they are keyed by id rather than by rank.
    pub fn certificates_in_registration_order(&self) -> Vec<&CertificateRecord> {
        let mut ordered: Vec<&CertificateRecord> = self.certificates.values().collect();
        ordered.sort_by_key(|certificate| certificate.seq);
        ordered
    }

    /// Whether this directory holds a certificate that is in service and performs
    /// `certificate_type` — the registrations an operation that needs such a
    /// certificate (`EnableLDAPS`) can rely on.
    ///
    /// A registration still being made ([`CERTIFICATE_STATE_REGISTERING`]) does not
    /// count: it is not in service until the transition the register owes falls due.
    pub fn has_certificate_in_service(&self, certificate_type: &str) -> bool {
        self.certificates.values().any(|certificate| {
            certificate.certificate_type == certificate_type
                && certificate.state == CERTIFICATE_STATE_REGISTERED
        })
    }

    /// Whether this directory's LDAP security rests on the certificates it holds:
    /// whether LDAP secure calls are active for any `LDAPSType` of it, or on their way
    /// there under the transition an activation owes.
    ///
    /// While they are, the registrations that perform
    /// [`CERTIFICATE_TYPE_CLIENT_LDAPS`] are what secures them, which is what
    /// `DeregisterCertificate` refuses to take out from under a live LDAP security
    /// connection.
    pub fn ldaps_security_in_force(&self) -> bool {
        self.ldaps_settings.values().any(|setting| {
            setting.status == LDAPS_STATUS_ENABLING || setting.status == LDAPS_STATUS_ENABLED
        })
    }

    /// Whether a change to this directory's IP routing is still being applied: whether any
    /// block it holds carries one of the TRANSIENT [`IpRouteRecord::status`] values
    /// ([`IP_ROUTE_STATUS_ADDING`], [`IP_ROUTE_STATUS_REMOVING`]) rather than the settled
    /// [`IP_ROUTE_STATUS_ADDED`].
    ///
    /// Routing a block changes the directory itself, and a directory takes one such change
    /// at a time: while this holds, the directory is under an update, which is what the
    /// routing writes refuse a second change with ([`directory_update_in_progress`]).
    /// `advance_clock` is what ends the update, by settling the transition the change owed.
    pub fn ip_route_change_in_flight(&self) -> bool {
        self.ip_routes.values().any(|route| {
            route.status == IP_ROUTE_STATUS_ADDING || route.status == IP_ROUTE_STATUS_REMOVING
        })
    }

    /// Whether this directory's domain controllers are this service's to report at all:
    /// whether its type is one of [`DOMAIN_CONTROLLER_DIRECTORY_TYPES`].
    pub fn manages_domain_controllers(&self) -> bool {
        DOMAIN_CONTROLLER_DIRECTORY_TYPES.contains(&self.directory_type.as_str())
    }

    /// The VPC this directory's servers were placed in, from the settings block its own
    /// type reports the placement as: the VPC settings of a directory this service runs the
    /// servers of, or the connection settings of an AD Connector, whose connector servers
    /// are placed the same way.
    pub fn vpc_id(&self) -> Option<&str> {
        match (self.vpc_settings.as_ref(), self.connect_settings.as_ref()) {
            (Some(settings), _) => settings.vpc_id.as_deref(),
            (None, Some(settings)) => settings.vpc_id.as_deref(),
            (None, None) => None,
        }
    }

    /// The subnets this directory's servers are placed across: the `SubnetIds` the create
    /// named, in the order it named them, read from whichever settings block this
    /// directory's type reports its placement as ([`Self::vpc_id`]).
    pub fn subnet_ids(&self) -> Vec<String> {
        let from_vpc_settings = self
            .vpc_settings
            .as_ref()
            .and_then(|settings| settings.subnet_ids.clone());
        from_vpc_settings
            .or_else(|| {
                self.connect_settings
                    .as_ref()
                    .and_then(|settings| settings.subnet_ids.clone())
            })
            .unwrap_or_default()
    }

    /// This directory's domain controllers in the order they were placed — the order
    /// `DescribeDomainControllers` pages over, which the records' placement rank gives,
    /// since they are keyed by id rather than by rank.
    pub fn domain_controllers_in_placement_order(&self) -> Vec<&DomainControllerRecord> {
        let mut ordered: Vec<&DomainControllerRecord> = self.domain_controllers.values().collect();
        ordered.sort_by_key(|controller| controller.seq);
        ordered
    }

    /// Whether LDAP secure calls can be activated for this directory's type at all:
    /// whether it is one of [`LDAPS_SUPPORTED_DIRECTORY_TYPES`].
    pub fn supports_ldaps(&self) -> bool {
        LDAPS_SUPPORTED_DIRECTORY_TYPES.contains(&self.directory_type.as_str())
    }

    /// The key [`Self::conditional_forwarders`] holds `remote_domain_name` under:
    /// [`domain_key`] of that name, so a lookup agrees with DNS.
    pub fn conditional_forwarder_key(remote_domain_name: &str) -> String {
        domain_key(remote_domain_name)
    }

    /// This directory's conditional forwarders as the modelled `ConditionalForwarders`
    /// list, in remote-domain order, narrowed to `wanted` when the request asked for
    /// particular remote domains. `wanted` holds the domains folded through
    /// [`Self::conditional_forwarder_key`], so a filter matches a stored forwarder the
    /// same way a second create of it would.
    pub fn conditional_forwarder_list(
        &self,
        wanted: Option<&BTreeSet<String>>,
    ) -> Vec<ConditionalForwarder> {
        self.conditional_forwarders
            .iter()
            .filter(|(key, _)| wanted.is_none_or(|keys| keys.contains(*key)))
            .map(|(_, record)| record.to_conditional_forwarder())
            .collect()
    }

    /// This directory's tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Whether this directory exists in `region` at all: its primary Region, or one of
    /// the Regions it is replicated to.
    pub fn exists_in_region(&self, region: &str) -> bool {
        self.primary_region == region || self.additional_regions.contains_key(region)
    }

    /// Whether this directory supports multi-Region replication at all.
    ///
    /// Replicating a directory across Regions is a feature of the Enterprise edition of
    /// AWS Managed Microsoft AD, so a Simple AD directory, an AD Connector and a
    /// Standard-edition Managed Microsoft AD support none of the multi-Region calls. THE
    /// one predicate `AddRegion` and `RemoveRegion` gate on, so both refuse the same
    /// directory, with the same sentence ([`MULTI_REGION_ENTERPRISE_ONLY`]), before
    /// anything about Regions is looked up.
    pub fn replicates_across_regions(&self) -> bool {
        self.edition.as_deref() == Some(DEFAULT_MICROSOFT_AD_EDITION)
            && self.directory_type == TYPE_MICROSOFT_AD
    }

    /// This directory's Regions as the modelled `RegionsDescription` list: its primary
    /// Region first — the one it was created in — then the Regions it is replicated to, in
    /// Region-name order, narrowed to `wanted` when the request asked for one Region.
    ///
    /// THE one place a directory's Regions are rendered, so `DescribeRegions` reports
    /// exactly what the create and the adds persisted. The primary Region's description is
    /// the directory's own: the stage it is in, the VPC settings it was created with and
    /// the controllers it runs, because that Region IS the directory.
    pub fn region_list(&self, wanted: Option<&str>) -> Vec<RegionDescription> {
        let primary = RegionDescription {
            directory_id: Some(self.directory_id.clone()),
            region_name: Some(self.primary_region.clone()),
            region_type: Some(REGION_TYPE_PRIMARY.to_string()),
            status: Some(self.stage.clone()),
            vpc_settings: self.requested_vpc_settings(),
            desired_number_of_domain_controllers: self.desired_number_of_domain_controllers,
            launch_time: Some(as_timestamp(self.launch_time)),
            status_last_updated_date_time: Some(as_timestamp(self.stage_last_updated)),
            last_updated_date_time: Some(as_timestamp(self.stage_last_updated)),
        };
        let additional = self
            .additional_regions
            .values()
            .map(|replication| RegionDescription {
                directory_id: Some(self.directory_id.clone()),
                region_name: Some(replication.region_name.clone()),
                region_type: Some(REGION_TYPE_ADDITIONAL.to_string()),
                status: Some(replication.status.clone()),
                vpc_settings: replication
                    .vpc_settings
                    .as_ref()
                    .and_then(requested_vpc_settings_of),
                desired_number_of_domain_controllers: replication
                    .desired_number_of_domain_controllers,
                launch_time: Some(as_timestamp(replication.launch_time)),
                status_last_updated_date_time: Some(as_timestamp(replication.status_last_updated)),
                last_updated_date_time: Some(as_timestamp(replication.status_last_updated)),
            });
        ::std::iter::once(primary)
            .chain(additional)
            .filter(|description| {
                wanted.is_none_or(|region| description.region_name.as_deref() == Some(region))
            })
            .collect()
    }

    /// This directory's own placement as the modelled `DirectoryVpcSettings` a
    /// `RegionDescription` carries — the VPC and subnets the create named, which is the
    /// request shape rather than the description shape `DescribeDirectories` reports.
    fn requested_vpc_settings(&self) -> Option<DirectoryVpcSettings> {
        self.vpc_settings
            .as_ref()
            .and_then(requested_vpc_settings_of)
    }

    /// This directory's domain SID: [`DOMAIN_SID_PREFIX`] plus the three sub-authorities
    /// that identify one Active Directory domain.
    ///
    /// Every account of a domain carries the domain's SID, so the three sub-authorities
    /// are derived from the directory's own id rather than minted: they are the same for
    /// every account of this directory, stable for the life of the record, and different
    /// for another directory, because the derivation below is injective in the id's forty
    /// bits (each sub-authority is a bijection of them modulo its own odd multiplier) and
    /// the ids themselves never repeat
    /// ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_directory_id`]).
    pub fn domain_sid(&self) -> String {
        // The id's ten hex digits, the width the modelled `DirectoryId` pattern
        // `^d-[0-9a-f]{10}$` declares. A record is only ever stored under a minted id, so
        // the fallback is unreachable and merely keeps this total.
        let seed = self
            .directory_id
            .strip_prefix("d-")
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
            .unwrap_or_default();
        // A sub-authority is a 32-bit number, and never zero: a domain SID's
        // sub-authorities are non-zero in every SID Windows renders.
        let sub_authority = |multiplier: u64, offset: u64| -> u32 {
            let span = u64::from(u32::MAX);
            (seed.wrapping_mul(multiplier).wrapping_add(offset) % span + 1) as u32
        };
        format!(
            "{DOMAIN_SID_PREFIX}-{first}-{second}-{third}",
            first = sub_authority(2_654_435_761, 1_779_033_703),
            second = sub_authority(2_246_822_519, 3_144_134_277),
            third = sub_authority(3_266_489_917, 1_013_904_242)
        )
    }

    /// The domain name components a distinguished name in this directory's domain ends
    /// with: the DNS labels of the directory's own `Name`, as `dc=` relative
    /// distinguished names.
    ///
    /// The directory's `Name` is its fully qualified domain name (`CreateDirectory`'s
    /// documentation: "The fully qualified name for the directory, such as
    /// `corp.example.com`"), and a DN in that domain ends with one `dc=` component per
    /// label, so this is what an `OrganizationalUnitDistinguishedName` is checked against.
    /// Folded to lower case, because a DN's attribute values are compared
    /// case-insensitively.
    pub fn domain_name_labels(&self) -> Vec<String> {
        self.name
            .split('.')
            .filter(|label| !label.is_empty())
            .map(|label| label.to_lowercase())
            .collect()
    }

    /// The key [`Self::computers`] holds `computer_name` under: the name folded to lower
    /// case, so a lookup agrees with Active Directory's case-insensitive account names.
    pub fn computer_key(computer_name: &str) -> String {
        computer_name.to_lowercase()
    }

    /// The key [`Self::users`] holds `user_name` under: the name folded to lower case,
    /// the same fold [`Self::computer_key`] performs and for the same reason — an Active
    /// Directory account name is compared case-insensitively, so `admin` and `Admin` name
    /// one account.
    pub fn user_key(user_name: &str) -> String {
        user_name.to_lowercase()
    }

    /// The account `user_name` names in this directory, if it holds one.
    pub fn user(&self, user_name: &str) -> Option<&UserRecord> {
        self.users.get(&Self::user_key(user_name))
    }

    /// Whether this directory's type holds user accounts of its own
    /// ([`USER_DIRECTORY_TYPES`]) — the types `ResetUserPassword` serves.
    pub fn holds_users(&self) -> bool {
        USER_DIRECTORY_TYPES.contains(&self.directory_type.as_str())
    }

    /// Whether this directory's type carries configurable settings
    /// ([`SETTINGS_DIRECTORY_TYPES`]) — the types `DescribeSettings` serves.
    pub fn has_settings(&self) -> bool {
        SETTINGS_DIRECTORY_TYPES.contains(&self.directory_type.as_str())
    }

    /// This directory's settings in name order — the order `DescribeSettings` reports
    /// them in.
    pub fn settings_in_name_order(&self) -> Vec<&DirectorySettingRecord> {
        self.settings.values().collect()
    }

    /// The `Size` this directory reports: "The directory size", one of the two values the
    /// modelled `DirectorySize` enum declares.
    ///
    /// THE one place a directory's size is decided, so every operation that reports a
    /// directory reports the same one. A directory whose create takes a `Size` reports the
    /// value it was created with; a Managed Microsoft AD's create takes none, so its size
    /// is derived from the edition it runs ([`microsoft_ad_size`]) rather than left absent
    /// — Directory Service reports a size for every directory it runs.
    pub fn reported_size(&self) -> Option<String> {
        match self.edition.as_deref() {
            Some(edition) => Some(microsoft_ad_size(edition).to_string()),
            None => self.size.clone(),
        }
    }

    /// This directory's domain as the trailing components of a distinguished name in it:
    /// one `DC=<label>` per label of the directory's own `Name`, in order and spelled the
    /// way the create spelled them.
    ///
    /// [`Self::domain_name_labels`] is the folded form a requested name is COMPARED
    /// through; this is the form a name is RENDERED in.
    pub fn domain_distinguished_name(&self) -> String {
        self.name
            .split('.')
            .filter(|label| !label.is_empty())
            .map(|label| format!("DC={label}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The container a computer account created in this directory is placed in when the
    /// create names no organizational unit.
    ///
    /// A directory that carries a NetBIOS short name keeps the objects it holds in an
    /// organizational unit named for it, with the computer accounts in that unit's own
    /// `Computers` unit; one that carries none has only the domain's built-in `Computers`
    /// container to place them in.
    pub fn default_computers_container(&self) -> String {
        let domain = self.domain_distinguished_name();
        match self.short_name.as_deref() {
            Some(short_name) => format!("OU=Computers,OU={short_name},{domain}"),
            None => format!("CN=Computers,{domain}"),
        }
    }

    /// The distinguished name the computer account `computer_name` carries in this
    /// directory: its own `CN=` component under the organizational unit the create named,
    /// or under [`Self::default_computers_container`] when it named none.
    pub fn computer_distinguished_name(
        &self,
        computer_name: &str,
        organizational_unit: Option<&str>,
    ) -> String {
        let container = organizational_unit
            .map_or_else(|| self.default_computers_container(), str::to_string);
        format!("CN={computer_name},{container}")
    }

    /// Hand out the next computer account SID of this directory's domain: its
    /// [`Self::domain_sid`] plus the next RID from the pool.
    ///
    /// MUTATING: the RID is consumed, so no two accounts of a directory can carry the
    /// same `ComputerId` even after one of them is gone.
    pub fn mint_computer_sid(&mut self) -> String {
        let rid = self.next_computer_rid.max(FIRST_COMPUTER_RID);
        self.next_computer_rid = rid.saturating_add(1);
        format!("{domain}-{rid}", domain = self.domain_sid())
    }

    /// This directory as the modelled `DirectoryDescription`.
    ///
    /// The members this slice has no record behind — RADIUS, sharing, regions, the owner
    /// description — are left absent rather than invented: the operations that would
    /// populate them are still scaffolded. The placement members are REPORTED rather than
    /// echoed: the DNS addresses ([`Self::dns_ip_addrs`]) and the Availability Zones
    /// ([`Self::availability_zones`]) come from the subnets the create resolved against the
    /// service that owns them, so the directory reports the network it was really placed in.
    pub fn to_description(&self) -> DirectoryDescription {
        DirectoryDescription {
            directory_id: Some(self.directory_id.clone()),
            name: Some(self.name.clone()),
            short_name: self.short_name.clone(),
            size: self.reported_size(),
            edition: self.edition.clone(),
            alias: Some(self.effective_alias().to_string()),
            access_url: Some(self.access_url()),
            description: self.description.clone(),
            dns_ip_addrs: optional_list(&self.dns_ip_addrs()),
            os_version: self.os_version().map(str::to_string),
            stage: Some(self.stage.clone()),
            launch_time: Some(as_timestamp(self.launch_time)),
            stage_last_updated_date_time: Some(as_timestamp(self.stage_last_updated)),
            r#type: Some(self.directory_type.clone()),
            vpc_settings: self.described_vpc_settings(),
            connect_settings: self.described_connect_settings(),
            sso_enabled: self.sso_enabled,
            desired_number_of_domain_controllers: self.desired_number_of_domain_controllers,
            network_type: self.network_type.clone(),
            ..DirectoryDescription::default()
        }
    }

    /// This directory's placement as the modelled `DirectoryVpcSettingsDescription`: the VPC
    /// and subnets the create named, with the Availability Zones read off the subnets it
    /// resolved. `None` for a directory stored without VPC settings, which reports its
    /// placement as connection settings instead.
    fn described_vpc_settings(&self) -> Option<DirectoryVpcSettingsDescription> {
        let settings = self.vpc_settings.as_ref()?;
        Some(DirectoryVpcSettingsDescription {
            availability_zones: optional_list(&self.availability_zones()),
            ..settings.clone()
        })
    }

    /// An AD Connector's placement as the modelled `DirectoryConnectSettingsDescription`:
    /// the VPC, subnets and service account the connect named, with the Availability Zones
    /// of the subnets it resolved and the addresses of the connector servers themselves
    /// (`ConnectIps`, "the IP addresses of the AD Connector servers") — one per chosen
    /// subnet, out of that subnet's own block, placed exactly as a directory server is
    /// ([`Self::server_addresses`]). `None` for every other directory type.
    fn described_connect_settings(&self) -> Option<DirectoryConnectSettingsDescription> {
        let settings = self.connect_settings.as_ref()?;
        Some(DirectoryConnectSettingsDescription {
            availability_zones: optional_list(&self.availability_zones()),
            connect_ips: optional_list(&self.server_addresses()),
            ..settings.clone()
        })
    }
}

/// The `RegionType` of the Region a directory was created in: the `Primary` value of the
/// modelled `RegionType` enum (`smithy_query(query="shape",shape="RegionType")` answers
/// `["Primary", "Additional"]`).
pub const REGION_TYPE_PRIMARY: &str = "Primary";

/// The `RegionType` of a Region a directory is replicated to: the `Additional` value of
/// that same modelled enum.
pub const REGION_TYPE_ADDITIONAL: &str = "Additional";

/// A stored placement as the modelled `DirectoryVpcSettings`: the VPC and the subnets of
/// the description, or `None` when the record carries no VPC.
///
/// A `RegionDescription` reports the REQUEST shape (`DirectoryVpcSettings`, whose two
/// members are required) where `DescribeDirectories` reports the description shape, so
/// this is the one conversion between them.
fn requested_vpc_settings_of(
    settings: &DirectoryVpcSettingsDescription,
) -> Option<DirectoryVpcSettings> {
    Some(DirectoryVpcSettings {
        vpc_id: settings.vpc_id.clone()?,
        subnet_ids: settings.subnet_ids.clone().unwrap_or_default(),
    })
}

/// One stored directory assessment: what `StartADAssessment` persisted and
/// `DescribeADAssessment` / `ListADAssessments` report.
///
/// Named `AssessmentRecord` rather than `Assessment` so it does not collide with the
/// modelled `Assessment` shape [`Self::to_assessment`] renders it into.
#[derive(Debug, Clone)]
pub struct AssessmentRecord {
    pub assessment_id: String,
    /// The directory the assessment was started for, when `StartADAssessment` was
    /// called with a `DirectoryId`. `None` for an assessment started from an
    /// `AssessmentConfiguration`, which names no existing directory.
    pub directory_id: Option<String>,
    pub dns_name: Option<String>,
    /// The self-managed DNS servers the configuration named; empty for an assessment
    /// started from a `DirectoryId`, for which this emulator holds no self-managed DNS
    /// record.
    pub customer_dns_ips: Vec<String>,
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    /// The Systems Manager managed nodes the configuration named, in the order it
    /// listed them: `StartADAssessment`'s documentation requires `InstanceIds` to keep
    /// a one-to-one positional correspondence with `CustomerDnsIps`, so the order is
    /// preserved exactly as given.
    pub self_managed_instance_ids: Vec<String>,
    /// One of the modelled `AssessmentReportType` values;
    /// [`ASSESSMENT_REPORT_TYPE_CUSTOMER`] for a caller-started assessment.
    pub report_type: String,
    /// One of the modelled `AssessmentStatus` values.
    pub status: String,
    pub status_reason: Option<String>,
    pub start_time: SystemTime,
    pub last_update: SystemTime,
    /// When the status transition the start owes falls due; `None` once the assessment
    /// has reached a terminal status. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], the
    /// same counter the directory records are stamped from. `ListADAssessments` pages
    /// over assessments sorted by this, and its opaque `NextToken` carries the rank the
    /// next page resumes after, so a page boundary survives an assessment being deleted
    /// between pages.
    pub seq: u64,
}

impl AssessmentRecord {
    /// This assessment as the modelled `Assessment`.
    ///
    /// The members this emulator has no record behind are left absent rather than
    /// invented: `StatusCode` ("a detailed status code providing additional information
    /// about the assessment state", which AWS does not document a value list for) and
    /// `Version` ("the version of the assessment framework used to evaluate your
    /// self-managed AD environment" — no framework ran, see
    /// [`ASSESSMENT_FAILED_REASON`]).
    pub fn to_assessment(&self) -> Assessment {
        Assessment {
            assessment_id: Some(self.assessment_id.clone()),
            directory_id: self.directory_id.clone(),
            dns_name: self.dns_name.clone(),
            start_time: Some(as_timestamp(self.start_time)),
            last_update_date_time: Some(as_timestamp(self.last_update)),
            status: Some(self.status.clone()),
            status_code: None,
            status_reason: self.status_reason.clone(),
            customer_dns_ips: optional_list(&self.customer_dns_ips),
            vpc_id: self.vpc_id.clone(),
            subnet_ids: optional_list(&self.subnet_ids),
            security_group_ids: optional_list(&self.security_group_ids),
            self_managed_instance_ids: optional_list(&self.self_managed_instance_ids),
            report_type: Some(self.report_type.clone()),
            version: None,
        }
    }

    /// This assessment as the modelled `AssessmentSummary`: the same record "without
    /// detailed validation results", as that shape's own documentation puts it, so every
    /// member it shares with [`Self::to_assessment`] reports the same value.
    pub fn to_summary(&self) -> AssessmentSummary {
        AssessmentSummary {
            assessment_id: Some(self.assessment_id.clone()),
            directory_id: self.directory_id.clone(),
            dns_name: self.dns_name.clone(),
            start_time: Some(as_timestamp(self.start_time)),
            last_update_date_time: Some(as_timestamp(self.last_update)),
            status: Some(self.status.clone()),
            customer_dns_ips: optional_list(&self.customer_dns_ips),
            report_type: Some(self.report_type.clone()),
        }
    }
}

/// One stored snapshot: what `CreateSnapshot` persisted and `DescribeSnapshots`
/// reports.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    /// The directory this snapshot was taken of. A snapshot does not outlive that
    /// directory: `DeleteDirectory` drops the snapshots naming the id it deletes, as
    /// the real service does.
    pub directory_id: String,
    /// The descriptive `Name` the create was called with, if any.
    pub name: Option<String>,
    /// One of the modelled `SnapshotType` values; [`SNAPSHOT_TYPE_MANUAL`] for a
    /// snapshot a caller asked for.
    pub snapshot_type: String,
    /// One of the modelled `SnapshotStatus` values.
    pub status: String,
    /// When the status transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// "The date and time that the snapshot was taken", the `StartTime` member reports.
    pub start_time: SystemTime,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl SnapshotRecord {
    /// This snapshot as the modelled `Snapshot` structure.
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot {
            directory_id: Some(self.directory_id.clone()),
            snapshot_id: Some(self.snapshot_id.clone()),
            r#type: Some(self.snapshot_type.clone()),
            name: self.name.clone(),
            status: Some(self.status.clone()),
            start_time: Some(as_timestamp(self.start_time)),
        }
    }
}

/// One stored schema extension: what `StartSchemaExtension` persisted and
/// `ListSchemaExtensions` reports.
///
/// The `LdifContent` the start was called with is deliberately NOT a field: it is the
/// schema change itself, which this emulator holds no representation of, no operation
/// reports it back, and state this emulator does not need is state it must not hold.
#[derive(Debug, Clone)]
pub struct SchemaExtensionRecord {
    pub schema_extension_id: String,
    /// The `Description` the start was called with, reported back unchanged.
    pub description: String,
    /// One of the modelled `SchemaExtensionStatus` values.
    pub status: String,
    /// "The reason for the `SchemaExtensionStatus`", when there is one to report.
    pub status_reason: Option<String>,
    /// "The date and time that the schema extension started being applied to the
    /// directory", the `StartDateTime` member reports.
    pub start_date_time: SystemTime,
    /// "The date and time that the schema extension was completed": `None` until the
    /// extension reaches a status that has an end.
    pub end_date_time: Option<SystemTime>,
    /// When the status transition the start owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl SchemaExtensionRecord {
    /// Whether this extension is still early enough in its application to be cancelled:
    /// its status is one of [`CANCELLABLE_SCHEMA_EXTENSION_STATUSES`].
    pub fn is_cancellable(&self) -> bool {
        CANCELLABLE_SCHEMA_EXTENSION_STATUSES.contains(&self.status.as_str())
    }

    /// Cancel this extension as of `now`: it enters the transient
    /// [`SCHEMA_EXTENSION_STATUS_CANCEL_IN_PROGRESS`] and owes the clock the move to
    /// [`SCHEMA_EXTENSION_STATUS_CANCELLED`] one [`TRANSITION_SPAN`] later, which is what
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`]
    /// materialises — so a caller listing straight after the cancel still sees the
    /// winding back under way, and the status the start owed is no longer owed.
    pub fn cancel(&mut self, now: SystemTime) {
        self.status = SCHEMA_EXTENSION_STATUS_CANCEL_IN_PROGRESS.to_string();
        self.status_due_at = Some(now + TRANSITION_SPAN);
    }

    /// This schema extension as the modelled `SchemaExtensionInfo` structure.
    pub fn to_info(&self, directory_id: &str) -> SchemaExtensionInfo {
        SchemaExtensionInfo {
            directory_id: Some(directory_id.to_string()),
            schema_extension_id: Some(self.schema_extension_id.clone()),
            description: Some(self.description.clone()),
            schema_extension_status: Some(self.status.clone()),
            schema_extension_status_reason: self.status_reason.clone(),
            start_date_time: Some(as_timestamp(self.start_date_time)),
            end_date_time: self.end_date_time.map(as_timestamp),
        }
    }
}

/// One stored trust relationship: what `CreateTrust` persisted and `DescribeTrusts`
/// reports.
///
/// A trust is a relationship between ONE Managed Microsoft AD directory and ONE external
/// domain, so a directory points at a given remote domain at most once — which is what
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::trust_of`] resolves. The
/// `TrustPassword` the create was called with is deliberately not a field: the model marks
/// it `@sensitive`, no operation reports it back, and state this emulator does not need is
/// state it must not hold.
#[derive(Debug, Clone)]
pub struct TrustRecord {
    pub trust_id: String,
    /// The Managed Microsoft AD directory this is the Amazon Web Services side of. A
    /// trust does not outlive that directory: `DeleteDirectory` drops the trusts naming
    /// the id it deletes.
    pub directory_id: String,
    /// The FQDN of the external domain, as the request spelled it, so the report gives the
    /// caller back its own spelling even though the lookup folds it.
    pub remote_domain_name: String,
    /// One of the modelled `TrustType` values; [`TRUST_TYPE_FOREST`] when the request
    /// named none.
    pub trust_type: String,
    /// One of the modelled `TrustDirection` values, as the request required.
    pub trust_direction: String,
    /// One of the modelled `SelectiveAuth` values; [`SELECTIVE_AUTH_DISABLED`] when the
    /// request named none.
    pub selective_auth: String,
    /// One of the modelled `TrustState` values.
    pub trust_state: String,
    /// The modelled `TrustStateReason`, "the reason for the TrustState": the sentence a
    /// state the relationship did not reach on its own carries, and `None` for a state
    /// that needs no explanation.
    pub trust_state_reason: Option<String>,
    /// When the state transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub trust_state_due_at: Option<SystemTime>,
    /// "The date and time that the trust relationship was created", the `CreatedDateTime`
    /// member reports.
    pub created_at: SystemTime,
    /// "The date and time that the trust relationship was last updated", the
    /// `LastUpdatedDateTime` member reports.
    pub last_updated: SystemTime,
    /// "The date and time that the TrustState was last updated", the
    /// `StateLastUpdatedDateTime` member reports.
    pub state_last_updated: SystemTime,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl TrustRecord {
    /// This trust as the modelled `Trust` structure.
    ///
    /// The one place a stored trust is rendered, so every operation that reports one
    /// reports exactly what was persisted — `TrustStateReason` ("the reason for the
    /// TrustState") included, which is what explains a relationship that did not settle
    /// the way the create asked for.
    pub fn to_trust(&self) -> Trust {
        Trust {
            directory_id: Some(self.directory_id.clone()),
            trust_id: Some(self.trust_id.clone()),
            remote_domain_name: Some(self.remote_domain_name.clone()),
            trust_type: Some(self.trust_type.clone()),
            trust_direction: Some(self.trust_direction.clone()),
            trust_state: Some(self.trust_state.clone()),
            created_date_time: Some(as_timestamp(self.created_at)),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
            state_last_updated_date_time: Some(as_timestamp(self.state_last_updated)),
            trust_state_reason: self.trust_state_reason.clone(),
            selective_auth: Some(self.selective_auth.clone()),
        }
    }
}

/// One stored directory share: what `ShareDirectory` persisted and
/// `DescribeSharedDirectories` reports.
///
/// A share is the pairing of ONE owner directory with ONE consumer account, so a
/// directory is shared with a given account at most once — which is what
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::share_of`] resolves. The
/// shared directory id is the consumer account's handle on the owner's directory, minted
/// from the same id space as a directory's own id, so no two resources of this service
/// ever answer to one `DirectoryId`.
#[derive(Debug, Clone)]
pub struct SharedDirectoryRecord {
    /// "Identifier of the shared directory in the directory consumer account."
    pub shared_directory_id: String,
    /// "Identifier of the directory in the directory owner account." A share does not
    /// outlive that directory: `DeleteDirectory` drops the shares naming the id it
    /// deletes.
    pub owner_directory_id: String,
    /// "Identifier of the directory owner account", the account this emulator presents.
    pub owner_account_id: String,
    /// "Identifier of the directory consumer account that has access to the shared
    /// directory": the `ShareTarget$Id` the request named.
    pub shared_account_id: String,
    /// One of the modelled `ShareMethod` values, as the request required.
    pub share_method: String,
    /// One of the modelled `ShareStatus` values.
    pub share_status: String,
    /// When the status transition an organization share owes falls due; `None` once there
    /// is no transition left to make. See [`TRANSITION_SPAN`].
    pub share_status_due_at: Option<SystemTime>,
    /// The `ShareNotes` the request carried, if any. Held because the modelled
    /// `SharedDirectory` structure reports it back to the consumer administrator, which
    /// is the whole point of the member; it stays in this process's memory, and no
    /// handler of this service writes state to a file or a log.
    pub share_notes: Option<String>,
    /// "The date and time that the shared directory was created."
    pub created_at: SystemTime,
    /// "The date and time that the shared directory was last updated."
    pub last_updated: SystemTime,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`], shared
    /// with the directories so every listing of this service pages over one ordering.
    pub seq: u64,
}

impl SharedDirectoryRecord {
    /// This share as the modelled `SharedDirectory` structure.
    ///
    /// The one place a stored share is rendered, so every operation that reports one
    /// reports exactly what was persisted.
    pub fn to_shared_directory(&self) -> SharedDirectory {
        SharedDirectory {
            owner_account_id: Some(self.owner_account_id.clone()),
            owner_directory_id: Some(self.owner_directory_id.clone()),
            share_method: Some(self.share_method.clone()),
            shared_account_id: Some(self.shared_account_id.clone()),
            shared_directory_id: Some(self.shared_directory_id.clone()),
            share_status: Some(self.share_status.clone()),
            share_notes: self.share_notes.clone(),
            created_date_time: Some(as_timestamp(self.created_at)),
            last_updated_date_time: Some(as_timestamp(self.last_updated)),
        }
    }
}

/// How this service compares two spellings of ONE remote domain name: the name folded to
/// lower case with the trailing root label dropped, so a lookup agrees with DNS, where a
/// domain name is compared case-insensitively and the fully qualified `example.com.` names
/// the same domain as `example.com` (the modelled `RemoteDomainName` pattern
/// `^([a-zA-Z0-9]+[\.-])+([a-zA-Z0-9])+[.]?$` admits both forms and either case).
///
/// One function, so the conditional forwarder index and the trust store cannot disagree
/// about which requests name the same external domain.
pub fn domain_key(remote_domain_name: &str) -> String {
    remote_domain_name.trim_end_matches('.').to_lowercase()
}

/// Whether `candidate` is an Amazon Web Services account id: the twelve decimal digits
/// the modelled `CustomerId` pattern `^(\d{12})$` admits
/// (`smithy_query(query="shape",shape="SharedDirectory")`), which is what
/// `SharedDirectory$SharedAccountId` reports a share's consumer account in.
///
/// One function, so `ShareDirectory` and `UnshareDirectory` cannot disagree about which
/// `Id` of an `ACCOUNT` target names a consumer account.
pub fn is_account_id(candidate: &str) -> bool {
    candidate.len() == 12 && candidate.bytes().all(|byte| byte.is_ascii_digit())
}

/// A stored list as an optional modelled list: absent when this emulator holds no
/// entries for it, so an empty list is never reported as though one had been recorded.
fn optional_list(values: &[String]) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values.to_vec())
    }
}

/// Per-service state for com-amazonaws-directoryservice-directoryservice-20150416. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Every stored directory, keyed by its `DirectoryId`.
    pub directories: BTreeMap<String, Directory>,
    /// Every stored directory assessment, keyed by its `AssessmentId`.
    pub assessments: BTreeMap<String, AssessmentRecord>,
    /// Every stored snapshot, keyed by its `SnapshotId`. A snapshot names the directory
    /// it was taken of, so this is one store rather than one per directory.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// Every stored trust relationship, keyed by its `TrustId`. A trust names the
    /// directory it belongs to, and `DescribeTrusts`/`VerifyTrust`/`DeleteTrust` all
    /// resolve one by trust id alone, so this is one store rather than one per directory.
    pub trusts: BTreeMap<String, TrustRecord>,
    /// Every stored directory share, keyed by its `SharedDirectoryId`. A share names the
    /// owner directory it was created from, and `AcceptSharedDirectory` /
    /// `RejectSharedDirectory` resolve one by shared directory id alone, so this is one
    /// store rather than one per directory.
    pub shared_directories: BTreeMap<String, SharedDirectoryRecord>,
    /// Which directory owns which alias. An alias is unique across the account, so
    /// this is what `CreateAlias` checks before assigning one. An entry OUTLIVES the
    /// directory it was created for: `CreateAlias`' documentation says an alias
    /// "cannot be deleted or reused", so `DeleteDirectory` leaves it reserved — which
    /// is why this index carries its own cap, [`MAX_RESERVED_ALIASES`], enforced at
    /// the single insert site in `create_alias.rs`.
    pub aliases: BTreeMap<String, String>,
    /// The `DirectoryId` of every directory this account HAD and has since deleted, with
    /// the creation rank it carried ([`Self::next_seq`]).
    ///
    /// Why a deletion is remembered: a reporting call that FILTERS by directory id answers
    /// a deleted id with an empty result rather than refusing it, while an id this account
    /// never had is refused — so a client polling `DescribeDirectories` to confirm a
    /// deletion finished gets a clean empty page instead of an exception, and one that
    /// passes an id nobody ever minted is still told the value is wrong. Only the id is
    /// kept: the record itself is gone, so nothing of the deleted directory can be
    /// reported.
    ///
    /// Bounded by [`MAX_REMEMBERED_DELETIONS`] at its single insert site
    /// ([`Self::remember_deleted_directory`]), which drops the earliest-created id once the
    /// bound is reached — so a create/delete cycle cannot grow this store without end.
    pub deleted_directories: BTreeMap<String, u64>,
    /// Drives [`Self::mint_directory_id`]; see there for how an id is derived from it.
    id_seed: u64,
    /// Drives [`Self::mint_assessment_id`], the way [`Self::id_seed`] drives the
    /// directory ids.
    assessment_id_seed: u128,
    /// Drives [`Self::mint_snapshot_id`], kept apart from [`Self::id_seed`] so the two
    /// id spaces advance independently.
    snapshot_id_seed: u64,
    /// Drives [`Self::mint_certificate_id`], its own id space for the same reason.
    certificate_id_seed: u64,
    /// Drives [`Self::mint_trust_id`], kept apart from the other seeds so the id spaces
    /// advance independently.
    trust_id_seed: u64,
    /// Drives [`Self::mint_schema_extension_id`], its own id space for the same reason.
    schema_extension_id_seed: u64,
    /// Drives [`Self::mint_domain_controller_id`], its own id space for the same reason.
    domain_controller_id_seed: u64,
    /// Drives [`Self::next_seq`]: the creation rank stamped on each directory.
    seq_counter: u64,
}

/// The number of distinct ids [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_directory_id`]
/// can render: `16^10`, the width the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// allows.
const ID_SPACE: u64 = 1 << 40;

/// Scrambles consecutive seeds across [`ID_SPACE`]. Odd, so it is coprime with
/// `16^10 = 2^40`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)` a
/// BIJECTION: distinct seeds can never mint the same id.
const ID_MULTIPLIER: u64 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading zeros.
const ID_OFFSET: u64 = 734_291;

/// The number of distinct ids
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_assessment_id`]
/// can render: `16^18 = 2^72`, the width the modelled `AssessmentId` pattern
/// `^da-[0-9a-f]{18}$` allows. Wider than [`ID_SPACE`], which is why an assessment id
/// cannot come from [`mint_id`].
const ASSESSMENT_ID_SPACE: u128 = 1 << 72;

/// Scrambles consecutive seeds across [`ASSESSMENT_ID_SPACE`], the same construction as
/// [`ID_MULTIPLIER`]: odd, so it is coprime with `2^72` and the map is a BIJECTION.
const ASSESSMENT_ID_MULTIPLIER: u128 = 1_000_000_000_000_000_003;

/// The assessment bijection's constant term, so the first id minted is not a run of
/// leading zeros.
const ASSESSMENT_ID_OFFSET: u128 = 9_876_543_210_987;

/// Mint the next id of one id space: `<prefix>-` followed by ten lower-case hex digits,
/// the shape both the modelled `DirectoryId` (`^d-[0-9a-f]{10}$`) and `SnapshotId`
/// (`^s-[0-9a-f]{10}$`) declare.
///
/// `seed` is the id space's own counter, advanced here and mapped through the bijection
/// described on [`ID_MULTIPLIER`], so an id looks unpredictable yet never repeats within
/// a process. One function, so the derivation cannot drift between the id spaces.
fn mint_id(seed: &mut u64, prefix: &str) -> String {
    *seed = seed.wrapping_add(1);
    let scrambled = seed
        .wrapping_mul(ID_MULTIPLIER)
        .wrapping_add(ID_OFFSET)
        % ID_SPACE;
    format!("{prefix}-{scrambled:010x}")
}

impl ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Mint a fresh `DirectoryId` of the shape the model declares,
    /// `^d-[0-9a-f]{10}$`.
    ///
    /// The id is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so it is unpredictable-looking yet never repeats within a
    /// process.
    pub fn mint_directory_id(&mut self) -> String {
        mint_id(&mut self.id_seed, "d")
    }

    /// Mint a fresh `SnapshotId` of the shape the model declares,
    /// `^s-[0-9a-f]{10}$`, through the same bijection as
    /// [`Self::mint_directory_id`] over its own seed.
    pub fn mint_snapshot_id(&mut self) -> String {
        mint_id(&mut self.snapshot_id_seed, "s")
    }

    /// Mint a fresh `CertificateId` of the shape the model declares,
    /// `^c-[0-9a-f]{10}$`, through the same bijection as [`Self::mint_directory_id`]
    /// over its own seed.
    pub fn mint_certificate_id(&mut self) -> String {
        mint_id(&mut self.certificate_id_seed, "c")
    }

    /// Mint a fresh `TrustId` of the shape the model declares, `^t-[0-9a-f]{10}$`,
    /// through the same bijection as [`Self::mint_directory_id`] over its own seed.
    pub fn mint_trust_id(&mut self) -> String {
        mint_id(&mut self.trust_id_seed, "t")
    }

    /// Mint a fresh `SchemaExtensionId` of the shape the model declares,
    /// `^e-[0-9a-f]{10}$`, through the same bijection as [`Self::mint_directory_id`]
    /// over its own seed.
    pub fn mint_schema_extension_id(&mut self) -> String {
        mint_id(&mut self.schema_extension_id_seed, "e")
    }

    /// Mint a fresh `DomainControllerId` of the shape the model declares,
    /// `^dc-[0-9a-f]{10}$`, through the same bijection as [`Self::mint_directory_id`]
    /// over its own seed.
    pub fn mint_domain_controller_id(&mut self) -> String {
        mint_id(&mut self.domain_controller_id_seed, "dc")
    }

    /// Record that the directory `directory_id`, created at rank `seq`, has been deleted.
    ///
    /// THE single insert site of [`Self::deleted_directories`], so the bound on that store
    /// is enforced in one place: once [`MAX_REMEMBERED_DELETIONS`] ids are remembered the
    /// earliest-created one is dropped, which keeps the store finite however many
    /// create/delete cycles a caller runs. A dropped id is then one this account cannot be
    /// shown to have had, and a filter naming it is refused the way an id nobody minted is.
    pub fn remember_deleted_directory(&mut self, directory_id: &str, seq: u64) {
        self.deleted_directories
            .insert(directory_id.to_string(), seq);
        while self.deleted_directories.len() > MAX_REMEMBERED_DELETIONS {
            let Some(earliest) = self
                .deleted_directories
                .iter()
                .min_by_key(|(_, seq)| **seq)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.deleted_directories.remove(&earliest);
        }
    }

    /// Whether `directory_id` names a directory this account HAD and has since deleted.
    ///
    /// What it is for: a reporting call that filters by directory id answers such an id
    /// with an empty result, while an id this account never had is refused — see
    /// [`Self::deleted_directories`].
    pub fn was_deleted(&self, directory_id: &str) -> bool {
        self.deleted_directories.contains_key(directory_id)
    }

    /// Place `count` domain controllers in the directory `directory_id`, as of `now`.
    ///
    /// One controller per subnet of the directory's VPC settings, taken in the order the
    /// create named them and wrapping round when more controllers are asked for than
    /// there are subnets: `DirectoryVpcSettings$SubnetIds`' own documentation says
    /// "Directory Service creates a directory server and a DNS server in each of these
    /// subnets", so a controller resides in a subnet of the directory rather than
    /// nowhere. Each one holds an address out of THAT subnet's own CIDR block
    /// ([`Directory::server_address_in`]), counted so that two controllers of one directory
    /// in one subnet never share one — and those addresses are what the directory serves
    /// DNS at ([`Directory::dns_ip_addrs`]). Each one is placed in
    /// [`DOMAIN_CONTROLLER_STATUS_CREATING`] with the move to
    /// [`DOMAIN_CONTROLLER_STATUS_ACTIVE`] owed by the clock at `ready_at`, so a caller
    /// that describes them immediately still sees the transient status.
    ///
    /// `ready_at` is the instant the controllers are in service, which the caller owns
    /// because it differs by what placed them: the controllers a CREATE places are the
    /// servers the directory is made of, so they are ready when the directory itself is
    /// ([`directory_active_at`]), while a resize of a directory already in service adds a
    /// controller on its own [`TRANSITION_SPAN`].
    ///
    /// The directory is never left holding more than
    /// [`MAX_DOMAIN_CONTROLLERS_PER_DIRECTORY`] — the count is clamped against what the
    /// directory already holds — and nothing is placed for an id that names no stored
    /// directory: there is nothing to place controllers in.
    pub fn place_domain_controllers(
        &mut self,
        directory_id: &str,
        count: usize,
        now: SystemTime,
        ready_at: SystemTime,
    ) {
        let Some(directory) = self.directories.get(directory_id) else {
            return;
        };
        let subnets = directory.subnet_ids();
        let already_placed = directory.domain_controllers.len();
        // The number of controllers to place is clamped BEFORE anything is allocated from
        // it, and against the controllers the directory ALREADY holds, so no sequence of
        // calls carrying a caller-supplied number can take one directory's set past the
        // per-directory limit.
        let count = count.min(MAX_DOMAIN_CONTROLLERS_PER_DIRECTORY.saturating_sub(already_placed));
        // Where each new controller goes, read off the directory's own placement: the next
        // subnet in the round and the address it holds out of that subnet's block. The round
        // over the subnets continues from the controllers the directory already holds, so a
        // controller added to a directory in service lands in the next subnet rather than
        // doubling up on the first one — and the second controller of one subnet takes the
        // next address of its block rather than the first one again.
        let subnets_in_round = subnets.len().max(1);
        let placements: Vec<(Option<String>, Option<String>)> = (0..count)
            .map(|index| {
                let position = already_placed + index;
                let subnet_id = subnets.get(position % subnets_in_round).cloned();
                let ordinal = u32::try_from(position / subnets_in_round).unwrap_or(u32::MAX);
                let address = directory.server_address_in(subnet_id.as_deref(), ordinal);
                (subnet_id, address)
            })
            .collect();
        // The ids and the placement ranks come from the service-wide seeds, so they are
        // minted before the directory is borrowed mutably below.
        let mut placed: Vec<DomainControllerRecord> = Vec::with_capacity(count);
        for (subnet_id, dns_ip_addr) in placements {
            let domain_controller_id = self.mint_domain_controller_id();
            let seq = self.next_seq();
            placed.push(DomainControllerRecord {
                domain_controller_id,
                subnet_id,
                dns_ip_addr,
                status: DOMAIN_CONTROLLER_STATUS_CREATING.to_string(),
                status_due_at: Some(ready_at),
                launch_time: now,
                status_last_updated: now,
                seq,
            });
        }
        let Some(directory) = self.directories.get_mut(directory_id) else {
            return;
        };
        for record in placed {
            directory
                .domain_controllers
                .insert(record.domain_controller_id.clone(), record);
        }
    }

    /// Take the `count` most recently placed domain controllers out of the directory
    /// `directory_id`, as of `now`.
    ///
    /// The controllers placed LAST are the ones a resize down takes out, so the servers a
    /// directory has been running longest are the ones it keeps. Each one moves to
    /// [`DOMAIN_CONTROLLER_STATUS_DELETING`] with its removal owed by the clock one
    /// [`TRANSITION_SPAN`] later, so a caller that describes the directory immediately
    /// still sees the controller in that transient status before it leaves the store
    /// ([`Self::advance_clock`]) — the same shape as an address block leaving a directory.
    ///
    /// A controller already on its way out is not retired twice, and nothing happens for
    /// an id that names no stored directory.
    pub fn retire_domain_controllers(&mut self, directory_id: &str, count: usize, now: SystemTime) {
        let Some(directory) = self.directories.get_mut(directory_id) else {
            return;
        };
        let retiring: Vec<String> = directory
            .domain_controllers_in_placement_order()
            .into_iter()
            .filter(|controller| controller.status != DOMAIN_CONTROLLER_STATUS_DELETING)
            .rev()
            .take(count)
            .map(|controller| controller.domain_controller_id.clone())
            .collect();
        for domain_controller_id in retiring {
            if let Some(controller) = directory.domain_controllers.get_mut(&domain_controller_id) {
                controller.status = DOMAIN_CONTROLLER_STATUS_DELETING.to_string();
                controller.status_due_at = Some(now + TRANSITION_SPAN);
                controller.status_last_updated = now;
            }
        }
    }

    /// The next creation rank. Stamped on a directory or an assessment when it is
    /// created, so a listing reports creation order even though the records are keyed
    /// by id.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }

    /// Mint a fresh `AssessmentId` of the shape the model declares,
    /// `^da-[0-9a-f]{18}$`.
    ///
    /// Derived from its own counter through the bijection described on
    /// [`ASSESSMENT_ID_MULTIPLIER`]. Eighteen hex digits do not fit [`mint_id`]'s
    /// ten-digit space, so this is the same construction over a wider one.
    pub fn mint_assessment_id(&mut self) -> String {
        self.assessment_id_seed = self.assessment_id_seed.wrapping_add(1);
        let scrambled = self
            .assessment_id_seed
            .wrapping_mul(ASSESSMENT_ID_MULTIPLIER)
            .wrapping_add(ASSESSMENT_ID_OFFSET)
            % ASSESSMENT_ID_SPACE;
        format!("da-{scrambled:018x}")
    }

    /// Materialise every status transition the clock has already made due at `now`.
    ///
    /// This is the ONLY write a reporting operation performs: the changes here were
    /// owed at `now` whether or not anybody called, because each due instant was stored
    /// when the record it belongs to was created — a directory's stage when
    /// `CreateDirectory` ran, a snapshot's status when `CreateSnapshot` ran, an IP
    /// route's status when `AddIpRoutes` or `RemoveIpRoutes` ran, an assessment's
    /// status when
    /// `StartADAssessment` ran, a directory's data-access status when
    /// `EnableDirectoryDataAccess` ran, a directory setting's status when
    /// `UpdateSettings` ran, a directory's LDAPS status when `EnableLDAPS`
    /// ran, a domain controller's status when the placement or the retirement of it ran,
    /// a directory's teardown when `DeleteDirectory` ran.
    pub fn advance_clock(&mut self, now: SystemTime) {
        for directory in self.directories.values_mut() {
            // The stage a create owes is a CHAIN — `Requested`, then `Creating` for the
            // build window, then `Active` — so every step the clock has already made due
            // is materialised here, however long ago the create ran. Each step stores the
            // next step's due instant, so the chain is driven by the create's own clock
            // and not by this call, and ONLY the two stages of that chain are advanced: a
            // directory in any other stage keeps it, so no future transient stage of the
            // modelled enum can be taken for the end of this one.
            while let Some(due) = directory.stage_due_at {
                if now < due {
                    break;
                }
                match directory.stage.as_str() {
                    STAGE_REQUESTED => {
                        directory.stage = STAGE_CREATING.to_string();
                        directory.stage_last_updated = due;
                        directory.stage_due_at = Some(due + DIRECTORY_BUILD_SPAN);
                    }
                    STAGE_CREATING => {
                        directory.stage = STAGE_ACTIVE.to_string();
                        directory.stage_last_updated = due;
                        directory.stage_due_at = None;
                    }
                    _ => break,
                }
            }
            // The status transition an `AddRegion` owes: the replication it starts answers
            // while the Region is still being brought up, so the transient `Requested`
            // settles on `Active` one span later — the same transition the directory's own
            // Region made when it was created.
            for replication in directory.additional_regions.values_mut() {
                let Some(due) = replication.status_due_at else {
                    continue;
                };
                if now >= due {
                    replication.status = STAGE_ACTIVE.to_string();
                    replication.status_last_updated = due;
                    replication.status_due_at = None;
                }
            }
            // The status transitions the IP route writes owe: the transient `Adding` an
            // add answers in settles on `Added`, and a block whose removal has fallen
            // due is gone — a block the directory no longer routes is not one
            // `ListIpRoutes` reports.
            for route in directory.ip_routes.values_mut() {
                let Some(due) = route.status_due_at else {
                    continue;
                };
                if now >= due && route.status == IP_ROUTE_STATUS_ADDING {
                    route.status = IP_ROUTE_STATUS_ADDED.to_string();
                    route.status_due_at = None;
                }
                // A block being removed has no settled status to move to: it leaves the
                // store below, so its due instant is left in place for that pass to
                // find.
            }
            directory.ip_routes.retain(|_, route| {
                route.status != IP_ROUTE_STATUS_REMOVING
                    || route.status_due_at.is_some_and(|due| now < due)
            });
            // The data-access transitions the toggles owe: the transient `Enabling`
            // `EnableDirectoryDataAccess` records settles on `Enabled`, and the
            // transient `Disabling` `DisableDirectoryDataAccess` records settles back on
            // `Disabled`. Either way the directory keeps a `DataAccessStatus`, so
            // neither transition removes anything from the store.
            if let Some(due) = directory.data_access_due_at {
                if now >= due {
                    if directory.data_access_status == DATA_ACCESS_ENABLING {
                        directory.data_access_status = DATA_ACCESS_ENABLED.to_string();
                        directory.data_access_due_at = None;
                    } else if directory.data_access_status == DATA_ACCESS_DISABLING {
                        directory.data_access_status = DATA_ACCESS_DISABLED.to_string();
                        directory.data_access_due_at = None;
                    }
                }
            }
            // The transition an `UpdateSettings` owes: the value the update REQUESTED
            // becomes the value APPLIED to the directory, and the setting's transient
            // `Requested` settles on `Updated`. Nothing is removed — a directory keeps
            // every setting it carries, at whatever value it last settled on.
            for setting in directory.settings.values_mut() {
                let Some(due) = setting.status_due_at else {
                    continue;
                };
                if now >= due {
                    setting.applied_value = setting.requested_value.clone();
                    setting.status = DIRECTORY_CONFIGURATION_STATUS_UPDATED.to_string();
                    setting.last_updated = due;
                    setting.status_due_at = None;
                }
            }
            // The state transition `RegisterCertificate` owes: the transient
            // `Registering` settles on `Registered`. The teardown
            // `DeregisterCertificate` owes has no settled state to move to — a
            // registration the system no longer holds is not one this service reports —
            // so once it falls due the registration is gone.
            for certificate in directory.certificates.values_mut() {
                let Some(due) = certificate.state_due_at else {
                    continue;
                };
                if now >= due && certificate.state == CERTIFICATE_STATE_REGISTERING {
                    certificate.state = CERTIFICATE_STATE_REGISTERED.to_string();
                    certificate.state_due_at = None;
                }
                // A certificate being deregistered leaves the store below, so its due
                // instant is left in place for that pass to find.
            }
            directory.certificates.retain(|_, certificate| {
                certificate.state != CERTIFICATE_STATE_DEREGISTERING
                    || certificate.state_due_at.is_some_and(|due| now < due)
            });
            // The status transition `EnableLDAPS` owes: the transient `Enabling`
            // settles on `Enabled`. A deactivation has no transient status to settle
            // out of — the modelled enum carries no `Disabling` — so `DisableLDAPS`
            // records the settled status itself and leaves nothing here to do.
            for setting in directory.ldaps_settings.values_mut() {
                let Some(due) = setting.status_due_at else {
                    continue;
                };
                if now >= due && setting.status == LDAPS_STATUS_ENABLING {
                    setting.status = LDAPS_STATUS_ENABLED.to_string();
                    setting.last_updated = due;
                    setting.status_due_at = None;
                }
            }
            // The status transitions the schema-extension writes owe: the transient
            // `Initializing` a start answers in settles on `Completed`, and the transient
            // `CancelInProgress` a cancel answers in settles on `Cancelled`. Either way
            // the extension is finished with, which is when it first has an end instant to
            // report.
            for extension in directory.schema_extensions.values_mut() {
                let Some(due) = extension.status_due_at else {
                    continue;
                };
                if now < due {
                    continue;
                }
                let settled = match extension.status.as_str() {
                    SCHEMA_EXTENSION_STATUS_INITIALIZING => SCHEMA_EXTENSION_STATUS_COMPLETED,
                    SCHEMA_EXTENSION_STATUS_CANCEL_IN_PROGRESS => {
                        SCHEMA_EXTENSION_STATUS_CANCELLED
                    }
                    _ => continue,
                };
                extension.status = settled.to_string();
                extension.end_date_time = Some(due);
                extension.status_due_at = None;
            }
            // The status transitions the domain-controller writes owe: the transient
            // `Creating` a placement answers in settles on `Active`, the status a
            // controller in service carries, and a controller whose removal has fallen
            // due is gone — a controller the directory no longer runs is not one
            // `DescribeDomainControllers` reports. The status a controller rebuilt from a
            // snapshot passes through belongs to `RestoreFromSnapshot`, still scaffolded,
            // so it lands with that handler rather than as a branch nothing can reach.
            for controller in directory.domain_controllers.values_mut() {
                let Some(due) = controller.status_due_at else {
                    continue;
                };
                if now >= due && controller.status == DOMAIN_CONTROLLER_STATUS_CREATING {
                    controller.status = DOMAIN_CONTROLLER_STATUS_ACTIVE.to_string();
                    controller.status_last_updated = due;
                    controller.status_due_at = None;
                }
                // A controller being removed has no settled status to move to: it leaves
                // the store below, so its due instant is left in place for that pass to
                // find.
            }
            directory.domain_controllers.retain(|_, controller| {
                controller.status != DOMAIN_CONTROLLER_STATUS_DELETING
                    || controller.status_due_at.is_some_and(|due| now < due)
            });
        }
        // The chain a DELETE owes: the transient `Deleting` a delete records settles on the
        // terminal `Deleted`, after which the directory's record leaves the store — a
        // directory this service no longer holds is not one `DescribeDirectories` reports.
        // It runs after the stage chain a create owes, so one pass can carry a directory
        // whose delete ran long enough ago all the way out.
        self.materialise_directory_deletions(now);
        for snapshot in self.snapshots.values_mut() {
            let Some(due) = snapshot.status_due_at else {
                continue;
            };
            if now >= due {
                snapshot.status = SNAPSHOT_STATUS_COMPLETED.to_string();
                snapshot.status_due_at = None;
            }
        }
        // The state transitions the trust writes owe: the transient `Creating` a create
        // answers in settles on `Created` when a peer answers for its remote domain, and on
        // `Failed` when none does — establishing the Amazon Web Services side needs a peer
        // at that domain, and that is the state reason the failed trust then carries. Which
        // domains have a peer is decided from STORED state and a string comparison (see
        // [`remote_domain_has_a_peer`]): the domains this service itself serves are peers,
        // and of the external names the ones the IETF reserves as never installed can have
        // none. Nothing here resolves a name, consults a resolver or opens a socket, so no
        // caller-supplied value leaves this process and the outcome is the same on every
        // host. The
        // `Verifying` a verify answers in settles on `Verified`, and the `Deleting` a
        // delete answers in settles on `Deleted` — after which the relationship leaves
        // the store one span later, because a trust this service no longer holds is not
        // one `DescribeTrusts` reports. The transition `UpdateTrust` owes belongs to that
        // handler, which is still scaffolded, so it lands with it rather than as a branch
        // nothing can reach.
        //
        // Read once, before the trusts are walked, so every trust falling due in this pass
        // resolves its remote domain against the same directories — the ones this service
        // holds at `now`, with the stage transitions above already materialised.
        let domains_served = self.domains_served();
        for trust in self.trusts.values_mut() {
            let Some(due) = trust.trust_state_due_at else {
                continue;
            };
            if now < due {
                continue;
            }
            let (settled, reason) = match trust.trust_state.as_str() {
                TRUST_STATE_CREATING => {
                    if remote_domain_has_a_peer(&trust.remote_domain_name, &domains_served) {
                        (TRUST_STATE_CREATED, None)
                    } else {
                        (
                            TRUST_STATE_FAILED,
                            Some(TRUST_CREATE_FAILURE_REASON.to_string()),
                        )
                    }
                }
                TRUST_STATE_VERIFYING => (TRUST_STATE_VERIFIED, None),
                TRUST_STATE_DELETING => (TRUST_STATE_DELETED, None),
                // A trust that has reached `Deleted`, or whose failure has been on record
                // for as long as this service keeps one, has no further state to move to:
                // it leaves the store below, so its due instant is left in place for that
                // pass to find.
                _ => continue,
            };
            trust.trust_state = settled.to_string();
            trust.trust_state_reason = reason;
            trust.state_last_updated = due;
            trust.last_updated = due;
            // A trust that has just been torn down is held for one more span, so the
            // settled `Deleted` state is one a caller can read before the record goes.
            trust.trust_state_due_at = if settled == TRUST_STATE_DELETED {
                Some(due + TRANSITION_SPAN)
            } else {
                None
            };
        }
        self.trusts.retain(|_, trust| {
            trust.trust_state != TRUST_STATE_DELETED
                || trust.trust_state_due_at.is_some_and(|due| now < due)
        });
        // The status transition an organization share owes: `ShareDirectory` answers
        // while the share is still being put in service, so the transient `Sharing`
        // settles on `Shared`. A handshake share rests at `PendingAcceptance` instead,
        // which no clock settles: the consumer account's answer does.
        for share in self.shared_directories.values_mut() {
            let Some(due) = share.share_status_due_at else {
                continue;
            };
            if now >= due && share.share_status == SHARE_STATUS_SHARING {
                share.share_status = SHARE_STATUS_SHARED.to_string();
                share.last_updated = due;
                share.share_status_due_at = None;
            }
        }
        // The teardown `UnshareDirectory` owes: a share being deleted has no settled
        // status to move to, because unsharing takes the consumer account's shared
        // directory away — so once the teardown falls due the share leaves the store and
        // the pairing can be shared again.
        self.shared_directories.retain(|_, share| {
            share.share_status != SHARE_STATUS_DELETING
                || share.share_status_due_at.is_some_and(|due| now < due)
        });
        for assessment in self.assessments.values_mut() {
            let Some(due) = assessment.status_due_at else {
                continue;
            };
            if now >= due {
                assessment.status = ASSESSMENT_STATUS_FAILED.to_string();
                assessment.status_reason = Some(ASSESSMENT_FAILED_REASON.to_string());
                assessment.last_update = due;
                assessment.status_due_at = None;
            }
        }
    }

    /// How many stored directories count towards `category` — the current count
    /// `GetDirectoryLimits` reports for that counter of `DirectoryLimits`.
    pub fn directories_in_category(&self, category: DirectoryLimitCategory) -> usize {
        self.directories
            .values()
            .filter(|directory| {
                DirectoryLimitCategory::of(&directory.directory_type) == Some(category)
            })
            .count()
    }

    /// Directories in creation order — the order `DescribeDirectories` pages over.
    pub fn directories_in_creation_order(&self) -> Vec<&Directory> {
        let mut ordered: Vec<&Directory> = self.directories.values().collect();
        ordered.sort_by_key(|directory| directory.seq);
        ordered
    }

    /// Assessments in creation order — the order `ListADAssessments` pages over.
    pub fn assessments_in_creation_order(&self) -> Vec<&AssessmentRecord> {
        let mut ordered: Vec<&AssessmentRecord> = self.assessments.values().collect();
        ordered.sort_by_key(|assessment| assessment.seq);
        ordered
    }

    /// How many CUSTOMER assessments the account currently holds — what
    /// [`MAX_CUSTOMER_ASSESSMENTS`] bounds.
    pub fn customer_assessment_count(&self) -> usize {
        self.assessments
            .values()
            .filter(|assessment| assessment.report_type == ASSESSMENT_REPORT_TYPE_CUSTOMER)
            .count()
    }

    /// Snapshots in creation order — the order `DescribeSnapshots` pages over.
    pub fn snapshots_in_creation_order(&self) -> Vec<&SnapshotRecord> {
        let mut ordered: Vec<&SnapshotRecord> = self.snapshots.values().collect();
        ordered.sort_by_key(|snapshot| snapshot.seq);
        ordered
    }

    /// Trust relationships in creation order — the order `DescribeTrusts` pages over.
    pub fn trusts_in_creation_order(&self) -> Vec<&TrustRecord> {
        let mut ordered: Vec<&TrustRecord> = self.trusts.values().collect();
        ordered.sort_by_key(|trust| trust.seq);
        ordered
    }

    /// The trust `directory_id` already holds with `remote_domain_name`, if any: a
    /// directory is one side of at most one trust with a given external domain, so this is
    /// what a second `CreateTrust` for it resolves as the entity that already exists.
    ///
    /// A relationship that is being or has been torn down, or whose establishment failed
    /// ([`TRUST_STATES_NOT_HELD`]), is not one the directory holds any more, even while
    /// the clock is still working through what it owes, so the directory can establish a
    /// trust with that domain again.
    pub fn trust_of(&self, directory_id: &str, remote_domain_name: &str) -> Option<&TrustRecord> {
        let key = domain_key(remote_domain_name);
        self.trusts.values().find(|trust| {
            trust.directory_id == directory_id
                && domain_key(&trust.remote_domain_name) == key
                && !TRUST_STATES_NOT_HELD.contains(&trust.trust_state.as_str())
        })
    }

    /// The DNS domains this service itself serves: the fully qualified domain name of
    /// every directory it stores, folded by [`domain_key`] so a lookup agrees with DNS.
    ///
    /// A directory whose teardown has started ([`STAGE_DELETING`], [`STAGE_DELETED`]) is
    /// NOT one of them: its servers are being decommissioned, so nothing answers DNS at the
    /// addresses `DirectoryDescription$DnsIpAddrs` reports for it any more, even while the
    /// control plane still describes the directory through that teardown.
    ///
    /// A trust's remote domain is resolved against this set
    /// ([`remote_domain_has_a_peer`]): a directory this service holds IS a peer at its own
    /// domain, so a trust pointed at one establishes, and `DeleteDirectory` taking that
    /// directory out of service takes the peer with it.
    pub fn domains_served(&self) -> BTreeSet<String> {
        self.directories
            .values()
            .filter(|directory| {
                !directory_deletion::DIRECTORY_TEARDOWN_STAGES.contains(&directory.stage.as_str())
            })
            .map(|directory| domain_key(&directory.name))
            .collect()
    }

    /// Directory shares in creation order — the order `DescribeSharedDirectories` pages
    /// over.
    pub fn shared_directories_in_creation_order(&self) -> Vec<&SharedDirectoryRecord> {
        let mut ordered: Vec<&SharedDirectoryRecord> = self.shared_directories.values().collect();
        ordered.sort_by_key(|share| share.seq);
        ordered
    }

    /// The share `owner_directory_id` already holds with the consumer account
    /// `shared_account_id`, if any: a directory is shared with a given account at most
    /// once, so this is what a second `ShareDirectory` for it resolves as the share that
    /// already exists.
    pub fn share_of(
        &self,
        owner_directory_id: &str,
        shared_account_id: &str,
    ) -> Option<&SharedDirectoryRecord> {
        self.shared_directories.values().find(|share| {
            share.owner_directory_id == owner_directory_id
                && share.shared_account_id == shared_account_id
        })
    }

    /// The same share as [`Self::share_of`], for a caller that changes it: what
    /// `UnshareDirectory` resolves as the one share its call stops.
    pub fn share_mut(
        &mut self,
        owner_directory_id: &str,
        shared_account_id: &str,
    ) -> Option<&mut SharedDirectoryRecord> {
        self.shared_directories.values_mut().find(|share| {
            share.owner_directory_id == owner_directory_id
                && share.shared_account_id == shared_account_id
        })
    }

    /// How many consumer accounts `owner_directory_id` is currently shared with — the
    /// count [`MAX_SHARES_PER_DIRECTORY`] bounds.
    pub fn share_count(&self, owner_directory_id: &str) -> usize {
        self.shared_directories
            .values()
            .filter(|share| share.owner_directory_id == owner_directory_id)
            .count()
    }

    /// How many trust relationships `directory_id` currently holds — the count
    /// [`MAX_TRUSTS_PER_DIRECTORY`] bounds, and so the bound on what the trust store can
    /// hold for one directory.
    ///
    /// A relationship whose teardown the clock is still working through
    /// ([`TORN_DOWN_TRUST_STATES`]) is on its way out of the store, so it is not counted.
    /// Every other record is — a relationship that FAILED included: it stays on record
    /// until the directory is deleted, so the quota has to see it, or a caller naming one
    /// non-resolvable domain after another could grow the store without end.
    pub fn trust_count(&self, directory_id: &str) -> usize {
        self.trusts
            .values()
            .filter(|trust| {
                trust.directory_id == directory_id
                    && !TORN_DOWN_TRUST_STATES.contains(&trust.trust_state.as_str())
            })
            .count()
    }

    /// How many manual snapshots `directory_id` currently holds — the count
    /// [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`] bounds.
    pub fn manual_snapshot_count(&self, directory_id: &str) -> usize {
        self.snapshots
            .values()
            .filter(|snapshot| {
                snapshot.directory_id == directory_id
                    && snapshot.snapshot_type == SNAPSHOT_TYPE_MANUAL
            })
            .count()
    }

    /// Why no further manual snapshot of `directory_id` can be stored — the message the
    /// modelled `SnapshotLimitExceededException` carries — or `None` when one can.
    ///
    /// Both bounds are checked here: the snapshot store's absolute ceiling
    /// ([`MAX_SNAPSHOTS`]) and the per-directory manual snapshot quota
    /// ([`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`]), which is the one
    /// `SnapshotLimitExceededException` describes. One place, so every operation that
    /// takes a snapshot of a directory — `CreateSnapshot`, and the
    /// `StartSchemaExtension` that was asked to snapshot first — refuses on the same
    /// bounds with the same wording.
    pub fn manual_snapshot_quota_exceeded(&self, directory_id: &str) -> Option<String> {
        if self.snapshots.len() >= MAX_SNAPSHOTS {
            return Some(format!(
                "Maximum number of snapshots has been reached: {max}",
                max = MAX_SNAPSHOTS
            ));
        }
        if self.manual_snapshot_count(directory_id) >= MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY {
            return Some(format!(
                "Maximum number of manual snapshots for directory {directory_id} has been \
                 reached: {max}",
                max = MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY
            ));
        }
        None
    }

    /// Persist a manual snapshot of `directory_id`, taken at `now`, and return its
    /// minted `SnapshotId`.
    ///
    /// The ONE place a snapshot record is built, so a snapshot taken on a caller's
    /// behalf is the same record whichever operation asked for it. The status transition
    /// the take owes (`Creating` to `Completed`) is stored as a due instant for
    /// [`Self::advance_clock`] to materialise. Callers check
    /// [`Self::manual_snapshot_quota_exceeded`] first.
    pub fn insert_manual_snapshot(
        &mut self,
        directory_id: &str,
        name: Option<String>,
        now: SystemTime,
    ) -> String {
        let snapshot_id = self.mint_snapshot_id();
        let seq = self.next_seq();
        self.snapshots.insert(
            snapshot_id.clone(),
            SnapshotRecord {
                snapshot_id: snapshot_id.clone(),
                directory_id: directory_id.to_string(),
                name,
                // A snapshot a caller asked for is a manual one; `Auto` is for the ones
                // Directory Service takes on its own schedule.
                snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
                status: SNAPSHOT_STATUS_CREATING.to_string(),
                status_due_at: Some(now + TRANSITION_SPAN),
                start_time: now,
                seq,
            },
        );
        snapshot_id
    }

    /// The manual snapshot limits `GetSnapshotLimits` reports for `directory_id`, as
    /// the modelled `SnapshotLimits`.
    ///
    /// Every member is derived rather than stored: the limit is the quota
    /// [`MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY`], the current count is what the snapshot
    /// store holds for that directory ([`Self::manual_snapshot_count`]), and
    /// `ManualSnapshotsLimitReached` ("Indicates if the manual snapshot limit has been
    /// reached") is the comparison of the two.
    pub fn snapshot_limits(&self, directory_id: &str) -> SnapshotLimits {
        let taken = self.manual_snapshot_count(directory_id);
        SnapshotLimits {
            manual_snapshots_limit: Some(clamp_to_i32(MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY)),
            manual_snapshots_current_count: Some(clamp_to_i32(taken)),
            manual_snapshots_limit_reached: taken >= MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY,
        }
    }

    /// The Region's directory limits `GetDirectoryLimits` reports, as the modelled
    /// `DirectoryLimits`.
    ///
    /// Every member is derived rather than stored. Each of the three counters the
    /// structure carries — cloud-only, cloud-only Managed Microsoft AD, connected —
    /// reports ITS OWN category's figures: [`Self::directories_in_category`] of that
    /// [`DirectoryLimitCategory`] as the current count, that category's published
    /// Region quota ([`DirectoryLimitCategory::quota`]) as the limit, and the
    /// `...LimitReached` flag ("Indicates if the … limit has been reached") as the
    /// comparison of the two — so the report says exactly when a further create of that
    /// category is refused with `DirectoryLimitExceededException`, which is what that
    /// error's own documentation points at this operation for.
    pub fn directory_limits(&self) -> DirectoryLimits {
        let cloud_only = self.category_report(DirectoryLimitCategory::CloudOnly);
        let microsoft_ad = self.category_report(DirectoryLimitCategory::CloudOnlyMicrosoftAd);
        let connected = self.category_report(DirectoryLimitCategory::Connected);
        DirectoryLimits {
            cloud_only_directories_limit: Some(cloud_only.limit),
            cloud_only_directories_current_count: Some(cloud_only.current_count),
            cloud_only_directories_limit_reached: cloud_only.limit_reached,
            cloud_only_microsoft_ad_limit: Some(microsoft_ad.limit),
            cloud_only_microsoft_ad_current_count: Some(microsoft_ad.current_count),
            cloud_only_microsoft_ad_limit_reached: microsoft_ad.limit_reached,
            connected_directories_limit: Some(connected.limit),
            connected_directories_current_count: Some(connected.current_count),
            connected_directories_limit_reached: connected.limit_reached,
        }
    }

    /// One counter of [`Self::directory_limits`]: the category's own quota, how many
    /// stored directories fall in it, and whether the quota is used up.
    fn category_report(&self, category: DirectoryLimitCategory) -> CategoryReport {
        let count = self.directories_in_category(category);
        let quota = category.quota();
        CategoryReport {
            limit: clamp_to_i32(quota),
            current_count: clamp_to_i32(count),
            limit_reached: count >= quota,
        }
    }

    /// Whether a further directory of `category` can be created: that category's own
    /// published quota against how many of it the store already holds. The one place a
    /// create decides whether it is past the quota, so the bound a create is refused at
    /// and the bound `GetDirectoryLimits` reports reached cannot drift apart.
    pub fn category_limit_reached(&self, category: DirectoryLimitCategory) -> bool {
        self.directories_in_category(category) >= category.quota()
    }
}

/// One counter of the modelled `DirectoryLimits`, as
/// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::category_report`]
/// derives it.
#[derive(Debug, Clone, Copy)]
struct CategoryReport {
    /// The category's published Region quota.
    limit: i32,
    /// How many stored directories fall in the category.
    current_count: i32,
    /// Whether the quota is used up.
    limit_reached: bool,
}

/// A count as the modelled `Limit` integer, which is 32 bits wide: every count a
/// `SnapshotLimits` or a `DirectoryLimits` reports is bounded by [`MAX_SNAPSHOTS`] or
/// [`MAX_DIRECTORIES`], far below the maximum, so the saturation can never be reached
/// and is here only so no cast can wrap.
fn clamp_to_i32(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// The opaque `NextToken` for a page of `kind` that ended at `value`.
///
/// Hex-encoded so the token carries no meaning a client could construct, and tagged
/// with both a version and the kind of listing that issued it, so a token replayed
/// against another operation is refused rather than misread.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{kind}:{value}"))
}

/// The cursor value inside a `kind` token, or `None` when `token` is not one this
/// service issued for that listing — which every caller answers with
/// `InvalidNextTokenException`.
///
/// The length check comes FIRST, so a caller-sized token is never decoded into a
/// caller-sized buffer: see [`MAX_NEXT_TOKEN_CHARS`].
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    if token.len() > MAX_NEXT_TOKEN_CHARS {
        return None;
    }
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let rest = decoded.strip_prefix(&format!("{CURSOR_VERSION}:{kind}:"))?;
    Some(rest.to_string())
}

/// The page size a `Limit` member resolves to.
///
/// From `Limit`'s own documentation: "The maximum number of items to return. If this
/// value is zero, the maximum number of items is specified by the limitations of the
/// operation" — so zero, and an absent member, both mean [`DEFAULT_PAGE_SIZE`].
pub fn page_size(limit: Option<i32>) -> usize {
    limit
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

/// A `SystemTime` as the modelled timestamp type.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The source of every request id this service reports.
///
/// Process-wide rather than a field of the state struct on purpose: a REPORTING
/// operation reports a request id too when it refuses a call, and a read must not write
/// to the store it reports on.
static MINTED_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// A fresh id for the call being answered, shaped the way the modelled `RequestId`
/// demands.
///
/// Every error structure in this service's model carries a `RequestId` whose `@pattern`
/// is `^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$`
/// (`smithy_query(query="shape",shape="EntityDoesNotExistException")`), so the value is
/// rendered as an RFC 4122 version-4 UUID. Its low field is a monotonic counter rather
/// than randomness, so the id is unique for the life of the process without being
/// derived from the request: two identical calls are answered with two different ids,
/// and no id is ever reused.
pub fn next_request_id() -> String {
    let minted = MINTED_REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{minted:012x}")
}

/// `message` with the request-id suffix Directory Service appends to the diagnostic of
/// every refusal: `… : RequestId: <id>`.
///
/// A recorded ds refusal carries its request id TWICE — in the modelled `RequestId`
/// member and at the end of `Message` — so the two are built together in [`refusal`] and
/// no handler can report one without the other.
fn with_request_id(message: &str, request_id: &str) -> String {
    format!("{message} : RequestId: {request_id}")
}

/// One of this service's modelled error structures.
///
/// Every error shape Directory Service models carries the same two members — `Message`
/// and the UUID-shaped `RequestId` — and the macro below implements this for each of
/// them, so [`refusal`] is the single place either member is ever filled.
pub trait ModeledFailure: Sized {
    /// The structure carrying `message` and `request_id`.
    fn from_parts(message: String, request_id: String) -> Self;
}

/// Implement [`ModeledFailure`] for each named error shape of this service's model.
macro_rules! modeled_failures {
    ($($error:ident),+ $(,)?) => {
        $(
            impl ModeledFailure for crate::ops::types::$error {
                fn from_parts(message: String, request_id: String) -> Self {
                    Self {
                        message: Some(message),
                        request_id: Some(request_id),
                    }
                }
            }
        )+
    };
}

modeled_failures!(
    ADAssessmentLimitExceededException,
    AccessDeniedException,
    AuthenticationFailedException,
    CertificateAlreadyExistsException,
    CertificateDoesNotExistException,
    CertificateInUseException,
    CertificateLimitExceededException,
    ClientException,
    DirectoryAlreadyInRegionException,
    DirectoryAlreadySharedException,
    DirectoryDoesNotExistException,
    DirectoryInDesiredStateException,
    DirectoryLimitExceededException,
    DirectoryNotSharedException,
    DirectoryUnavailableException,
    DomainControllerLimitExceededException,
    EntityAlreadyExistsException,
    EntityDoesNotExistException,
    InsufficientPermissionsException,
    InvalidCertificateException,
    InvalidClientAuthStatusException,
    InvalidLDAPSStatusException,
    InvalidNextTokenException,
    InvalidParameterException,
    InvalidPasswordException,
    InvalidTargetException,
    IpRouteLimitExceededException,
    NoAvailableCertificateException,
    RegionLimitExceededException,
    ServiceException,
    ShareLimitExceededException,
    SnapshotLimitExceededException,
    TagLimitExceededException,
    UnsupportedOperationException,
    UnsupportedSettingsException,
    UserDoesNotExistException,
);

/// Refuse the call with one of this service's modelled errors, `message` as its
/// diagnostic.
///
/// THE one place a ds refusal is built: it mints the call's request id
/// ([`next_request_id`]), reports it in the modelled `RequestId` member and appends it
/// to the message ([`with_request_id`]), so no refusal leaves this service without the
/// correlation handle an AWS client, an SDK retry log or a support workflow reads a
/// failed call by. WHICH error is refused is the error-enum variant the value is
/// constructed into, so a handler names it exactly where it did before.
pub fn refusal<E: ModeledFailure>(message: impl Into<String>) -> E {
    let request_id = next_request_id();
    E::from_parts(with_request_id(&message.into(), &request_id), request_id)
}

/// Refuse the call with one of this service's modelled errors whose diagnostic NAMES the
/// call's own request id — `diagnostic` is handed the id this refusal reports and words the
/// sentence around it, after which the envelope's suffix follows as usual.
///
/// A family whose recorded text closes with the request id itself
/// ([`computer_already_exists`]) cannot be built before the id is minted, which is why the
/// sentence arrives as a function of it rather than as a string. One id is minted, so the
/// message and the modelled `RequestId` member still report exactly one correlation handle,
/// the one of THIS call.
pub fn refusal_naming_its_request_id<E: ModeledFailure>(
    diagnostic: impl FnOnce(&str) -> String,
) -> E {
    let request_id = next_request_id();
    let stated = diagnostic(&request_id);
    E::from_parts(with_request_id(&stated, &request_id), request_id)
}

/// Refuse the call with one of this service's modelled errors whose diagnostic is NOT this
/// service's own: the message is stated exactly as the layer that decided the failure words
/// it, with no request-id suffix after it.
///
/// An authorization failure is decided by IAM ahead of this service (see
/// `crate::service::authorization`), so its diagnostic is IAM's sentence and nothing else:
/// the correlation handle travels in the modelled `RequestId` member — which this still
/// mints and reports, so the response carries the same handle every other refusal does —
/// rather than inside a message clients compare against IAM's documented wording. Every
/// diagnostic this SERVICE words itself goes through [`refusal`], which appends the suffix.
pub fn refusal_stating_its_diagnostic_alone<E: ModeledFailure>(message: impl Into<String>) -> E {
    E::from_parts(message.into(), next_request_id())
}

/// Refuse the call with one of this service's modelled errors, where the diagnostic
/// ITSELF ends with the call's request-id suffix and the envelope's own suffix follows
/// it.
///
/// Only the tag family's diagnostic ([`invalid_resource_id`]) is recorded that way,
/// while every other family's carries the suffix once ([`refusal`]): the inner suffix
/// belongs to the layer that produces that family's text, which has already appended the
/// id by the time the envelope appends its own. Both occurrences are the id of THIS
/// call, the one the modelled `RequestId` member reports, so the message still carries
/// exactly one correlation handle.
///
/// The recorded body is where the doubling comes from, verbatim from the wire corpus's
/// `AddTagsToResource` refusal of an absent resource id (fixture
/// `ds-tags-error-paths-fake-directory#task#1`):
///
/// ```text
/// HTTP 400; EntityDoesNotExistException: Invalid resource ID. : RequestId:
/// f4a44547-8d53-4939-9c8a-7fc6f361d17a : RequestId: f4a44547-8d53-4939-9c8a-7fc6f361d17a
/// ```
///
/// The issue report that filed this quotes the same message with ONE suffix, which is an
/// abbreviation of that body: a single-suffix message replays as a divergence on
/// `$.Message` for all three tag operations, and the doubled one replays clean. Should a
/// later recording show the single form, this function and the test row that pins it go
/// away in favour of plain [`refusal`] rather than growing a second exception.
fn refusal_restating_its_request_id<E: ModeledFailure>(diagnostic: &str) -> E {
    let request_id = next_request_id();
    let stated = with_request_id(diagnostic, &request_id);
    E::from_parts(with_request_id(&stated, &request_id), request_id)
}


/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the Directory Service emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDirectoryserviceDirectoryservice20150416State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDirectoryserviceDirectoryservice20150416State>)
}

/// The SHARED AD directory inventory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A directory is created and deleted HERE and its objects are served by the Directory
/// Service DATA plane, which creates no directory of its own — so which directories the
/// account holds is ONE record under one shared type
/// ([`::service_common::ad_directories::AdDirectoryInventory`]) rather than a second copy
/// inside [`ComAmazonawsDirectoryserviceDirectoryservice20150416State`]: the engine keys
/// state by TYPE, so a copy of its own here would be invisible to the plane that reads it.
///
/// Three more of that plane's answers are facts only THIS service holds — whether a
/// directory's data access has been enabled, the domain name the directory was created
/// with, and whether an account's password has been set — so they are part of the same
/// record: that is how the data plane can refuse a directory whose data access was never
/// enabled, report the directory's own realm, and report an account as inactive until
/// `ResetUserPassword` has given it a password.
///
/// LOCK ORDER: a handler takes [`service_state`] first and this inventory second, which is
/// the order every writer of it keeps.
pub fn ad_directory_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ad_directories::AdDirectoryInventory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::ad_directories::AdDirectoryInventory,
            >,
        )
}

/// The `UnsupportedOperationException` message for a client-authentication request made
/// against a directory whose type does not offer client authentication.
///
/// FIXED text, as Directory Service words this refusal: the directory type it is not
/// supported for is the one the request named, which the caller already has, so the
/// sentence carries no variable part. [`refusal`] appends the request id, once.
pub const CLIENT_AUTHENTICATION_UNSUPPORTED: &str =
    "Client authentication operations are not supported for this Directory Type.";

/// The `UnsupportedOperationException` message for an LDAP-security request made against a
/// directory whose type does not serve an LDAPS endpoint.
///
/// FIXED text, worded as this error class's sentence for the client-authentication half
/// ([`CLIENT_AUTHENTICATION_UNSUPPORTED`]) is: the directory type it is not supported for
/// is the one the request named, which the caller already has, so the sentence carries no
/// variable part. [`refusal`] appends the request id, once.
pub const LDAPS_UNSUPPORTED: &str = "LDAPS operations are not supported for this Directory Type.";

/// The directory types client authentication — the certificate-based (smart card)
/// authentication a [`CERTIFICATE_TYPE_CLIENT_CERT_AUTH`] registration configures — is
/// offered for.
///
/// Directory Service offers client authentication with smart cards for AD Connector: the
/// mutual-TLS client authentication an AD Connector presents its users' certificates to
/// the self-managed directory through, which is what `ClientCertAuthSettings` ("the OCSP
/// responder" the certificates are checked against) configures. A request asking for it
/// on a directory of another type is refused with
/// [`CLIENT_AUTHENTICATION_UNSUPPORTED`].
pub const CLIENT_AUTHENTICATION_DIRECTORY_TYPES: [&str; 1] = [TYPE_AD_CONNECTOR];

/// The `CertificateType` a certificate that performs client authentication carries: the
/// `ClientCertAuth` value of the modelled `CertificateType` enum.
pub const CERTIFICATE_TYPE_CLIENT_CERT_AUTH: &str = "ClientCertAuth";

/// The `InvalidTargetException` message for a share whose target account is the directory
/// owner's own account.
///
/// Directory Service words it as the target the request named and why it cannot be that
/// one: the variable part is the account id the request sent, and the rest is the error
/// class's own sentence. [`refusal`] appends the request id, once.
pub fn share_target_is_the_owner(account_id: &str) -> String {
    format!(
        "The Target AWS account ID {account_id} you specified cannot be the same as the \
         directory owner AWS account ID."
    )
}

/// The sentence the modelled `DirectoryAlreadySharedException` states this refusal with:
/// "The specified directory has already been shared with this Amazon Web Services
/// account." FIXED text, with no variable part. [`refusal`] appends the request id, once.
pub const DIRECTORY_ALREADY_SHARED: &str =
    "The specified directory has already been shared with this Amazon Web Services account.";

/// The sentence the modelled `InvalidParameterException` states a refusal with when the
/// parameter itself is what the call cannot be served for: "One or more parameters are not
/// valid." FIXED text, the error class's own, for the paths that have nothing to add to
/// it. [`refusal`] appends the request id, once.
pub const INVALID_PARAMETER: &str = "One or more parameters are not valid.";

/// The `TagLimitExceededException` message a tag write that would take a resource past
/// [`MAX_TAGS`] is refused with.
///
/// FIXED text, the sentence Directory Service itself states for this class, and the same one
/// however many tags the call carried and whichever resource it addressed: the service
/// reports the allowance as broken, not by how much, so nothing of the request or of the
/// stored tags is quoted into it. The error shape's own documentation states the class the
/// same way ("The maximum allowed number of tags was exceeded"). [`refusal`] appends the
/// request id, once.
pub const TAG_LIMIT_EXCEEDED: &str = "Tag limit exceeded.";

/// The reserved prefix of a tag key AWS itself owns, which no caller may add or remove.
///
/// From AWS's own tagging rules: a tag key beginning with `aws:` is a system tag, and the
/// tagging APIs refuse to add or remove one. Compared case-insensitively, the way AWS
/// treats the reserved prefix.
pub const SYSTEM_TAG_KEY_PREFIX: &str = "aws:";

/// The `InvalidParameterException` message a tagging call naming a system tag key is
/// refused with.
///
/// FIXED text, as AWS words it for this refusal, and the same sentence whether the call
/// ADDS or REMOVES the key. [`refusal`] appends the request id, once.
pub const SYSTEM_TAG_KEY_REFUSED: &str =
    "Adding or removing system tags (with prefix aws:) is not supported.";

/// Whether any of `tag_keys` names the AWS-reserved tag namespace, which is what
/// [`SYSTEM_TAG_KEY_REFUSED`] refuses the call for.
///
/// One function, so `AddTagsToResource` and `RemoveTagsFromResource` cannot disagree
/// about which keys belong to AWS. The prefix is read through a TOTAL operation, so a key
/// whose fourth byte sits inside a multi-byte character — which the modelled `TagKey`
/// admits, since it allows any Unicode letter — is simply not in that namespace rather
/// than a refused slice.
pub fn names_system_tag_key<'a>(tag_keys: impl IntoIterator<Item = &'a str>) -> bool {
    tag_keys.into_iter().any(|key| {
        key.get(..SYSTEM_TAG_KEY_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(SYSTEM_TAG_KEY_PREFIX))
    })
}

/// The `InvalidParameterException` message for a verification asked for from a
/// `TrustState` a trust cannot be verified from ([`VERIFIABLE_TRUST_STATES`]).
///
/// Directory Service words it as the state it refused from: the variable part is the
/// state the stored trust is in. [`refusal`] appends the request id, once.
pub fn cannot_verify_trust(trust_state: &str) -> String {
    format!("Cannot verify trust in state: {trust_state}")
}

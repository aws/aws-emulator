// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-controltower-awscontroltowerapis service.
//!
//! The landing zone is Control Tower's primary resource and AWS allows exactly ONE per
//! account, so [`ComAmazonawsControltowerAwscontroltowerapisState::landing_zones`] holds
//! at most one entry and the one-per-account rule is read off its size rather than from a
//! separate flag. Decommissioning it "deletes Amazon Web Services Control Tower resources
//! deployed in accounts managed by Amazon Web Services Control Tower", so the enablements
//! those resources are do not outlive it: applying a decommissioning empties
//! `enabled_controls` and `enabled_baselines` too (see [`settle`]).
//!
//! Landing zone operations are a log keyed by `operationIdentifier`: a record outlives
//! the landing zone it acted on, so `GetLandingZoneOperation` still resolves the
//! identifier a `DeleteLandingZone` returned.
//!
//! Baselines work the same way one level down: an `EnabledBaseline` is a baseline applied
//! to a target, and `EnableBaseline`/`UpdateEnabledBaseline`/`DisableBaseline` each start
//! an asynchronous operation whose record lands in a SECOND log keyed by `operationIdentifier`
//! ([`ComAmazonawsControltowerAwscontroltowerapisState::baseline_operations`]), which
//! `GetBaselineOperation` resolves. The two logs are separate: an identifier a landing
//! zone call returned is not a baseline operation and vice versa.
//!
//! Control enablement keeps a log of its own, the same shape one level down again: a call
//! that enables, updates, resets or disables a control starts an asynchronous operation
//! whose record lands in
//! [`ComAmazonawsControltowerAwscontroltowerapisState::control_operations`], keyed by
//! `operationIdentifier` and resolved by `GetControlOperation`/`ListControlOperations`.
//! It is a THIRD separate log: an identifier a landing zone or baseline call returned is
//! not a control operation, and vice versa.
//!
//! The baseline CATALOG is a third kind of data: the baselines themselves are AWS-managed
//! and identical in every account, so they live in the constant [`BASELINE_CATALOG`] that
//! `GetBaseline` resolves an identifier against, not in the per-account store. No call
//! creates, changes or deletes a baseline, so nothing a caller does can grow it.
//!
//! CONTROLS are catalog data of exactly that kind: a control is AWS-managed and identical
//! in every account, so the controls a caller may name live in the constant
//! [`CONTROL_CATALOG`] that `EnableControl` resolves a `controlIdentifier` against, and no
//! call creates, changes or deletes one either.
//!
//! RETENTION: every store a caller can grow by repeating calls is BOUNDED, so ordinary
//! request traffic cannot grow this process's heap, or the per-request scan [`settle`]
//! runs under the state lock, without limit:
//!
//! * `landing_zones` holds the one landing zone an account may have;
//! * the three operation logs are append-only only WITHIN a bound, retaining at most
//!   [`MAX_LANDING_ZONE_OPERATIONS`] / [`MAX_BASELINE_OPERATIONS`] /
//!   [`MAX_CONTROL_OPERATIONS`] records and evicting the oldest TERMINAL record to make
//!   room (see [`enforce_log_bound`]);
//!
//! A log whose operations document how long an operation's details stay available is
//! bounded by that window as well, and the window is the stronger rule: a record older
//! than [`OPERATION_DETAILS_AVAILABLE_FOR`] is dropped by [`settle`], so every call that
//! reports one — the by-identifier read and the listing alike — answers the same way
//! about the same record rather than each handler testing the age itself.
//! * `enabled_baselines` holds at most [`MAX_ENABLED_BASELINES`] enablements, and
//!   `EnableBaseline` is REFUSED past that cap rather than growing the store (see
//!   [`record_enablement`]), because an enablement is a resource a caller must tear down
//!   rather than a history record a newer one may evict;
//! * `enabled_controls` holds at most [`MAX_ENABLED_CONTROLS`] control enablements — one
//!   per control/target pair, whether `EnableControl` was called for it or the service
//!   applied it to a target brought under its governance ([`apply_governance_controls`]) —
//!   and `EnableControl` is REFUSED past that cap for the same reason as `EnableBaseline`,
//!   while a service-applied control is simply not written past it;
//! * `tags` is keyed by the ARNs of the stores above, and an entry is dropped with the
//!   resource it tags, so it is bounded by them; one resource's tag map holds at most
//!   [`MAX_TAGS_PER_RESOURCE`] entries and `TagResource` is REFUSED past that count (see
//!   [`merge_tags`]), so repeating it with fresh keys cannot grow the store either, and
//!   [`remove_tags`] only ever shrinks it.
//!
//! The LIST operations over these stores all narrow their listing the same way, so the
//! rule lives here once rather than in each handler: [`Filter`] is one `filter` member as
//! a handler matches it, [`filter_set`] turns a sent member into that set and
//! [`selected`]/[`selected_optional`] test one stored record against it. Every list
//! operation of this crate reaches the rule through these, so narrowing by a filter
//! member cannot drift between them.
//!
//! They all PAGINATE the listing they narrowed the same way as well, so that rule lives
//! here once for the same reason: [`page`] cuts out the page a `nextToken`/`maxResults`
//! pair asks for and hands back the token that resumes the listing after it, and
//! [`TokenScope`] says how far a token one listing handed out reaches — a listing the
//! request narrows carries the parameters it was narrowed by ([`ListingKey`]), so its
//! token resumes only a call that repeats them. Every list operation of this crate
//! reaches the rule through [`page`], so a page boundary — and the refusal of a token no
//! page of that listing handed out — cannot drift between them either.
//!
//! A resource reported by BOTH a Get and a List operation projects its stored drift onto
//! the modelled drift summary here once, for the same reason: [`EnabledBaseline::drift_status_summary`]
//! is the `driftStatusSummary` an enabled baseline reports and
//! [`EnabledControl::drift_status_summary`] the one an enabled control reports. Both
//! reporting operations of each resource kind read the member through its projection, so
//! which stored status becomes which reported member — and whether the member is reported
//! at all — cannot drift between the detail and the listing.
//!
//! The enablement calls share one PRECONDITION, so it lives here once as well: Control
//! Tower serves them for an account it has been set up in, and [`is_set_up`] is that
//! question asked of the stores above. An account it has provisioned nothing in has no
//! landing zone and no service role for a call to reach enablement state through, so each
//! such READ refuses rather than reporting out of its empty store, and each call that would
//! enable, update, re-deploy or disable a control or a baseline refuses rather than writing
//! an enablement into it — in the words its resource family uses,
//! [`SETUP_REQUIRED_FOR_BASELINES`] for the baseline enablement calls and
//! [`SETUP_REQUIRED_FOR_CONTROLS`] for the control enablement and control-operation ones.
//!
//! [`is_set_up`]: ComAmazonawsControltowerAwscontroltowerapisState::is_set_up
//! [`merge_tags`]: ComAmazonawsControltowerAwscontroltowerapisState::merge_tags
//! [`remove_tags`]: ComAmazonawsControltowerAwscontroltowerapisState::remove_tags
//!
//! [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
//! [`record_enablement`]: ComAmazonawsControltowerAwscontroltowerapisState::record_enablement
//! [`apply_governance_controls`]: ComAmazonawsControltowerAwscontroltowerapisState::apply_governance_controls

/// How the TAG PLANE refuses an ARN it resolves to no resource, and the error channel its
/// three operations carry that refusal in: the plane's own wire vocabulary, which no
/// operation's generated error enum can hold.
mod tagging_refusal;

pub use tagging_refusal::{
    TaggingError, TaggingRefusal, TAGGING_NOT_FOUND_ERROR_CODE, TAGGING_NOT_FOUND_MESSAGE,
};

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

// The modelled drift-summary shapes the projections below build, so a reported drift
// summary is written out once for each resource kind rather than once per handler.
use crate::ops::types::{
    DriftStatusSummary, EnabledBaselineDriftStatusSummary, EnabledBaselineDriftTypes,
    EnabledBaselineInheritanceDrift, EnabledBaselineSourceDrift, EnabledControlDriftTypes,
    EnabledControlInheritanceDrift, EnabledControlResourceDrift,
};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, from the
/// model's `aws.api#service` trait (`arnNamespace: controltower`).
pub const ARN_NAMESPACE: &str = "controltower";

/// The region an ARN is built in when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long an asynchronous operation stays non-terminal before the clock owes it a
/// terminal status — a landing zone or baseline operation its `IN_PROGRESS`, a control
/// enablement its `UNDER_CHANGE`.
///
/// `CreateLandingZone`/`DeleteLandingZone` "start an asynchronous operation" and
/// `EnableControl` leaves the control `UNDER_CHANGE` ("the EnabledControl configuration
/// is changing", model docs), so a caller must be able to observe the transient status.
/// AWS takes tens of minutes, which no emulator can usefully reproduce and which AWS
/// does not document as a contract, so this span is CHOSEN — kept short enough that a
/// test can wait for it and long enough that the call that starts the operation always
/// returns before it elapses.
pub const OPERATION_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How long an asynchronous operation's details stay available, after which the record is
/// no longer reported at all.
///
/// Documented by the operations that report a record: `GetLandingZoneOperation` and
/// `GetControlOperation` both state that "Details for an operation are available for 90
/// days", and `ListLandingZoneOperations` lists "all landing zone operations from the past
/// 90 days" — one window over the same records. It is measured from the instant the
/// operation STARTED, which is what the listing's own wording ("from the past 90 days")
/// dates a record by, so a record leaves the window once that instant is this far back.
///
/// Applied in [`settle`], so it bounds the store itself rather than one reader: a
/// by-identifier read and a listing cannot answer differently about one record.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
pub const OPERATION_DETAILS_AVAILABLE_FOR: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The most landing zone operation records the log retains.
///
/// A record stays resolvable through `GetLandingZoneOperation` until its details stop
/// being available ([`OPERATION_DETAILS_AVAILABLE_FOR`]) or the log is full, whichever
/// comes first; when the log is full the OLDEST terminal record is evicted to make room.
/// This cap is a STORE bound, not the documented retention: without it a caller repeating
/// create/delete inside the availability window would grow the store, and the `settle`
/// scan held under the state lock, without limit. AWS publishes no count for this log, so
/// the value is CHOSEN: large enough that no lifecycle an emulator client drives reaches
/// it, small enough to bound the store.
pub const MAX_LANDING_ZONE_OPERATIONS: usize = 1024;

/// The most enabled controls this account's store holds.
///
/// `EnableControl` is the one call a caller can repeat to grow the enabled-control map
/// (each distinct control/target pair is a new entry), so the map is bounded and the
/// call past the bound is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402) rather than growing the store. AWS publishes per-OU control quotas but
/// not one number for an account, so this value is CHOSEN: far above any lifecycle an
/// emulator client drives, small enough to bound the store and the per-request scan
/// `ListEnabledControls` runs under the state lock.
pub const MAX_ENABLED_CONTROLS: usize = 1024;

/// The most bytes one `EnableControl` request's `parameters` may weigh, keys and
/// documents together.
///
/// The declared-key check ([`unacceptable_control_parameters_message`]) already bounds the
/// parameter COUNT of an enablement — a control declares a handful of keys and each may be
/// given once — but an `EnabledControlParameter$value` is an unconstrained `Document`, so
/// one parameter can still carry a whole body. The weight is measured with
/// [`document_weight_within`], which stops as soon as this cap is exceeded, and a request
/// above it is refused with the modelled `ValidationException`. CHOSEN: orders of
/// magnitude above any real control configuration, and small enough that
/// [`MAX_ENABLED_CONTROLS`] records bound the store's size in bytes as well as in entries.
pub const MAX_ENABLED_CONTROL_PARAMETER_BYTES: usize = 64 * 1024;

/// The most control operation records the log retains.
///
/// Same reasoning, and the same CHOSEN size, as [`MAX_LANDING_ZONE_OPERATIONS`]: a record
/// stays resolvable through `GetControlOperation`/`ListControlOperations` until its details
/// stop being available ([`OPERATION_DETAILS_AVAILABLE_FOR`], which `GetControlOperation`
/// documents too) or the log is full, at which point the oldest terminal record is
/// evicted. Counted separately because
/// the three logs are separate stores, so a busy control caller cannot evict landing zone
/// or baseline history.
pub const MAX_CONTROL_OPERATIONS: usize = 1024;

/// The most baseline operation records the log retains.
///
/// Same reasoning, and the same CHOSEN size, as [`MAX_LANDING_ZONE_OPERATIONS`]: a record
/// stays resolvable through `GetBaselineOperation` until the log is full, at which point
/// the oldest terminal record is evicted. Counted separately because the two logs are
/// separate stores, so a busy baseline caller cannot evict landing zone history.
///
/// This log is bounded by the COUNT only: `GetBaselineOperation` is the one reporting call
/// of the three logs whose docs state no availability window for the details it reports,
/// so nothing times a baseline operation record out.
pub const MAX_BASELINE_OPERATIONS: usize = 1024;

/// The most `EnabledBaseline` resources this account may hold at once.
///
/// Unlike an operation log, this store holds RESOURCES: a caller tears an enablement down
/// with `DisableBaseline`, so nothing here may be evicted to make room for a newer one
/// without losing a resource its owner still has. `EnableBaseline` is therefore refused
/// past this cap with the `ServiceQuotaExceededException` the model declares for it ("The
/// request would cause a service quota to be exceeded"), which both keeps the store
/// — and the per-request [`settle`] scan — bounded and is the answer AWS gives when an
/// account is at its limit. AWS publishes its own Control Tower quotas; this value is
/// CHOSEN: far above any lifecycle an emulator client drives, small enough to bound the
/// store.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
pub const MAX_ENABLED_BASELINES: usize = 512;

/// The most bytes one baseline enablement's `parameters` may weigh, keys and documents
/// together.
///
/// The declared-key check below already bounds the parameter COUNT of an enablement (a
/// baseline declares a handful of keys and each may be given once), but an
/// `EnabledBaselineParameter$value` is an unconstrained `Document`, so one parameter can
/// still carry a whole body. The weight is measured with [`document_weight_within`],
/// which stops as soon as this cap is exceeded, and a request above it is refused with
/// the modelled `ValidationException`. CHOSEN, for the same reason and at the same size
/// as [`MAX_ENABLED_CONTROL_PARAMETER_BYTES`]: orders of magnitude above any real
/// baseline configuration, and small enough that [`MAX_ENABLED_BASELINES`] records bound
/// the store's size in bytes as well as in entries.
pub const MAX_ENABLED_BASELINE_PARAMETER_BYTES: usize = 64 * 1024;

/// The landing zone versions this service OFFERS, oldest first.
///
/// `LandingZoneVersion` (len 3..10, pattern `^\d+.\d+$`) bounds the SHAPE of a version
/// string, not the set of versions that exist, and the member's docs give one by example
/// ("The landing zone version, for example, 3.0"): a landing zone is created or updated
/// at a version the service publishes, so a value outside this set is refused with the
/// modelled `ValidationException` rather than minted. The set is the 2.x/3.x line the AWS
/// Control Tower user guide's "Landing zone versions" table publishes — 2.0 through 3.3.
///
/// Every entry satisfies `LandingZoneVersion`, and the list is kept in NUMERIC order so
/// its last entry is [`CATALOG_LANDING_ZONE_VERSION`].
pub const CATALOG_LANDING_ZONE_VERSIONS: &[&str] = &[
    "2.0", "2.1", "2.2", "2.3", "2.4", "2.5", "2.6", "2.7", "2.8", "2.9", "3.0", "3.1", "3.2",
    "3.3",
];

/// The most tags one resource may carry.
///
/// This is the model's OWN ceiling rather than a chosen one: `TagMap` carries
/// `@length(min: 0, max: 200)`, and `ListTagsForResource` reports a resource's tags as a
/// `TagMap`, so a resource holding more than 200 tags could not be reported inside the
/// modelled constraint. `TagResource` is therefore refused with the `ValidationException`
/// the model declares for it once the merged set would pass this count, which also bounds
/// the tag store: a caller cannot grow one resource's tag map by repeating calls with
/// fresh keys.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The tag-key prefix Amazon Web Services keeps for the tags it applies itself.
///
/// A key under this prefix names a tag Amazon Web Services generates, so it is not a
/// caller's to create: the AWS tag restrictions reserve the prefix for Amazon Web
/// Services' own use in ANY letter case, which is why [`is_reserved_tag_key`] compares it
/// case-insensitively. The modelled `TagKey` (len 1..128) can express nothing of this, so
/// it is the handler's rule: every request that WRITES tags refuses one naming such a key
/// with the `ValidationException` the model declares for it, and writes nothing — a key
/// Amazon Web Services reserves can be neither created nor deleted by a caller.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// A PREFIX test, in any letter case: only a key that OPENS with the prefix is reserved,
/// so a key that merely contains it (`team:aws:cost`) or that starts with the letters
/// without the separator (`awsome`) is an ordinary key a caller may create.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Why the tag keys a request would write cannot be accepted, or `None` when none of them
/// is reserved.
///
/// The ONE place this refusal is decided and worded, reached by every operation of this
/// service that writes tags (`TagResource`, `UntagResource` and the `tags` member of
/// `CreateLandingZone`, `EnableControl` and `EnableBaseline`), so the rule cannot drift
/// between them. The message names the member it is stated against and the offending key;
/// the SMALLEST offending key is the one named, so a request carrying several reserved keys
/// is refused in the same words on every call rather than naming whichever one a hash map
/// happened to yield first.
pub fn reserved_tag_key_message<'a, I>(member: &str, keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let offending = keys.into_iter().filter(|key| is_reserved_tag_key(key)).min()?;
    Some(format!(
        "{member} carries tag key {offending:?}, which opens with the reserved \
         {RESERVED_TAG_KEY_PREFIX} prefix; keys under that prefix name tags Amazon Web \
         Services applies itself and cannot be created, modified or removed"
    ))
}

/// The newest landing zone version this emulator's catalog offers.
///
/// Reported through `LandingZoneDetail$latestAvailableVersion` ("The latest available
/// version of the landing zone"). Satisfies `LandingZoneVersion` (len 3..10, pattern
/// `^\d+.\d+$`), and is the numerically newest entry of [`CATALOG_LANDING_ZONE_VERSIONS`].
pub const CATALOG_LANDING_ZONE_VERSION: &str = "3.3";

/// Whether this service offers `version` as a landing zone version.
///
/// The membership test behind every `version` refusal: a landing zone can only be created
/// or updated at a version the catalog publishes. Private because a handler refuses with
/// [`unsupported_version_message`], which is this test plus the wording.
fn offers_landing_zone_version(version: &str) -> bool {
    CATALOG_LANDING_ZONE_VERSIONS.contains(&version)
}

/// Why `version` cannot be deployed, or `None` when the catalog offers it.
///
/// The one place the refusal's wording is built, so `CreateLandingZone` and
/// `UpdateLandingZone` refuse an unoffered version in the same words. The message is
/// built from the request and from the catalog, so it tells the caller which versions it
/// may ask for.
pub fn unsupported_version_message(version: &str) -> Option<String> {
    if offers_landing_zone_version(version) {
        return None;
    }
    Some(format!(
        "version {version:?} is not a landing zone version this service offers; the \
         available versions are {}",
        CATALOG_LANDING_ZONE_VERSIONS.join(", ")
    ))
}

/// Why `requested` cannot be deployed over `deployed`, or `None` when it does not move
/// the landing zone backwards.
///
/// A landing zone is updated FORWARD only: an update deploys the requested version over
/// the version the landing zone already has, so the same version (a re-apply of the
/// configuration at that version) or a numerically newer one is a configuration the
/// service can reach from there, while an older one is not — the landing zone would have
/// to be un-deployed to reach it. Versions are ordered per component numerically, the
/// same order [`latest_available_version`] uses, so `3.10` is newer than `3.3`.
///
/// A value neither side can order (the modelled `LandingZoneVersion` pattern
/// `^\d+.\d+$` leaves its separator unescaped, so it admits some) is not refused here:
/// nothing establishes that it is older. The one place this refusal's wording is built,
/// and the message is built from the request and from the landing zone's own version, so
/// it tells the caller what it may ask for.
pub fn version_downgrade_message(deployed: &str, requested: &str) -> Option<String> {
    let (Some(deployed_order), Some(requested_order)) =
        (version_order(deployed), version_order(requested))
    else {
        return None;
    };
    if requested_order >= deployed_order {
        return None;
    }
    Some(format!(
        "version {requested:?} is older than the landing zone's version {deployed:?}; a \
         landing zone can only be updated to the same or a newer version"
    ))
}

/// The length window the modelled `RegionName` bounds a Region name by
/// (`smithy_query query="shape" shape="Region,RegionName"`: `Region$name` targets
/// `RegionName`, whose length is 1..50).
pub const REGION_NAME_LENGTH: ::std::ops::RangeInclusive<usize> = 1..=50;

/// Whether `name` is an Amazon Web Services Region name.
///
/// A Region name is hyphen-separated groups of lowercase ASCII letters — the Region's
/// geography and its position within it — closed by the Region's number (`us-west-2`,
/// `eu-central-1`, `ap-southeast-4`, `us-gov-east-1`), and it is also bounded by
/// [`REGION_NAME_LENGTH`], the window the member this service reports a Region name
/// through declares. A value outside either is not a Region name this service could
/// report, so it is not a Region name it accepts.
pub fn is_region_name(name: &str) -> bool {
    if !REGION_NAME_LENGTH.contains(&name.len()) {
        return false;
    }
    // The number closes the name, and the geography and position groups precede it.
    let Some((position, number)) = name.rsplit_once('-') else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    position.split('-').count() >= 2
        && position.split('-').all(|group| {
            !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_lowercase())
        })
}

/// What one documented `Manifest` member's value must be.
///
/// The manifest's full schema "is not published, by design"
/// (`UpdateLandingZoneInput$manifest`), so a member is constrained only as far as the
/// user guide's example manifest documents it ("Launch your landing zone") and as far as
/// the member this service REPORTS its value through declares: its JSON type, and for a
/// value with a form of its own — an account id, a Region name — that form. Anything the
/// manifest carries beyond these members is accepted and stored unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestMemberKind {
    /// A JSON object, whose own members are not constrained further.
    Object,
    /// A JSON array of Amazon Web Services Region names, each one a name
    /// [`is_region_name`] accepts: this service reports these back through a member the
    /// model length-bounds, so a string no Region name could be is not one of them. May
    /// be empty: a manifest that governs no region leaves the governed-region list empty
    /// rather than omitting it.
    RegionNameList,
    /// A 12-digit Amazon Web Services account id, as a JSON string.
    AccountId,
    /// A JSON boolean.
    Boolean,
    /// A JSON object carrying a `name` string: an organizational unit's name.
    NamedOrganizationalUnit,
}

impl ManifestMemberKind {
    /// Whether `value` is of this kind.
    pub fn accepts(self, value: &::aws_smithy_types::Document) -> bool {
        self.rejection(value).is_none()
    }

    /// What `value` is, when it is not of this kind — the "got …" clause of the refusal
    /// that reports it — or `None` when `value` is of this kind.
    ///
    /// A list's ENTRIES are part of what the kind requires, so a list of the right JSON
    /// type holding a value the kind does not carry is reported against that entry: the
    /// caller is told which value to change rather than that the array it sent is an
    /// array.
    pub fn rejection(self, value: &::aws_smithy_types::Document) -> Option<String> {
        use ::aws_smithy_types::Document;

        match self {
            Self::Object => match value {
                Document::Object(_) => None,
                other => Some(document_kind(other).to_string()),
            },
            Self::RegionNameList => match value {
                Document::Array(items) => items.iter().find_map(|item| match item {
                    Document::String(name) if is_region_name(name) => None,
                    Document::String(name) => Some(format!(
                        "the entry {name:?}, which is not an Amazon Web Services Region name"
                    )),
                    other => Some(format!("an entry that is {}", document_kind(other))),
                }),
                other => Some(document_kind(other).to_string()),
            },
            Self::AccountId => match value {
                Document::String(account_id)
                    if account_id.len() == 12
                        && account_id.bytes().all(|byte| byte.is_ascii_digit()) =>
                {
                    None
                }
                other => Some(document_kind(other).to_string()),
            },
            Self::Boolean => match value {
                Document::Bool(_) => None,
                other => Some(document_kind(other).to_string()),
            },
            Self::NamedOrganizationalUnit => match value {
                Document::Object(members)
                    if matches!(members.get("name"), Some(Document::String(_))) =>
                {
                    None
                }
                other => Some(document_kind(other).to_string()),
            },
        }
    }

    /// What this kind requires, for the refusal's message.
    pub fn requirement(self) -> &'static str {
        match self {
            Self::Object => "a JSON object",
            Self::RegionNameList => "a JSON array of Amazon Web Services Region names",
            Self::AccountId => "a 12-digit Amazon Web Services account id, as a string",
            Self::Boolean => "a JSON boolean",
            Self::NamedOrganizationalUnit => {
                "a JSON object carrying the organizational unit's `name` as a string"
            }
        }
    }
}

/// The members a landing zone `Manifest` must declare, as dotted paths into the document.
///
/// These are the required parameters of the example manifest the user guide publishes for
/// `CreateLandingZone`/`UpdateLandingZone` ("Launch your landing zone"): the governed
/// Regions, the organization structure's security OU, the centralized logging account and
/// its configuration, the security roles account, and whether access management is
/// enabled. A create or update whose manifest omits one of them describes no landing zone
/// the service could deploy, so it is refused with the modelled `ValidationException`.
///
/// A parent path precedes its children, so a member whose container is of the wrong type
/// is reported against the CONTAINER rather than as a missing child.
///
/// The nested entries (`centralizedLogging`'s account, configuration and flag,
/// `securityRoles`' account, `accessManagement`'s flag) are here because the user guide
/// lists them among the manifest's REQUIRED parameters, not merely because its example
/// happens to carry them — a section present but hollow still describes nothing the
/// service could deploy. Only these paths are checked, and only for their JSON kind:
/// everything else the manifest carries is accepted and stored unchanged, because the
/// schema as a whole is unpublished.
pub const REQUIRED_MANIFEST_MEMBERS: &[(&str, ManifestMemberKind)] = &[
    ("governedRegions", ManifestMemberKind::RegionNameList),
    ("organizationStructure", ManifestMemberKind::Object),
    (
        "organizationStructure.security",
        ManifestMemberKind::NamedOrganizationalUnit,
    ),
    ("centralizedLogging", ManifestMemberKind::Object),
    ("centralizedLogging.accountId", ManifestMemberKind::AccountId),
    (
        "centralizedLogging.configurations",
        ManifestMemberKind::Object,
    ),
    ("centralizedLogging.enabled", ManifestMemberKind::Boolean),
    ("securityRoles", ManifestMemberKind::Object),
    ("securityRoles.accountId", ManifestMemberKind::AccountId),
    ("accessManagement", ManifestMemberKind::Object),
    ("accessManagement.enabled", ManifestMemberKind::Boolean),
];

/// Manifest members the documented example carries OPTIONALLY: absent is fine, but a
/// value of the wrong type is not.
///
/// The sandbox OU is optional in the example manifest (a landing zone may be launched
/// without one), unlike the security OU.
pub const OPTIONAL_MANIFEST_MEMBERS: &[(&str, ManifestMemberKind)] = &[(
    "organizationStructure.sandbox",
    ManifestMemberKind::NamedOrganizationalUnit,
)];

/// The member `path` names, resolved through nested objects, or `None` when any step of
/// the path is absent or is not an object.
fn manifest_member<'a>(
    manifest: &'a ::aws_smithy_types::Document,
    path: &str,
) -> Option<&'a ::aws_smithy_types::Document> {
    use ::aws_smithy_types::Document;

    let mut node = manifest;
    for segment in path.split('.') {
        let Document::Object(members) = node else {
            return None;
        };
        node = members.get(segment)?;
    }
    Some(node)
}

/// The JSON kind of `document`, for a refusal's message.
fn document_kind(document: &::aws_smithy_types::Document) -> &'static str {
    use ::aws_smithy_types::Document;

    match document {
        Document::Object(_) => "an object",
        Document::Array(_) => "an array",
        Document::String(_) => "a string",
        Document::Number(_) => "a number",
        Document::Bool(_) => "a boolean",
        Document::Null => "null",
    }
}

/// The first way `manifest` fails to describe a landing zone configuration, or `None`
/// when it describes one.
///
/// The message names the member and the constraint it failed, so a caller is told what to
/// change; it is built from the request, never from a recorded example.
pub fn manifest_defect(manifest: &::aws_smithy_types::Document) -> Option<String> {
    use ::aws_smithy_types::Document;

    if !matches!(manifest, Document::Object(_)) {
        return Some(format!(
            "manifest must be a JSON object describing the landing zone configuration, got {}",
            document_kind(manifest)
        ));
    }

    for (path, kind) in REQUIRED_MANIFEST_MEMBERS {
        match manifest_member(manifest, path) {
            None => {
                return Some(format!(
                    "manifest is missing the required member {path:?}, which must be {}",
                    kind.requirement()
                ))
            }
            Some(value) => {
                if let Some(rejection) = kind.rejection(value) {
                    return Some(format!(
                        "manifest member {path:?} must be {}, got {rejection}",
                        kind.requirement()
                    ));
                }
            }
        }
    }

    for (path, kind) in OPTIONAL_MANIFEST_MEMBERS {
        if let Some(value) = manifest_member(manifest, path) {
            if let Some(rejection) = kind.rejection(value) {
                return Some(format!(
                    "manifest member {path:?} must be {}, got {rejection}",
                    kind.requirement()
                ));
            }
        }
    }

    None
}

/// `LandingZoneStatus`: the landing zone's deployment status.
///
/// Wire values from the model's `LandingZoneStatus` enum
/// (`ACTIVE` | `PROCESSING` | `FAILED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneStatus {
    Active,
    Processing,
    Failed,
}

impl LandingZoneStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Processing => "PROCESSING",
            Self::Failed => "FAILED",
        }
    }
}

/// `LandingZoneOperationType`: which call produced an operation record.
///
/// Wire values from the model's `LandingZoneOperationType` enum
/// (`CREATE` | `DELETE` | `RESET` | `UPDATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneOperationType {
    Create,
    Delete,
    Reset,
    Update,
}

impl LandingZoneOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Delete => "DELETE",
            Self::Reset => "RESET",
            Self::Update => "UPDATE",
        }
    }
}

/// `LandingZoneOperationStatus`: how far an operation has got.
///
/// Wire values from the model's `LandingZoneOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl LandingZoneOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// `LandingZoneDriftStatus`: whether a landing zone still matches its manifest.
///
/// Wire values from the model's `LandingZoneDriftStatus` enum, reported through
/// `LandingZoneDriftStatusSummary$status` (`DRIFTED` | `IN_SYNC`). Typed like the other
/// three modelled enums so the store cannot hold a drift value the model does not
/// declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingZoneDriftStatus {
    Drifted,
    InSync,
}

impl LandingZoneDriftStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Drifted => "DRIFTED",
            Self::InSync => "IN_SYNC",
        }
    }
}

/// A landing zone configuration an asynchronous operation has started deploying but the
/// clock has not yet made due.
///
/// `UpdateLandingZone` "starts an asynchronous operation that updates the landing zone"
/// (model docs) and `LandingZoneDetail$version` is "the landing zone's CURRENT DEPLOYED
/// version", so the configuration a caller asked for is NOT what a reporting call reports
/// while that operation is in flight: it is held here and deployed by [`settle`] at
/// `due_at`, the same instant the operation record reaches its terminal status — unless
/// that status is FAILED, since an operation that failed deployed nothing. The same holds
/// for the configuration a `CREATE` and a `RESET` start deploying, which is why those two
/// park theirs here too: what a deployment in flight has not deployed yet — above all the
/// drift assessment the deployed configuration earns — is not reported until the clock
/// makes that deployment due.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
#[derive(Debug, Clone)]
pub struct PendingLandingZoneConfiguration {
    /// The `CREATE`/`UPDATE`/`RESET` operation whose completion deploys this configuration.
    pub operation_identifier: String,
    pub version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    /// The drift status the deployed landing zone will carry once this configuration is
    /// deployed.
    pub drift_status: Option<LandingZoneDriftStatus>,
    /// The instant the clock owes this deployment — the same instant that operation
    /// becomes due, held here so applying it never depends on the operation's record still
    /// being in the (bounded) log. Stored, never slept on: a reader materialises the
    /// transition once it is due.
    pub due_at: SystemTime,
}

/// The configuration a call asks a landing zone to deploy, as its handler worked it out:
/// everything a [`PendingLandingZoneConfiguration`] holds except the identifier of the
/// operation that deploys it and the instant the clock owes it, both of which belong to
/// the call that parks it ([`record_landing_zone_configuration`]).
///
/// [`record_landing_zone_configuration`]: ComAmazonawsControltowerAwscontroltowerapisState::record_landing_zone_configuration
#[derive(Debug, Clone)]
pub struct LandingZoneConfiguration {
    pub version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    /// The drift status the deployed landing zone will carry once this configuration is
    /// deployed.
    pub drift_status: Option<LandingZoneDriftStatus>,
}

/// A landing zone decommissioning a `DELETE` has started and the clock has not yet made
/// due.
///
/// `DeleteLandingZone` "starts an asynchronous operation" that decommissions the landing
/// zone, so the resource does not disappear when that call returns: it stays a reportable
/// resource, `PROCESSING`, until [`settle`] removes it at `due_at` — the same instant the
/// `DELETE` record reaches its terminal status — unless that status is FAILED, since a
/// teardown that failed removed nothing.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
#[derive(Debug, Clone)]
pub struct PendingLandingZoneDeletion {
    /// The `DELETE` operation whose completion removes the landing zone.
    pub operation_identifier: String,
    /// The instant the clock owes this removal — the same instant that operation becomes
    /// due, held here so applying it never depends on the operation's record still being
    /// in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
}

/// `DriftStatus`: whether an enabled control still matches the configuration Control
/// Tower deployed.
///
/// Wire values from the model's `DriftStatus` enum, reported through
/// `DriftStatusSummary$driftStatus` and the two `EnabledControlDriftTypes` members
/// (`DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftStatus {
    Drifted,
    InSync,
    NotChecking,
    Unknown,
}

impl DriftStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Drifted => "DRIFTED",
            Self::InSync => "IN_SYNC",
            Self::NotChecking => "NOT_CHECKING",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// `EnabledBaselineDriftStatus`: whether what an enablement has deployed still matches the
/// configuration it is measured against.
///
/// Wire values from the model's `EnabledBaselineDriftStatus` enum, reported through both
/// drift dimensions the model declares for a baseline —
/// `EnabledBaselineInheritanceDrift$status` and `EnabledBaselineSourceDrift$status`
/// (`IN_SYNC` | `DRIFTED`). It is a DIFFERENT enum
/// from the [`DriftStatus`] an enabled control carries: a baseline's drift is only ever
/// in-sync or drifted, with no "unknown" or "not checking" value, so an enablement whose
/// drift is not assessed reports no drift summary at all rather than a third status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnabledBaselineDriftStatus {
    InSync,
    Drifted,
}

impl EnabledBaselineDriftStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InSync => "IN_SYNC",
            Self::Drifted => "DRIFTED",
        }
    }
}

/// A persisted enabled control: one control applied to one target.
///
/// Everything `ListEnabledControls` and `GetEnabledControl` report about a control is
/// read from here, so the two agree by construction. `parameters` keeps the caller's
/// `EnabledControlParameter` documents in the order they were supplied. The regions a
/// control is deployed into are NOT stored here: they follow the landing zone's current
/// governance, so they are derived on every read by [`target_regions_of`].
///
/// [`target_regions_of`]: ComAmazonawsControltowerAwscontroltowerapisState::target_regions_of
#[derive(Debug, Clone)]
pub struct EnabledControl {
    pub arn: String,
    pub control_identifier: String,
    pub target_identifier: String,
    /// `EnabledControlSummary$parentIdentifier` — "the ARN of the parent enabled control
    /// from which this control inherits its configuration, if applicable". The one fact
    /// that makes an enablement a CHILD of another: the reporting operations read the
    /// member off here, and [`listed_enabled_controls`] decides from these links alone
    /// which enablements a listing's `includeChildren` widens it by.
    ///
    /// A control `EnableControl` applied directly to its target inherits from nothing, so
    /// it is `None` for every control a caller enables here: a child enablement is one
    /// Control Tower derives from the organization's membership — a deployment into a
    /// member account or child unit of a target organizational unit — and no call this
    /// service serves states that one target contains another, nor does any rule derive
    /// containment from a unit's opaque identifier.
    ///
    /// [`listed_enabled_controls`]: ComAmazonawsControltowerAwscontroltowerapisState::listed_enabled_controls
    pub parent_identifier: Option<String>,
    /// `EnabledControlParameter`s as supplied, in order.
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
    /// The region the request that enabled this control was signed for. Only the
    /// FALLBACK for the regions a caller is told about, used while the account governs no
    /// region at all; the governed set, when there is one, is what
    /// [`target_regions_of`] reports.
    ///
    /// [`target_regions_of`]: ComAmazonawsControltowerAwscontroltowerapisState::target_regions_of
    pub enablement_region: String,
    pub status: EnablementStatus,
    /// `EnablementStatusSummary$lastOperationIdentifier`: the identifier of the most
    /// recent control operation against this control.
    pub last_operation_identifier: String,
    pub drift_status: DriftStatus,
    /// `EnabledControlDriftTypes$inheritance`.
    pub inheritance_drift_status: DriftStatus,
    /// `EnabledControlDriftTypes$resource`.
    pub resource_drift_status: DriftStatus,
    /// The instant the clock owes this control its terminal enablement status. Stored,
    /// never slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
    /// The configuration an `UPDATE_ENABLED_CONTROL` still in flight will apply, or
    /// `None` when nothing is pending. [`settle`] applies it once that operation is due,
    /// so the configured parameters change when the asynchronous update completes and not
    /// when the call that asked for it returned.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_update: Option<PendingEnabledControlUpdate>,
    /// The teardown a `DISABLE_CONTROL` still in flight will complete, or `None` while no
    /// disable is running. [`settle`] removes the enablement once the clock makes it due,
    /// so the resource stops existing when the asynchronous teardown completes and not
    /// when the call that asked for it returned; it TAKES the teardown at that instant, so
    /// a teardown the clock has already settled is never in flight any more and its
    /// outcome is never re-decided.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_deletion: Option<PendingEnabledControlDeletion>,
}

impl EnabledControl {
    /// Withdraw the drift assessment this control carries: every dimension reads `UNKNOWN`,
    /// the value for which the service "is not able to check the drift status".
    ///
    /// Drift is a property of a DEPLOYED configuration, so every call that starts changing
    /// what this control has deployed — a new configuration, a re-deployment of the one in
    /// place, or its removal — leaves the assessment withdrawn until the clock makes that
    /// operation due and [`settle`] assesses what is deployed then.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    fn withdraw_drift_assessment(&mut self) {
        self.drift_status = DriftStatus::Unknown;
        self.inheritance_drift_status = DriftStatus::Unknown;
        self.resource_drift_status = DriftStatus::Unknown;
    }

    /// The `driftStatusSummary` this control reports: its overall [`DriftStatus`] plus one
    /// member of `types` per drift dimension the model declares for a control
    /// (`inheritance` and `resource`), each read off the stored status for that dimension.
    ///
    /// Every drift dimension of a control always carries a status — `UNKNOWN` is the value
    /// for a dimension the service "is not able to check" — so the summary and both of its
    /// members are always reported. `GetEnabledControl` and `ListEnabledControls` both
    /// report the member through here, so they cannot disagree about one stored record.
    pub fn drift_status_summary(&self) -> DriftStatusSummary {
        DriftStatusSummary {
            drift_status: Some(self.drift_status.as_wire().to_string()),
            types: Some(EnabledControlDriftTypes {
                inheritance: Some(EnabledControlInheritanceDrift {
                    status: Some(self.inheritance_drift_status.as_wire().to_string()),
                }),
                resource: Some(EnabledControlResourceDrift {
                    status: Some(self.resource_drift_status.as_wire().to_string()),
                }),
            }),
        }
    }
}

/// A control teardown a `DISABLE_CONTROL` has started and the clock has not yet made due.
///
/// `DisableControl` "starts an asynchronous operation that deletes Amazon Web Services
/// resources on the specified organizational unit and the accounts it contains", so the
/// enablement does not disappear when that call returns: it stays a reportable resource,
/// `UNDER_CHANGE`, until [`settle`] removes it at `due_at` — the same instant the
/// `DISABLE_CONTROL` record reaches its terminal status.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
#[derive(Debug, Clone)]
pub struct PendingEnabledControlDeletion {
    /// The `DISABLE_CONTROL` operation whose completion removes the enablement.
    pub operation_identifier: String,
    /// The instant the clock owes this removal — the same instant that operation becomes
    /// due, held here so applying it never depends on the operation's record still being
    /// in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
}

/// A configuration `UpdateEnabledControl` accepted and the clock has yet to apply.
///
/// `UpdateEnabledControl` "updates the configuration of an already enabled control"
/// through an asynchronous operation, so the request's parameters are parked here and
/// become the control's configured ones once the clock has made that operation due.
#[derive(Debug, Clone)]
pub struct PendingEnabledControlUpdate {
    /// The `UPDATE_ENABLED_CONTROL` operation whose completion applies this change.
    pub operation_identifier: String,
    /// The instant the clock owes this change its application — the same instant that
    /// operation becomes due, held here so applying it never depends on the operation's
    /// record still being in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
    /// The configured parameters the update replaces the control's with. Required on the
    /// request, so an accepted update always carries a (possibly empty) list.
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
}

/// A persisted landing zone. Everything `GetLandingZone` reports is read from here.
///
/// `manifest` keeps the caller's `Manifest` document EXACTLY as it was deserialized
/// (no re-encoding through another representation), so what `GetLandingZone` returns is
/// the document `CreateLandingZone` was given.
#[derive(Debug, Clone)]
pub struct LandingZone {
    pub arn: String,
    pub version: String,
    pub latest_available_version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    pub status: LandingZoneStatus,
    /// `LandingZoneDriftStatusSummary$status`, absent when the service has not assessed
    /// drift for this landing zone.
    pub drift_status: Option<LandingZoneDriftStatus>,
    /// The configuration an in-flight `UPDATE`/`RESET` will deploy, `None` when the
    /// deployed configuration is the only one. Deployed by [`settle`] once the clock makes
    /// it due, unless the operation it carries FAILED.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_configuration: Option<PendingLandingZoneConfiguration>,
    /// The decommissioning an in-flight `DELETE` will complete, `None` while no delete is
    /// running. [`settle`] removes the landing zone once the clock makes it due — unless
    /// the operation it carries FAILED, since a teardown that failed removed nothing — so
    /// the resource stops existing when the asynchronous delete completes and not when the
    /// call that asked for it returned.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_deletion: Option<PendingLandingZoneDeletion>,
    pub created_at: SystemTime,
    /// The instant the landing zone's DEPLOYED configuration last changed — its creation
    /// instant until a deployment leaves it configured differently, and then the instant
    /// the clock owed that deployment.
    ///
    /// What the enablements deployed under this landing zone are measured against: a
    /// baseline enablement is in source drift "when an enabled baseline no longer matches
    /// the landing zone's configuration" (model docs), and what an enablement has deployed
    /// is the landing zone's configuration as of the instant the clock deployed it. Stored,
    /// never slept on, and moved only by a deployment [`settle`] applies, so no reader
    /// changes what a later reader sees.
    ///
    /// A deployment that re-applies the configuration already in place (a `RESET`, or an
    /// `UPDATE` carrying no new value) leaves it where it is: nothing the enablements follow
    /// has changed.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub configuration_changed_at: SystemTime,
}

impl LandingZone {
    /// The newest version this landing zone has accepted: the one an in-flight `UPDATE`
    /// is deploying while there is one, otherwise the deployed one.
    ///
    /// A further update is ordered against THIS rather than against `version`, so a
    /// version already on its way to being deployed cannot be walked backwards by a
    /// second call that arrives before the first is due. REACHED on the wire path:
    /// `UpdateLandingZone` settles what the request asks for before it looks at whether
    /// the landing zone has an operation in flight, so a move backwards is refused with
    /// `ValidationException` whether or not the clock has made the deployment in flight
    /// due — once it has, [`settle`] has applied it and the deployed branch answers the
    /// same question.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub fn configured_version(&self) -> &str {
        match &self.pending_configuration {
            Some(pending) => &pending.version,
            None => &self.version,
        }
    }
}

/// One entry of the landing zone operation log.
#[derive(Debug, Clone)]
pub struct LandingZoneOperation {
    pub operation_identifier: String,
    pub operation_type: LandingZoneOperationType,
    /// The landing zone this operation acted on. Kept even after the landing zone is
    /// deleted, so a `DELETE` identifier still resolves.
    pub landing_zone_arn: String,
    pub status: LandingZoneOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// `BaselineOperationType`: which call produced a baseline operation record.
///
/// Wire values from the model's `BaselineOperationType` enum (`ENABLE_BASELINE` |
/// `DISABLE_BASELINE` | `UPDATE_ENABLED_BASELINE` | `RESET_ENABLED_BASELINE`), which is
/// exactly the set of calls `GetBaselineOperation` documents as starting one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineOperationType {
    EnableBaseline,
    DisableBaseline,
    UpdateEnabledBaseline,
    ResetEnabledBaseline,
}

impl BaselineOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnableBaseline => "ENABLE_BASELINE",
            Self::DisableBaseline => "DISABLE_BASELINE",
            Self::UpdateEnabledBaseline => "UPDATE_ENABLED_BASELINE",
            Self::ResetEnabledBaseline => "RESET_ENABLED_BASELINE",
        }
    }
}

/// `BaselineOperationStatus`: how far a baseline operation has got.
///
/// Wire values from the model's `BaselineOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl BaselineOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// `EnablementStatus`: the deployment status of an enabled resource.
///
/// Wire values from the model's `EnablementStatus` enum, reported through
/// `EnablementStatusSummary$status` (`SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, where
/// `UNDER_CHANGE` is "the configuration is changing").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnablementStatus {
    Succeeded,
    Failed,
    UnderChange,
}

impl EnablementStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::UnderChange => "UNDER_CHANGE",
        }
    }
}

/// A persisted `EnabledBaseline`: one baseline applied to one target.
///
/// `parameters` keeps the caller's `EnabledBaselineParameters` as key/`Document` pairs in
/// the order supplied, so the enablement parameters a later reporting call reports are the
/// ones the last call that applied them — `EnableBaseline`, or an `UpdateEnabledBaseline`
/// the clock has made due — was given.
#[derive(Debug, Clone)]
pub struct EnabledBaseline {
    pub arn: String,
    pub baseline_identifier: String,
    pub baseline_version: String,
    pub target_identifier: String,
    pub parameters: Vec<(String, ::aws_smithy_types::Document)>,
    /// `EnabledBaselineSummary$parentIdentifier` /
    /// `EnabledBaselineDetails$parentIdentifier`: the `EnabledBaseline` this one INHERITED
    /// its configuration from, or `None` when it inherits from none.
    ///
    /// A parent is "an ARN that represents the parent EnabledBaseline at the
    /// Organizational Unit (OU) level, from which the child EnabledBaseline inherits its
    /// configuration" (model docs), and a child of one is a deployment into a MEMBER
    /// ACCOUNT of that unit — `EnabledBaselineInheritanceDrift` has inheritance drift
    /// occurring "when any accounts in the target OU do not match the baseline
    /// configuration defined on that OU". Which accounts a unit holds is the organization's
    /// membership, and this service records none: no call it serves states that an account
    /// is a member of a unit, and no rule derives one from a unit's ARN (an organizational
    /// unit identifier is opaque). So every enablement here is one `EnableBaseline` applied
    /// DIRECTLY to its target, which inherits from nothing and carries no parent — the
    /// member is absent rather than filled with a parent nothing establishes. Reported and
    /// filtered on from HERE by both reporting operations, so the day membership is
    /// recorded, the enablements derived from it report their parent through the same path.
    pub parent_identifier: Option<String>,
    /// `EnablementStatusSummary$status`, derived from `due_at` and the transitions parked
    /// on this enablement by [`settle`].
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub status: EnablementStatus,
    /// `EnablementStatusSummary$lastOperationIdentifier`: the most recent operation
    /// started against this resource.
    pub last_operation_identifier: String,
    /// The instant the clock owes this enablement its terminal status — the same instant
    /// as that of every transition parked on it. Stored, never slept on: a reader
    /// materialises the transition once it is due, and [`settle`] derives the reported
    /// status from THIS rather than from the (bounded) operation log, so a record the log
    /// has evicted cannot make an enablement still being changed read as settled.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub due_at: SystemTime,
    pub created_at: SystemTime,
    /// `EnabledBaselineDriftStatusSummary` -> `EnabledBaselineDriftTypes$inheritance` ->
    /// `EnabledBaselineInheritanceDrift$status`, or `None` when no inheritance drift is
    /// assessed for this enablement.
    ///
    /// Two things decide whether drift is assessed at all. Control Tower "detects
    /// inheritance drift on the enabled baselines that target OUs" and "does not detect
    /// drift on the baselines that apply to your landing zone" (model docs), which is the
    /// [`CatalogBaseline::detects_drift`] flag; and drift is a property of a
    /// DEPLOYED configuration, so it is assessed only once the enablement's operation has
    /// become due — [`settle`] materialises it then, the same way it materialises an
    /// enabled control's drift statuses.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub inheritance_drift_status: Option<EnabledBaselineDriftStatus>,
    /// `EnabledBaselineDriftStatusSummary` -> `EnabledBaselineDriftTypes$source` ->
    /// `EnabledBaselineSourceDrift$status`, or `None` when no source drift is assessed for
    /// this enablement.
    ///
    /// The same two things decide whether it is assessed at all as for the dimension above:
    /// the baseline is one AWS Control Tower detects drift on
    /// ([`CatalogBaseline::detects_drift`]), and the enablement's configuration is deployed.
    /// WHICH status it then carries is the one question this dimension answers differently:
    /// source drift occurs "when an enabled baseline no longer matches the landing zone's
    /// configuration" (model docs), so an enablement deployed before the landing zone's
    /// configuration last changed ([`LandingZone::configuration_changed_at`]) has fallen
    /// behind it and reads `DRIFTED`, and one deployed since still matches it and reads
    /// `IN_SYNC`. [`settle`] derives it from those two stored instants, so re-deploying the
    /// enablement — a reset or an update — is what brings a drifted one back in sync.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub source_drift_status: Option<EnabledBaselineDriftStatus>,
    /// The change an `UPDATE_ENABLED_BASELINE` still in flight will apply, or `None`
    /// when nothing is pending. `settle` applies it once that operation is due, so the
    /// applied version and parameters change when the asynchronous update completes and
    /// not when the call that asked for it returned.
    pub pending_update: Option<PendingEnabledBaselineUpdate>,
    /// The teardown a `DISABLE_BASELINE` still in flight will complete, or `None` while no
    /// disable is running. [`settle`] removes the enablement once the clock makes it due,
    /// so the resource stops existing when the asynchronous teardown completes and not
    /// when the call that asked for it returned.
    ///
    /// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
    pub pending_deletion: Option<PendingEnabledBaselineDeletion>,
}

impl EnabledBaseline {
    /// The `driftStatusSummary` this enablement reports: the stored statuses nested under
    /// the drift dimensions the model declares for a baseline (`types.inheritance.status`
    /// and `types.source.status`), and NOTHING at all when no drift is assessed for the
    /// enablement — a baseline AWS detects no drift on, or a deployment the clock has not
    /// made due yet — rather than a status the model does not declare.
    ///
    /// `GetEnabledBaseline` and `ListEnabledBaselines` both report the member through here,
    /// so they cannot disagree about one stored record.
    pub fn drift_status_summary(&self) -> Option<EnabledBaselineDriftStatusSummary> {
        if self.inheritance_drift_status.is_none() && self.source_drift_status.is_none() {
            return None;
        }
        Some(EnabledBaselineDriftStatusSummary {
            types: Some(EnabledBaselineDriftTypes {
                inheritance: self.inheritance_drift_status.map(|status| {
                    EnabledBaselineInheritanceDrift {
                        status: Some(status.as_wire().to_string()),
                    }
                }),
                source: self
                    .source_drift_status
                    .map(|status| EnabledBaselineSourceDrift {
                        status: Some(status.as_wire().to_string()),
                    }),
            }),
        })
    }
}

/// A baseline teardown a `DISABLE_BASELINE` has started and the clock has not yet made
/// due.
///
/// `DisableBaseline` "starts an asynchronous operation to remove all resources deployed as
/// part of the baseline enablement" (model docs), so the enablement does not disappear
/// when that call returns: it stays a reportable resource, `UNDER_CHANGE`, until [`settle`]
/// removes it at `due_at` — the same instant the `DISABLE_BASELINE` record reaches its
/// terminal status.
///
/// [`settle`]: ComAmazonawsControltowerAwscontroltowerapisState::settle
#[derive(Debug, Clone)]
pub struct PendingEnabledBaselineDeletion {
    /// The `DISABLE_BASELINE` operation whose completion removes the enablement.
    pub operation_identifier: String,
    /// The instant the clock owes this removal — the same instant that operation becomes
    /// due, held here so applying it never depends on the operation's record still being
    /// in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
}

/// A change `UpdateEnabledBaseline` accepted and the clock has yet to apply.
///
/// `UpdateEnabledBaseline` "updates an `EnabledBaseline` resource's applied parameters or
/// version" through an asynchronous operation, so the request's values are parked here
/// and become the resource's once the clock has made that operation due.
#[derive(Debug, Clone)]
pub struct PendingEnabledBaselineUpdate {
    /// The `UPDATE_ENABLED_BASELINE` operation whose completion applies this change.
    pub operation_identifier: String,
    /// The instant the clock owes this change its application — the same instant that
    /// operation becomes due, held here so applying it never depends on the operation's
    /// record still being in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
    /// The `baselineVersion` the update moves the enablement to. Required on the
    /// request, so always present.
    pub baseline_version: String,
    /// The applied parameters the update replaces the enablement's with, or `None` when
    /// the request carried no `parameters` member at all — the applied parameters then
    /// stay as they are, while a present (even empty) list is the new applied set.
    pub parameters: Option<Vec<(String, ::aws_smithy_types::Document)>>,
}

/// One entry of the baseline operation log.
#[derive(Debug, Clone)]
pub struct BaselineOperationRecord {
    pub operation_identifier: String,
    pub operation_type: BaselineOperationType,
    /// The `EnabledBaseline` this operation acted on. Kept even after that resource is
    /// gone, so a `DISABLE_BASELINE` identifier still resolves.
    pub enabled_baseline_arn: String,
    pub status: BaselineOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// `ControlOperationType`: which call produced a control operation record.
///
/// Wire values from the model's `ControlOperationType` enum (`ENABLE_CONTROL` |
/// `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`), which is
/// exactly the set of calls that act on an `EnabledControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOperationType {
    EnableControl,
    DisableControl,
    UpdateEnabledControl,
    ResetEnabledControl,
}

impl ControlOperationType {
    /// The value this operation type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnableControl => "ENABLE_CONTROL",
            Self::DisableControl => "DISABLE_CONTROL",
            Self::UpdateEnabledControl => "UPDATE_ENABLED_CONTROL",
            Self::ResetEnabledControl => "RESET_ENABLED_CONTROL",
        }
    }
}

/// `ControlOperationStatus`: how far a control operation has got.
///
/// Wire values from the model's `ControlOperationStatus` enum
/// (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOperationStatus {
    InProgress,
    Succeeded,
    Failed,
}

impl ControlOperationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// One entry of the control operation log.
#[derive(Debug, Clone)]
pub struct ControlOperationRecord {
    pub operation_identifier: String,
    pub operation_type: ControlOperationType,
    /// The `controlIdentifier` the call named, carried on the record so a reporting call
    /// reports the control the operation acted on without the enablement it belongs to
    /// still being in the store.
    pub control_identifier: String,
    /// The `targetIdentifier` the call named, carried for the same reason.
    pub target_identifier: String,
    /// The `EnabledControl` this operation acted on. Kept even after that resource is
    /// gone, so a `DISABLE_CONTROL` identifier still resolves.
    pub enabled_control_arn: String,
    pub status: ControlOperationStatus,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_message: Option<String>,
    /// The instant the clock owes this operation its terminal status. Stored, never
    /// slept on: a reader materialises the transition once it is due.
    pub due_at: SystemTime,
}

/// Per-service state for com-amazonaws-controltower-awscontroltowerapis. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsControltowerAwscontroltowerapisState {
    /// Landing zones keyed by ARN. At most one entry: AWS allows one landing zone per
    /// account, and that rule is enforced from this map's size.
    pub landing_zones: HashMap<String, LandingZone>,
    /// The operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_LANDING_ZONE_OPERATIONS`] records; see the module header on retention.
    pub landing_zone_operations: HashMap<String, LandingZoneOperation>,
    /// Enabled controls keyed by their own ARN — the single home for this service's
    /// control enablements, written by `EnableControl` and by the service itself when a
    /// target comes under its governance ([`apply_governance_controls`]), and read by
    /// `ListEnabledControls`/`GetEnabledControl`. Holds at most
    /// [`MAX_ENABLED_CONTROLS`] entries.
    ///
    /// [`apply_governance_controls`]: Self::apply_governance_controls
    pub enabled_controls: HashMap<String, EnabledControl>,
    /// The control operation log, keyed by `operationIdentifier`. Written by every call
    /// that acts on an `EnabledControl` and read by
    /// `GetControlOperation`/`ListControlOperations`. Append-only up to
    /// [`MAX_CONTROL_OPERATIONS`] records; see the module header on retention.
    pub control_operations: HashMap<String, ControlOperationRecord>,
    /// Enabled baselines keyed by the `EnabledBaseline` ARN this service minted for them.
    pub enabled_baselines: HashMap<String, EnabledBaseline>,
    /// The baseline operation log, keyed by `operationIdentifier`. Append-only up to
    /// [`MAX_BASELINE_OPERATIONS`] records; see the module header on retention.
    pub baseline_operations: HashMap<String, BaselineOperationRecord>,
    /// Tags keyed by the tagged resource's ARN — the single home for this service's
    /// tags, written by `CreateLandingZone`'s, `EnableControl`'s and `EnableBaseline`'s
    /// `tags` members and by `TagResource`, removed from by `UntagResource`, and read by
    /// `ListTagsForResource`.
    pub tags: HashMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsControltowerAwscontroltowerapisState {
    /// Materialise every transition the CLOCK has already made due, and nothing else.
    ///
    /// An operation whose `due_at` has passed is terminal whether or not anybody called,
    /// so settling it here is not a read-triggered mutation; a landing zone's status is
    /// then derived from whether any operation against it is still in flight, and a
    /// configuration an `UPDATE` queued is deployed once its own `due_at` has passed,
    /// unless the operation that queued it FAILED, in which case it deployed nothing.
    /// Call this first in every handler so reporting and mutating operations agree on the
    /// clock.
    ///
    /// An operation record whose availability window has run out is dropped here for the
    /// same reason: it stopped being available at the instant the window closed, whether
    /// or not anybody called (see [`expire_operation_logs`]).
    ///
    /// [`expire_operation_logs`]: Self::expire_operation_logs
    pub fn settle(&mut self, now: SystemTime) {
        for op in self.landing_zone_operations.values_mut() {
            if op.status == LandingZoneOperationStatus::InProgress && now >= op.due_at {
                op.status = LandingZoneOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                op.end_time = Some(op.due_at);
            }
        }

        // A DELETE the clock has made due has finished decommissioning its landing zone,
        // so the resource is gone whether or not anybody called; its operation record
        // stays, and still resolves. The due instant is the parked removal's OWN, not
        // read back off the operation log: `enforce_log_bound` evicts the oldest in-flight
        // record when every record is in flight, and a missing record must not be read as
        // "due" and remove a resource the clock does not owe yet. A teardown whose own
        // record FAILED decommissioned nothing, so the landing zone stays — the same
        // reading the parked configuration below gives a failed operation.
        let operations = &self.landing_zone_operations;
        let mut decommissioned: Vec<String> = Vec::new();
        for (arn, landing_zone) in self.landing_zones.iter_mut() {
            let due = landing_zone
                .pending_deletion
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                // Not due yet leaves the parked removal where it is, unchanged.
                continue;
            };
            if operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == LandingZoneOperationStatus::Failed)
            {
                // A failed teardown removed nothing, so the landing zone stays.
                continue;
            }
            decommissioned.push(arn.clone());
        }
        for arn in &decommissioned {
            self.landing_zones.remove(arn);
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(arn);
        }
        // Decommissioning "deletes Amazon Web Services Control Tower resources deployed in
        // accounts managed by Amazon Web Services Control Tower", not just the landing
        // zone's own record, so applying one that leaves the account with no landing zone
        // at all removes those resources too ([`remove_deployed_enablements`]). Hung off
        // the removals actually APPLIED above rather than off the parked removal, so a
        // decommissioning the clock has not applied — one not due yet, or one whose own
        // operation FAILED — deletes nothing, exactly as it leaves the landing zone and its
        // tags where they are.
        //
        // [`remove_deployed_enablements`]: Self::remove_deployed_enablements
        if !decommissioned.is_empty() && self.landing_zones.is_empty() {
            self.remove_deployed_enablements();
        }

        // A configuration a CREATE, an UPDATE or a RESET started deploying becomes the
        // deployed one once the clock has made that deployment due — the same instant the
        // operation record reaches its terminal status — whether or not anybody called.
        // Until then a reporting call reports what is actually deployed. The due instant is
        // the parked deployment's OWN, for the reason the removal above states, and a
        // deployment whose own record FAILED deployed nothing, so the landing zone keeps
        // the configuration it had.
        let operations = &self.landing_zone_operations;
        for landing_zone in self.landing_zones.values_mut() {
            let due = landing_zone
                .pending_configuration
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                // Not due yet leaves the parked deployment where it is, unchanged.
                continue;
            };
            if operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == LandingZoneOperationStatus::Failed)
            {
                // A failed deployment deployed nothing, so the landing zone keeps the
                // version, manifest, remediation types and drift status it had.
                continue;
            }
            // Whether this deployment leaves the landing zone configured DIFFERENTLY,
            // asked before it is applied. A `RESET` re-deploys the configuration already in
            // place and an `UPDATE` carries the deployed value forward for every member the
            // request omits, so neither moves the instant the enablements under this
            // landing zone measure their source drift against; a deployment that changes
            // the version or the manifest does, as of the instant the clock owed it.
            let changes_configuration =
                landing_zone.version != pending.version || landing_zone.manifest != pending.manifest;
            landing_zone.version = pending.version;
            landing_zone.latest_available_version =
                latest_available_version(&landing_zone.version, CATALOG_LANDING_ZONE_VERSION);
            landing_zone.manifest = pending.manifest;
            landing_zone.remediation_types = pending.remediation_types;
            landing_zone.drift_status = pending.drift_status;
            if changes_configuration {
                landing_zone.configuration_changed_at = pending.due_at;
            }
        }

        let operations = &self.landing_zone_operations;
        for (arn, landing_zone) in self.landing_zones.iter_mut() {
            // A FAILED landing zone is not re-derived from the log. Nothing in this
            // slice can set `Failed` yet, so the branch is unreachable today; it is here
            // so the later UpdateLandingZone/ResetLandingZone slice, which can fail a
            // landing zone, does not have its status silently overwritten by a reader.
            if landing_zone.status == LandingZoneStatus::Failed {
                continue;
            }
            // A landing zone still being decommissioned, or still having a configuration
            // deployed, is PROCESSING whether or not that call's record is still in the
            // (bounded) log: the parked transition is what the clock owes, so the reported
            // status and the transitions parked on the resource cannot diverge.
            let in_flight = any_operation_in_flight(operations, arn)
                || landing_zone.pending_deletion.is_some()
                || landing_zone.pending_configuration.is_some();
            landing_zone.status = if in_flight {
                LandingZoneStatus::Processing
            } else {
                LandingZoneStatus::Active
            };
        }

        // A DISABLE_CONTROL the clock has made due has finished tearing its
        // `EnabledControl` down, so the resource is gone whether or not anybody called;
        // its operation record stays, and still resolves. The parked teardown is TAKEN the
        // moment it falls due, exactly as a landing zone's decommissioning and an enabled
        // baseline's are, so the outcome is decided once: the due instant is the parked
        // teardown's OWN, and the operation's terminal status is read at that one instant
        // rather than on every later settle against a bounded, self-evicting log. A
        // teardown whose own record FAILED removed nothing, so the enablement stays and
        // stops being a teardown in flight — the same reading the parked update below
        // gives a failed operation.
        let operations = &self.control_operations;
        let mut torn_down: Vec<String> = Vec::new();
        for (arn, control) in self.enabled_controls.iter_mut() {
            let due = control
                .pending_deletion
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                // Not due yet leaves the parked teardown where it is, unchanged.
                continue;
            };
            if operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == ControlOperationStatus::Failed)
            {
                // A failed teardown removed nothing, so the enablement stays.
                continue;
            }
            torn_down.push(arn.clone());
        }
        for arn in torn_down {
            self.enabled_controls.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(&arn);
        }

        // An UPDATE_ENABLED_CONTROL the clock has made due has finished applying the
        // configuration it carried, so those parameters are the control's whether or not
        // anybody called. The due instant is the pending change's OWN, not read back off
        // the operation log: `enforce_log_bound` evicts the oldest in-flight record when
        // every record is in flight, and a missing record must not be read as "due" and
        // apply a change the clock does not owe yet.
        let control_operations = &self.control_operations;
        for control in self.enabled_controls.values_mut() {
            // Not due yet leaves the pending change parked, unchanged.
            let due = control
                .pending_update
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                continue;
            };
            if control_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == ControlOperationStatus::Failed)
            {
                // A failed update applied nothing, so the control keeps the parameters it
                // had.
                continue;
            }
            control.parameters = pending.parameters;
        }

        for control in self.enabled_controls.values_mut() {
            if control.status == EnablementStatus::UnderChange && now >= control.due_at {
                control.status = EnablementStatus::Succeeded;
                // Drift is only assessable once the configuration is deployed: while the
                // control is `UNDER_CHANGE` the service "is not able to check the drift
                // status" (`UNKNOWN`), and the deployment that completes here is of the
                // configuration the service expects, so nothing has diverged from it yet.
                if control.drift_status == DriftStatus::Unknown {
                    control.drift_status = DriftStatus::InSync;
                }
                if control.inheritance_drift_status == DriftStatus::Unknown {
                    control.inheritance_drift_status = DriftStatus::InSync;
                }
                if control.resource_drift_status == DriftStatus::Unknown {
                    control.resource_drift_status = DriftStatus::InSync;
                }
            }
        }

        for operation in self.control_operations.values_mut() {
            if operation.status == ControlOperationStatus::InProgress && now >= operation.due_at {
                operation.status = ControlOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                operation.end_time = Some(operation.due_at);
            }
        }

        for operation in self.baseline_operations.values_mut() {
            if operation.status == BaselineOperationStatus::InProgress && now >= operation.due_at {
                operation.status = BaselineOperationStatus::Succeeded;
                // The end time is when the operation became due, not when a caller
                // happened to look, so repeated reads report one stable instant.
                operation.end_time = Some(operation.due_at);
            }
        }

        let baseline_operations = &self.baseline_operations;

        // A DISABLE_BASELINE the clock has made due has finished tearing its
        // `EnabledBaseline` down, so the resource is gone whether or not anybody called;
        // its operation record stays, and still resolves. The due instant is the parked
        // teardown's OWN, not read back off the operation log, for the same reason a
        // landing zone's decommissioning and a control's teardown carry their own:
        // `enforce_log_bound` evicts the oldest in-flight record when every record is in
        // flight, and a removal the clock owes must still be applied when that record is
        // the one dropped. A teardown whose own record FAILED removed nothing, so the
        // enablement stays — the same reading the pending update below gives a failed
        // operation.
        let mut torn_down: Vec<String> = Vec::new();
        for (arn, enabled_baseline) in self.enabled_baselines.iter_mut() {
            let due = enabled_baseline
                .pending_deletion
                .take_if(|pending| now >= pending.due_at);
            let Some(pending) = due else {
                // Not due yet leaves the parked teardown where it is, unchanged.
                continue;
            };
            if baseline_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == BaselineOperationStatus::Failed)
            {
                // A failed teardown removed nothing, so the enablement stays.
                continue;
            }
            torn_down.push(arn.clone());
        }
        for arn in torn_down {
            self.enabled_baselines.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the
            // tag map bounded by the two resource stores.
            self.tags.remove(&arn);
        }

        // An UPDATE_ENABLED_BASELINE the clock has made due has finished applying the
        // change it carried, so the new version and parameters are the resource's whether
        // or not anybody called. The due instant is the pending change's OWN, not read
        // back off the operation log: `enforce_log_bound` evicts the oldest in-flight
        // record when every record is in flight, and a missing record must not be read as
        // "due" and apply a change the clock does not owe yet.
        for enabled_baseline in self.enabled_baselines.values_mut() {
            let Some(pending) = enabled_baseline.pending_update.take() else {
                continue;
            };
            if now < pending.due_at {
                // Not due yet: park it again, unchanged.
                enabled_baseline.pending_update = Some(pending);
                continue;
            }
            if baseline_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == BaselineOperationStatus::Failed)
            {
                // A failed update applied nothing, so the enablement keeps the version
                // and parameters it had.
                continue;
            }
            enabled_baseline.baseline_version = pending.baseline_version;
            if let Some(parameters) = pending.parameters {
                enabled_baseline.parameters = parameters;
            }
        }

        // What a deployed enablement's SOURCE drift is measured against, read once before
        // the walk below: source drift occurs "when an enabled baseline no longer matches
        // the landing zone's configuration" (model docs), and what every enablement has
        // deployed is the landing zone's configuration as of the instant the clock deployed
        // it. `None` when the account holds no landing zone at all, so nothing an
        // enablement could have fallen behind has ever been deployed.
        let configuration_changed_at = self.landing_zone_configuration_changed_at();

        for enabled_baseline in self.enabled_baselines.values_mut() {
            // A FAILED enablement is not re-derived, for the same reason as a FAILED
            // landing zone: a reader must not overwrite a failure.
            if enabled_baseline.status == EnablementStatus::Failed {
                continue;
            }
            // Whether an enablement is still being changed is a property of the RESOURCE:
            // the clock owes it its terminal status at its own `due_at`, the instant every
            // transition parked on it becomes due. Derived from that rather than from the
            // operation log for the reason each parked transition states — a record the
            // log evicted while the transition it started is still owed must not make the
            // resource read as though nothing were changing it.
            enabled_baseline.status = if now < enabled_baseline.due_at {
                EnablementStatus::UnderChange
            } else {
                EnablementStatus::Succeeded
            };
            // Inheritance drift is only assessable once the baseline configuration is
            // deployed, and only for the baselines Control Tower assesses drift on; the
            // deployment that has become due here is of the configuration the enablement
            // defines for the whole target organizational unit, so no account under it has
            // diverged from that configuration yet. Assessed once and then left alone, so
            // a later reader cannot reset a drift status already reported.
            if enabled_baseline.status == EnablementStatus::Succeeded
                && enabled_baseline.inheritance_drift_status.is_none()
                && assesses_drift(&enabled_baseline.baseline_identifier)
            {
                enabled_baseline.inheritance_drift_status =
                    Some(EnabledBaselineDriftStatus::InSync);
            }
            // Source drift is assessable under the same two conditions — a deployed
            // configuration, of a baseline Control Tower assesses drift on — but it is a
            // comparison against something that keeps moving, so it is DERIVED on every
            // settle rather than assessed once: what makes an enablement that was in sync
            // drift is the landing zone having a changed configuration deployed after this
            // enablement deployed its own, and what brings a drifted one back is
            // re-deploying the enablement (a reset or an update, each of which moves its
            // `due_at`). Both are transitions the clock owes, so materialising them here
            // reports what a caller would see had nobody called.
            if enabled_baseline.status == EnablementStatus::Succeeded
                && assesses_drift(&enabled_baseline.baseline_identifier)
            {
                enabled_baseline.source_drift_status = Some(match configuration_changed_at {
                    Some(changed_at) if enabled_baseline.due_at < changed_at => {
                        EnabledBaselineDriftStatus::Drifted
                    }
                    _ => EnabledBaselineDriftStatus::InSync,
                });
            }
        }

        // Done LAST, so every transition a record still owed has been applied from it
        // before it goes: a record only leaves the store once nothing reads it any more.
        self.expire_operation_logs(now);
    }

    /// Drop every operation record whose details have stopped being available.
    ///
    /// A record's details are available for [`OPERATION_DETAILS_AVAILABLE_FOR`] after the
    /// operation started, so one that started longer ago than that is gone whether or not
    /// anybody called — which is why [`settle`] materialises it rather than each reader
    /// testing the age of the record it found. Applied to the logs whose reporting
    /// operations document the window (landing zone and control operations); the baseline
    /// log's does not, so nothing times its records out (see
    /// [`MAX_BASELINE_OPERATIONS`]).
    ///
    /// A clock so far back that the window would predate the epoch leaves every record
    /// inside it, which is what the absent instant means here.
    ///
    /// [`settle`]: Self::settle
    fn expire_operation_logs(&mut self, now: SystemTime) {
        let Some(earliest_available) = now.checked_sub(OPERATION_DETAILS_AVAILABLE_FOR) else {
            return;
        };
        expire_log(&mut self.landing_zone_operations, earliest_available);
        expire_log(&mut self.control_operations, earliest_available);
    }

    /// Remove every control and baseline enablement this account holds, together with the
    /// tags attached to them.
    ///
    /// What a decommissioned landing zone's governance deployed: an `EnabledControl` is the
    /// Amazon Web Services resources `EnableControl` "creates on the specified
    /// organizational unit and the accounts it contains", and an `EnabledBaseline` the same
    /// for `EnableBaseline`, so neither is a resource of an account that holds no landing
    /// zone any more — which is also why an enablement's reported `targetRegions` is read
    /// off the deployed landing zone's governance ([`target_regions_of`]) rather than
    /// captured when it was enabled.
    ///
    /// The three operation logs are untouched: a record outlives the resource it acted on,
    /// exactly as a decommissioned landing zone's own `DELETE` record does, so every
    /// identifier a caller was handed still resolves.
    ///
    /// [`target_regions_of`]: Self::target_regions_of
    fn remove_deployed_enablements(&mut self) {
        let deployed: Vec<String> = self
            .enabled_controls
            .keys()
            .chain(self.enabled_baselines.keys())
            .cloned()
            .collect();
        self.enabled_controls.clear();
        self.enabled_baselines.clear();
        for arn in deployed {
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(&arn);
        }
    }

    /// The stored ARN a `landingZoneIdentifier` names, or `None` when no landing zone
    /// matches.
    ///
    /// A landing zone is addressed by its own ARN — the only identifier this service ever
    /// hands a caller for one — so the identifier is matched against the map key exactly.
    /// A value that is not a landing zone ARN at all is not a lookup that could miss but
    /// a malformed input; a caller checks it with
    /// [`non_landing_zone_identifier_message`] BEFORE asking this.
    pub fn resolve_landing_zone_arn(&self, identifier: &str) -> Option<String> {
        self.landing_zones
            .get_key_value(identifier)
            .map(|(arn, _)| arn.clone())
    }

    /// Whether AWS Control Tower has been set up in this account at all.
    ///
    /// Setting Control Tower up is what creates the account's landing zone and the service
    /// role (`AWSControlTowerAdmin`) its enablement APIs act through, so an account it has
    /// provisioned NOTHING in has neither, and a call that would have to reach the
    /// enablement state through them cannot be served from the store at all — see
    /// [`SETUP_REQUIRED_FOR_BASELINES`] and [`SETUP_REQUIRED_FOR_CONTROLS`] for the two
    /// refusals that reports, each in the words the resource family's own reads carry.
    ///
    /// Read off the stores rather than from a flag, so it answers the same question every
    /// one of those reads asks: anything this service has provisioned — the landing zone,
    /// an enablement of a control or a baseline, or a record in one of the three operation
    /// logs — means setup has run, and an account holding none of it has never been set
    /// up. A landing zone a caller has since decommissioned leaves its operation log
    /// behind, so a read after the teardown is still served (and reports what the cascade
    /// left: nothing enabled).
    pub fn is_set_up(&self) -> bool {
        !self.landing_zones.is_empty()
            || !self.landing_zone_operations.is_empty()
            || !self.enabled_controls.is_empty()
            || !self.control_operations.is_empty()
            || !self.enabled_baselines.is_empty()
            || !self.baseline_operations.is_empty()
    }

    /// The latest instant a landing zone of this account had a CHANGED configuration
    /// deployed ([`LandingZone::configuration_changed_at`]), or `None` while the account
    /// holds no landing zone at all.
    ///
    /// The one instant an enablement's source drift is measured against, so every reader
    /// asks the question the same way: Control Tower governs an account through one landing
    /// zone, and the latest change is what an enablement has to match to be in sync with
    /// it.
    pub fn landing_zone_configuration_changed_at(&self) -> Option<SystemTime> {
        self.landing_zones
            .values()
            .map(|landing_zone| landing_zone.configuration_changed_at)
            .max()
    }

    /// Whether the landing zone `arn` names is still being changed by an operation of its
    /// own. Call [`settle`] first.
    ///
    /// Read off the transitions parked on the RESOURCE as well as off the operation log,
    /// for the same reason [`deletion_in_flight_on`] reads a landing zone's parked removal:
    /// [`enforce_log_bound`] can evict a record while the transition it started is still
    /// owed, and a change in flight must not stop being reported because its record was
    /// dropped. Derived from those facts rather than from the status [`settle`] wrote, which
    /// a landing zone left FAILED keeps rather than having re-derived.
    ///
    /// [`settle`]: Self::settle
    /// [`deletion_in_flight_on`]: Self::deletion_in_flight_on
    pub fn has_operation_in_flight(&self, arn: &str) -> bool {
        any_operation_in_flight(&self.landing_zone_operations, arn)
            || self.landing_zones.get(arn).is_some_and(|landing_zone| {
                landing_zone.pending_configuration.is_some()
                    || landing_zone.pending_deletion.is_some()
            })
    }

    /// The control already enabled for `control` on `target_identifier`, or
    /// `None` when that pair is not enabled.
    ///
    /// A control is enabled at most once per target, so this is what `EnableControl`
    /// reads to refuse a repeat. The control is named by the CATALOG ENTRY a caller's
    /// identifier resolved to ([`resolve_control`]), not by that identifier's text: a
    /// control is published under two ARN forms in every partition and region, so many
    /// identifiers name one control and each stored identifier is resolved before it is
    /// compared. The ARN tie-break keeps the answer independent of the map's iteration
    /// order should a pair ever appear twice.
    pub fn enabled_control_on_target(
        &self,
        control: &CatalogControl,
        target_identifier: &str,
    ) -> Option<&EnabledControl> {
        self.enabled_controls
            .values()
            .filter(|enabled| {
                resolve_control(&enabled.control_identifier) == Some(control)
                    && enabled.target_identifier == target_identifier
            })
            .min_by(|a, b| a.arn.cmp(&b.arn))
    }

    /// The enabled controls one listing reports, as the set of their ARNs: the
    /// enablements the request SCOPES the listing to, plus — only when `include_children`
    /// is set — the ones that inherited their configuration from one of those, directly or
    /// through a chain of inheritance.
    ///
    /// `ListEnabledControls` lists the controls enabled "on the specified organizational
    /// unit and the accounts it contains", and `includeChildren` "specifies whether to
    /// include enabled controls from child organizational units and child accounts in the
    /// response" (model docs, default false). What makes an enablement one of those
    /// children is the enablement it reports as its `parentIdentifier` — "the ARN of the
    /// parent enabled control from which this control inherits its configuration" — so the
    /// inheritance a listing widens by is the one the RECORDS state: an enablement whose
    /// parent chain reaches the scope is a control enabled on something that scope
    /// contains, and one whose chain does not is not. Nothing else is consulted; no target
    /// identifier's text is read for a containment it does not carry.
    ///
    /// The scope is the enablements on `target` when the request names one — including an
    /// inherited enablement whose own target that is, which is a control enabled ON the
    /// named target rather than on a child of it — and, when it names none, the
    /// enablements that inherit from nothing, so the default listing is the directly
    /// applied set and the inherited ones are what the flag adds.
    ///
    /// Call [`settle`] first, so an enablement the clock has already torn down is not in
    /// the answer. The widening itself is [`listed_enablements`], which this service's
    /// other enablement listing answers the same flag through.
    ///
    /// [`settle`]: Self::settle
    pub fn listed_enabled_controls(
        &self,
        target: Option<&str>,
        include_children: bool,
    ) -> BTreeSet<String> {
        listed_enablements(
            &self.enabled_controls,
            |enabled| &enabled.arn,
            |enabled| enabled.parent_identifier.as_deref(),
            |enabled| match target {
                Some(target) => enabled.target_identifier == target,
                None => inherits_from_nothing(
                    &self.enabled_controls,
                    enabled.parent_identifier.as_deref(),
                ),
            },
            include_children,
        )
    }

    /// The enabled baselines one listing reports, as the set of their ARNs: the
    /// enablements that inherited from nothing, plus — only when `include_children` is set
    /// — the ones that inherited their configuration from one of those, directly or
    /// through a chain.
    ///
    /// `ListEnabledBaselines`' `includeChildren` is "a value that can be set to include
    /// the child enabled baselines in responses. The default value is false", and a child
    /// is an enablement that inherited its configuration from another one
    /// (`EnabledBaselineDetails$parentIdentifier`) — the same reading
    /// [`listed_enabled_controls`] gives the control side, and answered through the same
    /// [`listed_enablements`] walk so one change to the rule narrows both listings. This
    /// operation's input carries no target to scope the listing to, only filter members,
    /// and a filter NARROWS what is listed rather than deciding what the inheritance hangs
    /// off, so the scope here is the whole store's directly applied set.
    ///
    /// Call [`settle`] first, so an enablement the clock has already torn down is not in
    /// the answer.
    ///
    /// [`settle`]: Self::settle
    /// [`listed_enabled_controls`]: Self::listed_enabled_controls
    pub fn listed_enabled_baselines(&self, include_children: bool) -> BTreeSet<String> {
        listed_enablements(
            &self.enabled_baselines,
            |enabled| &enabled.arn,
            |enabled| enabled.parent_identifier.as_deref(),
            |enabled| {
                inherits_from_nothing(&self.enabled_baselines, enabled.parent_identifier.as_deref())
            },
            include_children,
        )
    }

    /// The regions the deployed landing zone governs, read out of its `Manifest`
    /// document's `governedRegions` list.
    ///
    /// Empty when no landing zone is deployed or its manifest carries no such list: a
    /// control enablement then falls back to the region its request was signed for. The
    /// account holds at most one landing zone; the ARN order makes the choice total
    /// anyway, so the answer never depends on the map's iteration order.
    ///
    /// Every entry of a stored manifest's list is a Region name ([`is_region_name`],
    /// enforced by [`manifest_defect`] before the manifest is stored), so what this
    /// answers can be reported through the length-bounded member that carries a Region
    /// name as it stands.
    pub fn governed_regions(&self) -> Vec<String> {
        use ::aws_smithy_types::Document;

        let Some(landing_zone) = self.landing_zones.values().min_by(|a, b| a.arn.cmp(&b.arn))
        else {
            return Vec::new();
        };
        let Document::Object(members) = &landing_zone.manifest else {
            return Vec::new();
        };
        let Some(Document::Array(regions)) = members.get("governedRegions") else {
            return Vec::new();
        };
        regions
            .iter()
            .filter_map(|region| match region {
                Document::String(name) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// The regions an enabled control is expected to be deployed into — the value
    /// `EnabledControlDetails$targetRegions` reports.
    ///
    /// A control is deployed into the regions the landing zone governs (`Region`: "the
    /// expected Regions are based on the Regions that are governed by the landing zone"),
    /// so this is DERIVED from the deployed manifest on every read rather than captured
    /// when the control was enabled: extending governance extends every existing
    /// enablement into the added regions, and withdrawing a region withdraws them from it.
    /// Call [`settle`] first, so a governance change an `UPDATE` queued counts from the
    /// instant its deployment became due.
    ///
    /// With no landing zone deployed, or one whose manifest governs no region, there is no
    /// governed set to read and the only region the enablement names is the one its own
    /// request was signed for.
    ///
    /// [`settle`]: Self::settle
    pub fn target_regions_of(&self, control: &EnabledControl) -> Vec<String> {
        let governed = self.governed_regions();
        if governed.is_empty() {
            vec![control.enablement_region.clone()]
        } else {
            governed
        }
    }

    /// The `operationIdentifier` of a `DELETE` still decommissioning a landing zone of
    /// this account, or `None` when none is. Call [`settle`] first.
    ///
    /// An account holds one landing zone, and a landing zone being decommissioned still
    /// occupies it: the account is only free once the clock has made that removal due. The
    /// `due_at`/identifier pair orders the candidates totally, so the answer does not
    /// depend on the map's iteration order.
    ///
    /// [`settle`]: Self::settle
    pub fn deletion_in_flight(&self) -> Option<String> {
        self.landing_zones
            .values()
            .filter_map(|landing_zone| landing_zone.pending_deletion.as_ref())
            .min_by(|a, b| {
                (a.due_at, a.operation_identifier.as_str())
                    .cmp(&(b.due_at, b.operation_identifier.as_str()))
            })
            .map(|pending| pending.operation_identifier.clone())
    }

    /// The `operationIdentifier` of the `DELETE` still decommissioning the landing zone
    /// `arn` names, or `None` when that landing zone is not being decommissioned. Call
    /// [`settle`] first.
    ///
    /// Read off the parked removal rather than the operation log, for the same reason
    /// `settle` derives the status from it: `enforce_log_bound` can evict the `DELETE`
    /// record while the removal the clock owes is still parked, and the identifier a
    /// repeated delete reports must be the one that removal carries.
    ///
    /// [`settle`]: Self::settle
    pub fn deletion_in_flight_on(&self, arn: &str) -> Option<String> {
        self.landing_zones
            .get(arn)
            .and_then(|landing_zone| landing_zone.pending_deletion.as_ref())
            .map(|pending| pending.operation_identifier.clone())
    }

    /// Start a `DELETE` against the landing zone `arn` names, parking its removal for
    /// [`settle`] to apply once the clock makes it due, and return the operation's
    /// `operationIdentifier`.
    ///
    /// The landing zone stays in the store, `PROCESSING`, until that instant: the delete
    /// is asynchronous, so while it runs the landing zone is still a resource a reporting
    /// call reports and a second mutating call collides with. Only the operation record is
    /// written when `arn` names nothing, which a caller rules out first by resolving it.
    ///
    /// [`settle`]: Self::settle
    pub fn record_landing_zone_deletion(&mut self, arn: &str, now: SystemTime) -> String {
        let operation_identifier =
            self.record_operation(LandingZoneOperationType::Delete, arn, now);
        if let Some(landing_zone) = self.landing_zones.get_mut(arn) {
            // The landing zone is PROCESSING while its DELETE runs. `settle` derives the
            // status, from the parked removal below as well as from the log, so this only
            // covers the gap until the next settle rather than owning the status.
            landing_zone.status = LandingZoneStatus::Processing;
            landing_zone.pending_deletion = Some(PendingLandingZoneDeletion {
                operation_identifier: operation_identifier.clone(),
                due_at: now + OPERATION_SETTLE_AFTER,
            });
        }
        operation_identifier
    }

    /// Start a `CREATE`/`UPDATE`/`RESET` against the landing zone `arn` names, parking the
    /// configuration it deploys for [`settle`] to deploy once the clock makes it due, and
    /// return the operation's `operationIdentifier`.
    ///
    /// The identifier is minted FIRST and carried on the parked deployment, so [`settle`]
    /// can tell a deployment whose operation succeeded from one whose operation failed —
    /// the single place this crate ties a landing zone's parked configuration to the
    /// operation that started it, as it ties every other parked transition to its own.
    /// The landing zone keeps reporting the configuration it has deployed until that
    /// instant: the deployment is asynchronous. Only the operation record is written when
    /// `arn` names nothing, which a caller rules out first by resolving it.
    ///
    /// [`settle`]: Self::settle
    pub fn record_landing_zone_configuration(
        &mut self,
        operation_type: LandingZoneOperationType,
        arn: &str,
        configuration: LandingZoneConfiguration,
        now: SystemTime,
    ) -> String {
        let operation_identifier = self.record_operation(operation_type, arn, now);
        if let Some(landing_zone) = self.landing_zones.get_mut(arn) {
            // The landing zone is PROCESSING while the deployment runs. `settle` derives
            // the status, from the parked deployment below as well as from the log, so this
            // only covers the gap until the next settle rather than owning the status.
            landing_zone.status = LandingZoneStatus::Processing;
            landing_zone.pending_configuration = Some(PendingLandingZoneConfiguration {
                operation_identifier: operation_identifier.clone(),
                version: configuration.version,
                manifest: configuration.manifest,
                remediation_types: configuration.remediation_types,
                drift_status: configuration.drift_status,
                due_at: now + OPERATION_SETTLE_AFTER,
            });
        }
        operation_identifier
    }

    /// Append an operation to the log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_LANDING_ZONE_OPERATIONS`]) on the way
    /// out, so this is the single place the log can grow.
    pub fn record_operation(
        &mut self,
        operation_type: LandingZoneOperationType,
        landing_zone_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.landing_zone_operations.insert(
            operation_identifier.clone(),
            LandingZoneOperation {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                landing_zone_arn: landing_zone_arn.to_string(),
                status: LandingZoneOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(
            &mut self.landing_zone_operations,
            MAX_LANDING_ZONE_OPERATIONS,
        );
        operation_identifier
    }

    /// Append a control operation to its log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_CONTROL_OPERATIONS`]) on the way out, so
    /// this is the single place that log can grow.
    pub fn record_control_operation(
        &mut self,
        operation_type: ControlOperationType,
        control_identifier: &str,
        target_identifier: &str,
        enabled_control_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.control_operations.insert(
            operation_identifier.clone(),
            ControlOperationRecord {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                control_identifier: control_identifier.to_string(),
                target_identifier: target_identifier.to_string(),
                enabled_control_arn: enabled_control_arn.to_string(),
                status: ControlOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(&mut self.control_operations, MAX_CONTROL_OPERATIONS);
        operation_identifier
    }

    /// Start an `UPDATE_ENABLED_CONTROL` against the control `arn` names, parking
    /// `parameters` for [`settle`] to apply once that operation is due, and return the
    /// operation's `operationIdentifier`.
    ///
    /// `None` when `arn` names no stored control, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it
    /// is not bounded by [`MAX_ENABLED_CONTROLS`]; the operation it appends is bounded by
    /// the log's own retention rule.
    ///
    /// The caller refuses an update that overlaps an operation still changing the control
    /// with the modelled `ConflictException`, which is what keeps the single pending slot
    /// from being overwritten while the change it holds is still owed: every accepted
    /// update is applied before another can be accepted.
    ///
    /// [`settle`]: Self::settle
    pub fn record_enabled_control_update(
        &mut self,
        arn: &str,
        parameters: Vec<(String, ::aws_smithy_types::Document)>,
        now: SystemTime,
    ) -> Option<String> {
        self.start_control_change(
            ControlOperationType::UpdateEnabledControl,
            arn,
            parameters,
            now,
        )
    }

    /// Start a `RESET_ENABLED_CONTROL` against the control `arn` names and return the
    /// operation's `operationIdentifier`.
    ///
    /// A reset re-deploys the configuration the control already carries rather than a new
    /// one, so the parameters it parks are the DEPLOYED ones and a reporting call reads the
    /// same configuration before, during and after — exactly as a landing zone's reset
    /// re-deploys the manifest it already has. What the re-deployment settles is drift: a
    /// control cannot be assessed against a configuration still being deployed, so the
    /// three drift dimensions go back to `UNKNOWN` ([`start_control_change`], which every
    /// call that deploys a configuration shares) and [`settle`] makes them `IN_SYNC` once
    /// the clock has made the operation due.
    ///
    /// [`start_control_change`]: Self::start_control_change
    ///
    /// `None` when `arn` names no stored control, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it is
    /// not bounded by [`MAX_ENABLED_CONTROLS`]; the operation it appends is bounded by the
    /// log's own retention rule.
    ///
    /// [`settle`]: Self::settle
    pub fn record_enabled_control_reset(&mut self, arn: &str, now: SystemTime) -> Option<String> {
        // The configuration the reset re-deploys is the one already configured, read out
        // of the store rather than taken from the request: a reset carries none.
        let deployed = self.enabled_controls.get(arn)?.parameters.clone();
        self.start_control_change(ControlOperationType::ResetEnabledControl, arn, deployed, now)
    }

    /// Start a `DISABLE_CONTROL` against the control `arn` names, parking its removal for
    /// [`settle`] to apply once the clock makes it due, and return the operation's
    /// `operationIdentifier`.
    ///
    /// The enablement stays in the store, `UNDER_CHANGE`, until that instant: the teardown
    /// is asynchronous, so while it runs the control is still a resource a reporting call
    /// reports and a second mutating call collides with. It deploys no configuration, so
    /// unlike an update or a reset it parks no parameters — what it parks is the removal.
    ///
    /// The configuration the control had deployed is being removed, so it is no longer the
    /// configuration the target is held against and the control's three drift dimensions
    /// read `UNKNOWN` while the teardown is in flight, exactly as they do while any other
    /// change to what it has deployed is in flight ([`start_control_change`]).
    ///
    /// `None` when `arn` names no stored control, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written.
    ///
    /// [`start_control_change`]: Self::start_control_change
    ///
    /// [`settle`]: Self::settle
    pub fn record_enabled_control_disable(&mut self, arn: &str, now: SystemTime) -> Option<String> {
        let (control_identifier, target_identifier) = self.enabled_controls.get(arn).map(|control| {
            (
                control.control_identifier.clone(),
                control.target_identifier.clone(),
            )
        })?;

        let operation_identifier = self.record_control_operation(
            ControlOperationType::DisableControl,
            &control_identifier,
            &target_identifier,
            arn,
            now,
        );
        let control = self.enabled_controls.get_mut(arn)?;
        // ONE instant, held in both places that need it: the status the control reports
        // while the teardown runs is owed at the same moment as the removal itself, so the
        // two cannot drift apart.
        let due_at = now + OPERATION_SETTLE_AFTER;
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = due_at;
        control.pending_deletion = Some(PendingEnabledControlDeletion {
            operation_identifier: operation_identifier.clone(),
            due_at,
        });
        control.withdraw_drift_assessment();
        Some(operation_identifier)
    }

    /// The one span every call that deploys a configuration against an already enabled
    /// control shares: append the operation record its type names, mark the control
    /// `UNDER_CHANGE` until the clock makes that operation due, park `parameters` for
    /// [`settle`] to apply then, and drop the drift assessment the control carries.
    ///
    /// Drift is a property of a DEPLOYED configuration, and the configuration a call like
    /// this starts deploying is not deployed until the clock makes the operation due, so
    /// the three dimensions read `UNKNOWN` — the service "is not able to check the drift
    /// status" — until [`settle`] assesses the deployed configuration.
    ///
    /// `None` when `arn` names no stored control, in which case nothing is written.
    ///
    /// [`settle`]: Self::settle
    fn start_control_change(
        &mut self,
        operation_type: ControlOperationType,
        arn: &str,
        parameters: Vec<(String, ::aws_smithy_types::Document)>,
        now: SystemTime,
    ) -> Option<String> {
        let (control_identifier, target_identifier) = self
            .enabled_controls
            .get(arn)
            .map(|control| {
                (
                    control.control_identifier.clone(),
                    control.target_identifier.clone(),
                )
            })?;

        let operation_identifier = self.record_control_operation(
            operation_type,
            &control_identifier,
            &target_identifier,
            arn,
            now,
        );
        let control = self.enabled_controls.get_mut(arn)?;
        // The control's configuration is changing until the operation is due, so a caller
        // reading straight after the call sees the transient status rather than the
        // configuration it asked for.
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = now + OPERATION_SETTLE_AFTER;
        control.pending_update = Some(PendingEnabledControlUpdate {
            operation_identifier: operation_identifier.clone(),
            due_at: now + OPERATION_SETTLE_AFTER,
            parameters,
        });
        control.withdraw_drift_assessment();
        Some(operation_identifier)
    }

    /// The stored ARN an `enabledBaselineIdentifier` names, or `None` when no enabled
    /// baseline matches.
    ///
    /// Matched against the map key exactly, for the same reason
    /// [`resolve_landing_zone_arn`] is: the identifier of an enablement is the ARN this
    /// service minted for it, which is what every call reports
    /// (`EnableBaselineOutput$arn`, `EnabledBaselineSummary$arn`). Every member carrying
    /// one is typed `Arn`, so the front door refuses a value that is not an ARN before any
    /// handler runs.
    ///
    /// [`resolve_landing_zone_arn`]: Self::resolve_landing_zone_arn
    pub fn resolve_enabled_baseline_arn(&self, identifier: &str) -> Option<String> {
        self.enabled_baselines
            .get_key_value(identifier)
            .map(|(arn, _)| arn.clone())
    }

    /// The enabled baseline that already applies `baseline` to `target_identifier`, or
    /// `None` when that pair is not enabled. Call [`settle`] first, so a pair whose
    /// DISABLE has become due reads as not enabled — and so the status carried by the
    /// record handed back is the one the clock owes it.
    ///
    /// The baseline is named by the CATALOG ENTRY a caller's identifier resolved to
    /// ([`resolve_baseline`]), not by that identifier's text: the same catalog is
    /// published in every partition, region and account, so many identifiers name one
    /// baseline and each stored identifier is resolved before it is compared. A stored
    /// identifier always resolves, because an enablement is only ever written for a
    /// baseline this service publishes.
    ///
    /// The whole record rather than its ARN, for the same reason
    /// [`enabled_control_on_target`] hands one back: `EnableBaseline` classifies a
    /// refused repeat by the STATE of the enablement it collided with, so it needs more
    /// of that enablement than its name.
    ///
    /// [`settle`]: Self::settle
    /// [`enabled_control_on_target`]: Self::enabled_control_on_target
    pub fn enabled_baseline_on_target(
        &self,
        baseline: &CatalogBaseline,
        target_identifier: &str,
    ) -> Option<&EnabledBaseline> {
        self.enabled_baselines
            .values()
            .filter(|enabled| {
                resolve_baseline(&enabled.baseline_identifier) == Some(baseline)
                    && enabled.target_identifier == target_identifier
            })
            // The ARN orders the candidates totally, so the answer does not depend on the
            // map's iteration order.
            .min_by(|a, b| a.arn.cmp(&b.arn))
    }

    /// The identifier of the baseline operation still changing the enablement `arn` names,
    /// or `None` when nothing is changing it. Call [`settle`] first.
    ///
    /// Read off the RESOURCE — the status [`settle`] derived from the transitions parked on
    /// it — rather than off the operation log, for the same reason
    /// [`deletion_in_flight_on`] reads a landing zone's parked removal:
    /// [`enforce_log_bound`] can evict a record while the transition it started is still
    /// owed, and a change in flight must not stop being reported because its record was
    /// dropped. An enablement carries at most one operation at a time — every call that
    /// starts one is refused while another is in flight — so the identifier of the one
    /// changing it is the last one started against it.
    ///
    /// [`settle`]: Self::settle
    /// [`deletion_in_flight_on`]: Self::deletion_in_flight_on
    pub fn baseline_change_in_flight_on(&self, arn: &str) -> Option<String> {
        self.enabled_baselines
            .get(arn)
            .filter(|enabled| enabled.status == EnablementStatus::UnderChange)
            .map(|enabled| enabled.last_operation_identifier.clone())
    }

    /// The `operationIdentifier` of the `DISABLE_BASELINE` still tearing the enablement
    /// `arn` names down, or `None` when that enablement is not being torn down. Call
    /// [`settle`] first.
    ///
    /// Read off the parked teardown rather than the operation log, for the same reason
    /// [`settle`] applies it from there: [`enforce_log_bound`] can evict the
    /// `DISABLE_BASELINE` record while the removal the clock owes is still parked, and the
    /// identifier a repeated disable reports must be the one that teardown carries.
    ///
    /// [`settle`]: Self::settle
    pub fn baseline_deletion_in_flight_on(&self, arn: &str) -> Option<String> {
        self.enabled_baselines
            .get(arn)
            .and_then(|enabled| enabled.pending_deletion.as_ref())
            .map(|pending| pending.operation_identifier.clone())
    }

    /// Append a baseline operation to its log and return its `operationIdentifier`.
    ///
    /// Enforces the log's retention bound ([`MAX_BASELINE_OPERATIONS`]) on the way out,
    /// so this is the single place that log can grow.
    pub fn record_baseline_operation(
        &mut self,
        operation_type: BaselineOperationType,
        enabled_baseline_arn: &str,
        now: SystemTime,
    ) -> String {
        let operation_identifier = new_operation_identifier();
        self.baseline_operations.insert(
            operation_identifier.clone(),
            BaselineOperationRecord {
                operation_identifier: operation_identifier.clone(),
                operation_type,
                enabled_baseline_arn: enabled_baseline_arn.to_string(),
                status: BaselineOperationStatus::InProgress,
                start_time: now,
                end_time: None,
                status_message: None,
                due_at: now + OPERATION_SETTLE_AFTER,
            },
        );
        enforce_log_bound(&mut self.baseline_operations, MAX_BASELINE_OPERATIONS);
        operation_identifier
    }

    /// Persist an enablement and start its `ENABLE_BASELINE` operation, returning that
    /// operation's `operationIdentifier`.
    ///
    /// `None` when the account already holds [`MAX_ENABLED_BASELINES`] enablements: the
    /// request is then refused and NOTHING is written — neither the resource nor an
    /// operation record. The single insertion site for `enabled_baselines`, so the cap
    /// cannot be bypassed from a handler.
    pub fn record_enablement(
        &mut self,
        arn: &str,
        baseline_identifier: &str,
        baseline_version: &str,
        target_identifier: &str,
        parameters: Vec<(String, ::aws_smithy_types::Document)>,
        now: SystemTime,
    ) -> Option<String> {
        if self.enabled_baselines.len() >= MAX_ENABLED_BASELINES {
            return None;
        }

        let operation_identifier = self.record_baseline_operation(
            BaselineOperationType::EnableBaseline,
            arn,
            now,
        );
        self.enabled_baselines.insert(
            arn.to_string(),
            EnabledBaseline {
                arn: arn.to_string(),
                baseline_identifier: baseline_identifier.to_string(),
                baseline_version: baseline_version.to_string(),
                target_identifier: target_identifier.to_string(),
                parameters,
                // `EnableBaseline` applies a baseline DIRECTLY to the target the request
                // names, so what it persists inherits its configuration from no other
                // enablement and reports no parent. A child enablement is one Control
                // Tower derives from the organization's membership — a deployment into a
                // member ACCOUNT of a target organizational unit — and this service
                // records no membership for one to be derived from, so the member stays
                // absent rather than naming a parent nothing establishes.
                parent_identifier: None,
                // The enablement is changing until its ENABLE_BASELINE operation is due;
                // the status is derived from the log by `settle`, and seeded to match.
                status: EnablementStatus::UnderChange,
                last_operation_identifier: operation_identifier.clone(),
                // The clock owes the enablement its terminal status when its
                // ENABLE_BASELINE operation becomes due; `settle` derives the reported
                // status from this instant.
                due_at: now + OPERATION_SETTLE_AFTER,
                created_at: now,
                // Drift is a property of a DEPLOYED configuration, so nothing is assessed
                // while the ENABLE_BASELINE operation is still in flight; `settle`
                // materialises both dimensions once that operation is due.
                inheritance_drift_status: None,
                source_drift_status: None,
                pending_update: None,
                pending_deletion: None,
            },
        );
        Some(operation_identifier)
    }

    /// Apply every MANDATORY control of [`CONTROL_CATALOG`] to `target_identifier`, the
    /// controls AWS Control Tower applies to a target it governs itself.
    ///
    /// Enabling the baseline that brings a target under governance
    /// ([`CatalogBaseline::establishes_governance`]) is what makes the service responsible
    /// for a target, and the controls it applies itself are the mandatory ones — the
    /// entries no caller may enable ([`CatalogControl::enablable`]) — so they become
    /// enablements of this account's one control store and every control reporting
    /// operation reports them beside the ones a caller enabled. A baseline that establishes
    /// no governance applies nothing.
    ///
    /// Each lands as its OWN `EnabledControl` with its own `ENABLE_CONTROL` operation,
    /// `UNDER_CHANGE` until the clock makes that operation due, exactly as a caller's
    /// enablement does: the deployment is asynchronous whoever started it, and these are
    /// started together with the enablement that brought the target under governance. A
    /// control already enabled on the target is left alone, so nothing is applied twice,
    /// and nothing is written past [`MAX_ENABLED_CONTROLS`]: the store's bound holds
    /// whoever writes into it.
    pub fn apply_governance_controls(
        &mut self,
        baseline: &CatalogBaseline,
        target_identifier: &str,
        region: &str,
        now: SystemTime,
    ) {
        if !baseline.establishes_governance {
            return;
        }

        for control in CONTROL_CATALOG.iter().filter(|control| !control.enablable) {
            if self.enabled_controls.len() >= MAX_ENABLED_CONTROLS {
                break;
            }
            if self
                .enabled_control_on_target(control, target_identifier)
                .is_some()
            {
                continue;
            }

            // The control is named by its AWS Control Tower ARN, the identifier form whose
            // resource segment is the control's name, so what is persisted is an identifier
            // a caller can read back into any member that names a control.
            let control_identifier = control_arn(region, control.name);
            let arn = enabled_control_arn(region, &new_enabled_control_id());
            let operation_identifier = self.record_control_operation(
                ControlOperationType::EnableControl,
                &control_identifier,
                target_identifier,
                &arn,
                now,
            );
            self.enabled_controls.insert(
                arn.clone(),
                EnabledControl {
                    arn,
                    control_identifier,
                    target_identifier: target_identifier.to_string(),
                    // Applied directly to the target the baseline named, so it inherits its
                    // configuration from no other enabled control — the same reading
                    // `EnableControl` gives this member.
                    parent_identifier: None,
                    // A mandatory control declares no enablement parameter, so there is no
                    // configuration for one to carry.
                    parameters: Vec::new(),
                    enablement_region: region.to_string(),
                    status: EnablementStatus::UnderChange,
                    last_operation_identifier: operation_identifier,
                    // Drift is not assessable until the configuration is deployed;
                    // [`settle`] moves these to `IN_SYNC` once the enablement is due.
                    drift_status: DriftStatus::Unknown,
                    inheritance_drift_status: DriftStatus::Unknown,
                    resource_drift_status: DriftStatus::Unknown,
                    due_at: now + OPERATION_SETTLE_AFTER,
                    pending_update: None,
                    pending_deletion: None,
                },
            );
        }
    }

    /// Start the teardown of every MANDATORY control applied to the target of the enabled
    /// baseline `arn` names.
    ///
    /// The counterpart of [`apply_governance_controls`]: a target stops being governed when
    /// the baseline that brought it under governance is disabled, so the controls the
    /// service applied to it are no longer enablements of this account. Each goes through
    /// the asynchronous teardown a caller's disable starts
    /// ([`record_enabled_control_disable`]), so a caller polling straight after the call
    /// still sees them `UNDER_CHANGE` and [`settle`] removes them once the clock makes them
    /// due. An enablement of a baseline that establishes no governance, and an `arn` naming
    /// no enablement, withdraw nothing.
    ///
    /// [`settle`]: Self::settle
    /// [`apply_governance_controls`]: Self::apply_governance_controls
    /// [`record_enabled_control_disable`]: Self::record_enabled_control_disable
    pub fn withdraw_governance_controls(&mut self, arn: &str, now: SystemTime) {
        let Some(target_identifier) = self
            .enabled_baselines
            .get(arn)
            .filter(|enabled| {
                resolve_baseline(&enabled.baseline_identifier)
                    .is_some_and(|baseline| baseline.establishes_governance)
            })
            .map(|enabled| enabled.target_identifier.clone())
        else {
            return;
        };

        // The ARNs order the teardowns totally, so the operations they start are started in
        // one order whatever order the store iterates in. A control already being torn down
        // is left to the teardown it carries rather than given a second one.
        let mut applied: Vec<String> = self
            .enabled_controls
            .values()
            .filter(|enabled| {
                enabled.target_identifier == target_identifier
                    && enabled.pending_deletion.is_none()
                    && is_mandatory_control(&enabled.control_identifier)
            })
            .map(|enabled| enabled.arn.clone())
            .collect();
        applied.sort();
        for control_arn in &applied {
            let _ = self.record_enabled_control_disable(control_arn, now);
        }
    }

    /// Whether `arn` names a resource of this service that tags attach to.
    ///
    /// The three resource stores are the whole set: a landing zone, an enabled control
    /// and an enabled baseline are the resources this service mints ARNs for, and every
    /// one of those ARNs is its store's key. An ARN outside them names no resource here,
    /// which is what the tagging operations refuse with the tag plane's own not-found
    /// class ([`TaggingRefusal`]). Call [`settle`] first, so a resource the clock has
    /// already torn down is gone.
    ///
    /// [`settle`]: Self::settle
    pub fn taggable_resource_exists(&self, arn: &str) -> bool {
        self.landing_zones.contains_key(arn)
            || self.enabled_controls.contains_key(arn)
            || self.enabled_baselines.contains_key(arn)
    }

    /// The tags attached to `arn`, in key order; empty when the resource carries none.
    ///
    /// A resource with no entry in the tag map is untagged, not missing: existence is
    /// [`taggable_resource_exists`]'s question, and a caller must be able to tell an
    /// untagged resource (an empty map) from an absent one (a 404).
    ///
    /// [`taggable_resource_exists`]: Self::taggable_resource_exists
    pub fn tags_of(&self, arn: &str) -> BTreeMap<String, String> {
        self.tags.get(arn).cloned().unwrap_or_default()
    }

    /// Merge `tags` into the tags attached to `arn`, returning the resulting tag count.
    ///
    /// A key already attached takes the new value and every other key is left alone, so
    /// applying tags adds to a resource's tag set rather than replacing it. `None` when
    /// the merged set would exceed [`MAX_TAGS_PER_RESOURCE`]: NOTHING is then written and
    /// the caller refuses the request. The single site that GROWS a resource's tag set,
    /// so that cap cannot be bypassed by repeated tagging; the create/enable operations
    /// write their own `tags` member straight into the store at a freshly minted ARN,
    /// where the front door's `TagMap` `@length` max already holds the same count.
    ///
    /// The keys are the CALLER's to check first: a map carrying a key in the reserved
    /// [`RESERVED_TAG_KEY_PREFIX`] namespace is refused before any write reaches here
    /// (see [`reserved_tag_key_message`]), so nothing stored through this method holds a
    /// key Amazon Web Services keeps for itself.
    pub fn merge_tags<I>(&mut self, arn: &str, tags: I) -> Option<usize>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut merged = self.tags_of(arn);
        merged.extend(tags);
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            return None;
        }
        let count = merged.len();
        self.tags.insert(arn.to_string(), merged);
        Some(count)
    }

    /// Remove the tags `keys` names from the tags attached to `arn`.
    ///
    /// A key the resource does not carry is removed by having nothing to remove: untagging
    /// states the tag set a caller no longer wants, so repeating it is the same call, and
    /// every key not named is left exactly as it was. The single site that SHRINKS a
    /// resource's tag set, and the counterpart of [`merge_tags`]; it can only lower the
    /// store's size, so no cap applies and nothing here can refuse the request — which is
    /// why it reports no count where [`merge_tags`] reports one. A resource left with no
    /// tags loses its entry rather than keeping an empty map, which is the same thing a
    /// read reports ([`tags_of`] has no entry and no tags alike) and keeps the store no
    /// larger than the resources that have tags.
    ///
    /// [`merge_tags`]: Self::merge_tags
    /// [`tags_of`]: Self::tags_of
    pub fn remove_tags<'a, I>(&mut self, arn: &str, keys: I)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let Some(attached) = self.tags.get_mut(arn) else {
            return;
        };
        for key in keys {
            attached.remove(key);
        }
        if attached.is_empty() {
            self.tags.remove(arn);
        }
    }

    /// Start an `UPDATE_ENABLED_BASELINE` against the enablement `arn` names, parking the
    /// change for [`settle`] to apply once that operation is due, and return the
    /// operation's `operationIdentifier`.
    ///
    /// `None` when `arn` names no stored enablement, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it
    /// is not bounded by [`MAX_ENABLED_BASELINES`]; the operation it appends is bounded by
    /// the log's own retention rule.
    ///
    /// The caller refuses an update that overlaps a baseline operation still in flight
    /// against `arn` with the modelled `ConflictException` (`UpdateEnabledBaseline` is not
    /// `@idempotent`), which is what keeps the single pending slot from being overwritten
    /// while the change it holds is still owed: every accepted update is applied before
    /// another can be accepted.
    ///
    /// The configuration this parks is not deployed yet, so the enablement carries no
    /// assessed inheritance drift while the operation is in flight — the one rule
    /// [`start_enabled_baseline_change`] holds for every call that starts changing what an
    /// existing enablement has deployed.
    ///
    /// [`settle`]: Self::settle
    /// [`start_enabled_baseline_change`]: Self::start_enabled_baseline_change
    pub fn record_enabled_baseline_update(
        &mut self,
        arn: &str,
        baseline_version: &str,
        parameters: Option<Vec<(String, ::aws_smithy_types::Document)>>,
        now: SystemTime,
    ) -> Option<String> {
        if !self.enabled_baselines.contains_key(arn) {
            return None;
        }

        let operation_identifier =
            self.record_baseline_operation(BaselineOperationType::UpdateEnabledBaseline, arn, now);
        // ONE instant, held in both places that need it: the status the enablement reports
        // while the update runs is owed at the same moment as the change itself, so the two
        // cannot drift apart.
        let due_at = now + OPERATION_SETTLE_AFTER;
        let enabled_baseline =
            self.start_enabled_baseline_change(arn, &operation_identifier, due_at)?;
        enabled_baseline.pending_update = Some(PendingEnabledBaselineUpdate {
            operation_identifier: operation_identifier.clone(),
            due_at,
            baseline_version: baseline_version.to_string(),
            parameters,
        });
        Some(operation_identifier)
    }

    /// Start a `DISABLE_BASELINE` against the enablement `arn` names, parking its removal
    /// for [`settle`] to apply once the clock makes it due, and return the operation's
    /// `operationIdentifier`.
    ///
    /// The enablement stays in the store, `UNDER_CHANGE`, until that instant: the teardown
    /// is asynchronous, so while it runs the enablement is still a resource a reporting
    /// call reports and a second mutating call collides with. It applies no configuration,
    /// so unlike an update it parks no parameters — what it parks is the removal, exactly
    /// as a control's teardown does ([`record_enabled_control_disable`]).
    ///
    /// The configuration the enablement had deployed is being removed, so it is no longer
    /// the configuration the target is held against and the enablement carries no assessed
    /// inheritance drift while the teardown is in flight: the span every call that starts
    /// changing what an enablement has deployed shares
    /// ([`start_enabled_baseline_change`]).
    ///
    /// `None` when `arn` names no stored enablement, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written.
    ///
    /// [`settle`]: Self::settle
    /// [`record_enabled_control_disable`]: Self::record_enabled_control_disable
    /// [`start_enabled_baseline_change`]: Self::start_enabled_baseline_change
    pub fn record_enabled_baseline_disable(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<String> {
        if !self.enabled_baselines.contains_key(arn) {
            return None;
        }

        let operation_identifier =
            self.record_baseline_operation(BaselineOperationType::DisableBaseline, arn, now);
        // ONE instant, held in both places that need it: the status the enablement reports
        // while the teardown runs is owed at the same moment as the removal itself, so the
        // two cannot drift apart.
        let due_at = now + OPERATION_SETTLE_AFTER;
        let enabled_baseline =
            self.start_enabled_baseline_change(arn, &operation_identifier, due_at)?;
        enabled_baseline.pending_deletion = Some(PendingEnabledBaselineDeletion {
            operation_identifier: operation_identifier.clone(),
            due_at,
        });
        Some(operation_identifier)
    }

    /// Start a `RESET_ENABLED_BASELINE` against the enablement `arn` names and return the
    /// operation's `operationIdentifier`.
    ///
    /// A reset RE-APPLIES the configuration the enablement already carries rather than a new
    /// one, so it parks no pending change: the applied version and parameters read the same
    /// before, during and after it, exactly as a control's reset re-deploys the configuration
    /// that control already carries ([`record_enabled_control_reset`]). What the
    /// re-application settles is inheritance drift — an enablement cannot be assessed against
    /// a configuration still being applied, so the drift status goes back to UNASSESSED here
    /// (through [`start_enabled_baseline_change`], which every call that starts changing what
    /// an enablement has deployed shares) and [`settle`] assesses it again once the clock has
    /// made the operation due.
    ///
    /// `None` when `arn` names no stored enablement, so a handler answers the modelled
    /// `ResourceNotFoundException` and NOTHING is written. Writes no new resource, so it is
    /// not bounded by [`MAX_ENABLED_BASELINES`]; the operation it appends is bounded by the
    /// log's own retention rule.
    ///
    /// The caller refuses a reset that overlaps a baseline operation still in flight against
    /// `arn` with the modelled `ConflictException`, the same rule `UpdateEnabledBaseline`
    /// applies: neither call is `@idempotent`, so a second one starting while the first is
    /// half applied is a refusal rather than a repeat.
    ///
    /// [`settle`]: Self::settle
    /// [`record_enabled_control_reset`]: Self::record_enabled_control_reset
    /// [`start_enabled_baseline_change`]: Self::start_enabled_baseline_change
    pub fn record_enabled_baseline_reset(&mut self, arn: &str, now: SystemTime) -> Option<String> {
        if !self.enabled_baselines.contains_key(arn) {
            return None;
        }

        let operation_identifier =
            self.record_baseline_operation(BaselineOperationType::ResetEnabledBaseline, arn, now);
        self.start_enabled_baseline_change(
            arn,
            &operation_identifier,
            now + OPERATION_SETTLE_AFTER,
        )?;
        Some(operation_identifier)
    }

    /// The one span every call that starts changing what an already enabled baseline has
    /// deployed shares — applying a new configuration, re-applying the one in place, or
    /// removing it: mark the enablement `UNDER_CHANGE` until `due_at`, record
    /// `operation_identifier` as the operation changing it, and drop the drift assessment
    /// it carries.
    ///
    /// Drift is a property of a DEPLOYED configuration, and what a call like this starts is
    /// not deployed until the clock makes the operation due, so the assessment the
    /// configuration in place earned stops being reported the moment the change starts;
    /// [`settle`] assesses what is deployed once it is due, and an enablement whose change was
    /// its own removal is gone by then. The status is set here as well as derived there so a
    /// caller reading straight after the call sees the transient state.
    ///
    /// `None` when `arn` names no stored enablement, in which case nothing is written;
    /// otherwise the enablement itself, so a caller that also has a change to park has it in
    /// hand.
    ///
    /// [`settle`]: Self::settle
    fn start_enabled_baseline_change(
        &mut self,
        arn: &str,
        operation_identifier: &str,
        due_at: SystemTime,
    ) -> Option<&mut EnabledBaseline> {
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.to_string();
        enabled_baseline.due_at = due_at;
        enabled_baseline.inheritance_drift_status = None;
        enabled_baseline.source_drift_status = None;
        Some(enabled_baseline)
    }
}

/// One record of an asynchronous operation log, as the shared retention bound reads it.
///
/// Implemented by every log's record type so [`enforce_log_bound`] is written once; the
/// logs hold different record shapes but retain them by the same rule.
trait OperationLogRecord {
    /// The record's `operationIdentifier`, which is also its key in the log.
    fn identifier(&self) -> &str;
    /// When the operation started.
    fn start_time(&self) -> SystemTime;
    /// Whether the operation has yet to reach a terminal status.
    fn is_in_flight(&self) -> bool;
}

impl OperationLogRecord for LandingZoneOperation {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == LandingZoneOperationStatus::InProgress
    }
}

impl OperationLogRecord for BaselineOperationRecord {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == BaselineOperationStatus::InProgress
    }
}

impl OperationLogRecord for ControlOperationRecord {
    fn identifier(&self) -> &str {
        &self.operation_identifier
    }

    fn start_time(&self) -> SystemTime {
        self.start_time
    }

    fn is_in_flight(&self) -> bool {
        self.status == ControlOperationStatus::InProgress
    }
}

/// Drop records until `log` holds at most `max_records`.
///
/// Evicts the oldest TERMINAL record first, so an operation a caller is still polling is
/// never dropped out from under it; only when every record is in flight does the oldest
/// overall go. The `start_time`/identifier pair orders the candidates totally, so which
/// record goes does not depend on the map's iteration order.
fn enforce_log_bound<R: OperationLogRecord>(log: &mut HashMap<String, R>, max_records: usize) {
    while log.len() > max_records {
        let oldest = |terminal_only: bool| {
            log.values()
                .filter(|record| !terminal_only || !record.is_in_flight())
                .min_by(|a, b| {
                    (a.start_time(), a.identifier()).cmp(&(b.start_time(), b.identifier()))
                })
                .map(|record| record.identifier().to_string())
        };
        let Some(evicted) = oldest(true).or_else(|| oldest(false)) else {
            break;
        };
        log.remove(&evicted);
    }
}

/// Drop every record of `log` that started before `earliest_available`.
///
/// The age bound a reporting operation's documented availability window is, written once
/// for every log that carries one: the logs hold different record shapes but date a record
/// by the same `start_time`.
fn expire_log<R: OperationLogRecord>(log: &mut HashMap<String, R>, earliest_available: SystemTime) {
    log.retain(|_, record| record.start_time() >= earliest_available);
}

/// Whether any operation in `operations` against `arn` is still `IN_PROGRESS`.
///
/// The single definition of "in flight": `settle` derives a landing zone's status from it
/// while holding a mutable borrow of the landing zone map, which is why it takes the
/// operation map rather than `&self`.
fn any_operation_in_flight(
    operations: &HashMap<String, LandingZoneOperation>,
    arn: &str,
) -> bool {
    operations
        .values()
        .any(|op| op.landing_zone_arn == arn && op.status == LandingZoneOperationStatus::InProgress)
}

/// How many identifiers this process has minted, mixed into the next one so two
/// identifiers taken within the same clock tick still differ.
static IDENTIFIER_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// This process's identifier seed, taken once.
///
/// `RandomState` seeds its hasher keys from the operating system, so hashing a fixed
/// value through a fresh instance yields a number this process did not choose and a
/// restarted process does not repeat. That is the entropy an identifier needs: it must
/// not be guessable from the request, and it is not a credential.
fn identifier_seed() -> u64 {
    static SEED: ::std::sync::OnceLock<u64> = ::std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        use ::std::hash::{BuildHasher, Hasher};
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write(b"com.amazonaws.controltower identifier seed");
        hasher.finish()
    })
}

/// 16 fresh bytes nothing in the request determines: the entropy every identifier this
/// service mints is drawn from.
///
/// The single entropy source in this service, shared by both identifier FORMS
/// ([`new_operation_identifier`] and [`new_resource_id`]) so neither can drift from the
/// other's uniqueness guarantee. The bytes are the first half of a SHA-256 over this
/// process's seed, the mint counter and the clock: the counter makes two identifiers
/// minted in one tick distinct, the clock separates two processes that drew the same
/// seed, and the hash spreads all three over the whole width. Nothing from the request
/// reaches it, so an identifier is never derivable from the call that asked for it.
fn fresh_identifier_bytes() -> [u8; 16] {
    use ::sha2::Digest as _;

    let counter = IDENTIFIER_SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0u128, |since_epoch| since_epoch.as_nanos());

    let mut hasher = ::sha2::Sha256::new();
    hasher.update(identifier_seed().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes
}

/// How many characters a minted resource id carries, and the alphabet it draws them
/// from: the form `BaselineArn`'s pattern spells out for this service's resource ids,
/// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`.
const RESOURCE_ID_LENGTH: usize = 16;

/// The `[A-Z0-9]` characters of a resource id, digits first so the value's lowest
/// base-36 digit maps to `'0'`.
const RESOURCE_ID_ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// A fresh resource id for a resource THIS ACCOUNT holds — a landing zone, an enabled
/// control, an enabled baseline: [`RESOURCE_ID_LENGTH`] characters of `[A-Z0-9]`.
///
/// This service's resource ids and its `OperationIdentifier`s are different kinds of
/// token: the model constrains an operation identifier as a hyphenated lowercase-hex
/// UUID and a resource id (`BaselineArn`) as 16 characters of `[A-Z0-9]`, so a resource
/// id must never be minted in the operation-identifier form or the two become
/// confusable in one response body. The id is the base-36 rendering of
/// [`fresh_identifier_bytes`] read low digit first, keeping the full width's entropy
/// and the ARN inside `Arn`'s `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern.
pub fn new_resource_id() -> String {
    let mut value = u128::from_le_bytes(fresh_identifier_bytes());
    let mut id = String::with_capacity(RESOURCE_ID_LENGTH);
    for _ in 0..RESOURCE_ID_LENGTH {
        let digit = (value % RESOURCE_ID_ALPHABET.len() as u128) as usize;
        value /= RESOURCE_ID_ALPHABET.len() as u128;
        id.push(char::from(RESOURCE_ID_ALPHABET[digit]));
    }
    id
}

/// A fresh `OperationIdentifier`: the lowercase-hex hyphenated UUID form the model's
/// pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, len 36), with
/// RFC 4122's version-4 and variant bits set.
///
/// Drawn from [`fresh_identifier_bytes`], so two operation identifiers minted in the
/// same clock tick still differ and neither is derivable from the request.
pub fn new_operation_identifier() -> String {
    let mut bytes = fresh_identifier_bytes();
    // RFC 4122 §4.4: version 4 in the high nibble of octet 6, variant 0b10 in the two
    // high bits of octet 8 — the form the model's pattern describes.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut identifier = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        // Hyphens after octets 4, 6, 8 and 10, giving the 8-4-4-4-12 grouping.
        if matches!(index, 4 | 6 | 8 | 10) {
            identifier.push('-');
        }
        use ::std::fmt::Write as _;
        let _ = write!(identifier, "{byte:02x}");
    }
    identifier
}

/// `arn:aws:controltower:<region>:<account>:enabledbaseline/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal. Satisfies `Arn`'s
/// `^arn:aws[0-9a-zA-Z_\-:\/]+$` pattern and its 20..2048 length.
pub fn enabled_baseline_arn(region: &str, enabled_baseline_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("enabledbaseline/{enabled_baseline_id}"),
    )
}

/// A fresh `EnabledBaseline` resource id, used as the ARN's resource segment.
///
/// This service's resource-id form ([`new_resource_id`]), not an operation
/// identifier's.
pub fn new_enabled_baseline_id() -> String {
    new_resource_id()
}

/// One entry of this service's BASELINE CATALOG: an AWS-managed baseline a caller may
/// enable on a target, as `GetBaseline` reports it.
///
/// A baseline is NOT an account resource — no call creates, changes or deletes one — so
/// the catalog is service-managed data ([`BASELINE_CATALOG`]) rather than a map in the
/// per-account store, the same way the landing zone version catalog is the constant
/// [`CATALOG_LANDING_ZONE_VERSION`] rather than something a caller writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogBaseline {
    /// The ARN's resource segment, which is the baseline's identity: 16 characters of
    /// `[A-Z0-9]`, from `BaselineArn`'s pattern
    /// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`.
    ///
    /// AWS-owned and fixed per baseline — the identifier clients and templates embed and
    /// hand back to `GetBaseline`/`EnableBaseline` — so it is carried verbatim in
    /// [`BASELINE_CATALOG`] rather than generated.
    pub id: &'static str,
    /// `GetBaselineOutput$name` / `BaselineSummary$name` — "The human-readable name of a
    /// Baseline."
    pub name: &'static str,
    /// `GetBaselineOutput$description` / `BaselineSummary$description` — "A summary
    /// description of a Baseline."
    pub description: &'static str,
    /// The `baselineVersion`s this baseline OFFERS, oldest first.
    ///
    /// `BaselineVersion` (len 1..10, pattern `^\d+(?:\.\d+){0,2}$`) bounds the SHAPE of a
    /// version string, not the set of versions a baseline has, and the member is "the
    /// specific version to be enabled of the specified baseline" (model docs): a baseline
    /// is enabled at a version it offers, so a value outside this set names no deployable
    /// configuration and is refused rather than persisted — the same rule
    /// [`CATALOG_LANDING_ZONE_VERSIONS`] carries one level up.
    ///
    /// Unlike the ids, names and descriptions beside them — which are AWS's own published
    /// catalog values — these sets are this emulator's own publication: a baseline's
    /// version list moves with AWS's releases. The `4.0` of AWS's own `EnableBaseline`
    /// example is kept for the baseline that example enables, every baseline starts at
    /// `1.0`, and each entry satisfies `BaselineVersion`.
    pub versions: &'static [&'static str],
    /// The `EnabledBaselineParameter` keys this baseline DECLARES, in a stable order.
    ///
    /// `EnabledBaselineParameter$key` is "a string denoting the parameter applied to the
    /// Baseline" — a parameter the baseline itself declares, not a name the caller
    /// invents, so a key outside this set configures nothing and is refused.
    /// `IdentityCenterEnabledForBaseline` is the one the AWS Control Tower documentation
    /// publishes, on the baseline that governs an organizational unit's accounts; a
    /// baseline that declares none carries an EMPTY set, and any `parameters` entry at
    /// all is then refused for it.
    pub parameters: &'static [&'static str],
    /// Whether AWS Control Tower detects DRIFT on this baseline at all.
    ///
    /// `EnabledBaselineDriftTypes`, the shape that declares both dimensions a baseline's
    /// drift is reported in, names the set exactly: Control Tower "detects
    /// inheritance drift on the enabled baselines that target OUs:
    /// `AWSControlTowerBaseline` and `BackupBaseline`" and "does not detect drift on the
    /// baselines that apply to your landing zone: `IdentityCenterBaseline`,
    /// `AuditBaseline`, `LogArchiveBaseline`, `BackupCentralVaultBaseline`, or
    /// `BackupAdminBaseline`" (model docs). It is a property of the BASELINE, not of one
    /// enablement, which is why it lives in the catalog beside the versions and parameters
    /// a baseline publishes; an enablement of a baseline this is `false` for reports no
    /// `driftStatusSummary` at all.
    pub detects_drift: bool,
    /// Whether enabling this baseline is what brings a target under AWS Control Tower's
    /// GOVERNANCE.
    ///
    /// One baseline sets up the resources and permissions AWS Control Tower requires to
    /// govern the accounts of a target organizational unit — enabling it on a target is how
    /// a caller brings that target under governance — and governance is what makes the
    /// service apply the mandatory controls to the target itself
    /// ([`apply_governance_controls`]). It is a property of the BASELINE, not of one
    /// enablement, so it lives in the catalog beside the rest of what a baseline publishes.
    /// The baselines that configure a landing zone's shared accounts, and the ones that
    /// apply backup configuration, govern nothing and so establish none.
    ///
    /// [`apply_governance_controls`]: ComAmazonawsControltowerAwscontroltowerapisState::apply_governance_controls
    pub establishes_governance: bool,
}

impl CatalogBaseline {
    /// Whether this baseline offers `version`.
    ///
    /// The membership test behind every `baselineVersion` refusal. Matched exactly, like
    /// a landing zone version: a baseline publishes version strings, and one differing in
    /// a character names a version it does not have.
    pub fn offers_version(&self, version: &str) -> bool {
        self.versions.contains(&version)
    }

    /// Whether this baseline declares `key` as an enablement parameter.
    pub fn declares_parameter(&self, key: &str) -> bool {
        self.parameters.contains(&key)
    }
}

/// The baselines AWS Control Tower publishes, in the order `ListBaselines` reports them.
///
/// Every member of an entry — the `name`, the `description` and the ARN's 16-character id
/// segment — is AWS-OWNED catalog data, not an emulator choice: the catalog is the same in
/// every partition, region and account, a client discovers a baseline by name or ARN and
/// then enables it by that ARN, and `GetBaseline` is expected to answer the name and
/// description AWS publishes for the ARN it is handed. So the entries below carry AWS's own
/// values: the id segments clients and templates embed, and each baseline's fixed
/// description text, reproduced rather than summarised. The id segment is fixed per
/// baseline, which is why nothing here is minted per request — only the partition and
/// region segments of a baseline ARN come from the request ([`baseline_arn`]).
///
/// The remaining members are this service's own model of what a baseline OFFERS (the
/// versions it may be enabled at, the parameter keys it declares, whether AWS Control Tower
/// assesses drift on it and whether enabling it brings a target under
/// governance); see [`CatalogBaseline`] for the rule behind each.
///
/// Every description below is AWS's own sentence for that baseline, reproduced verbatim —
/// fixed text with no request-, state- or run-derived part — so none of it is summarised,
/// shortened or re-worded here.
pub const BASELINE_CATALOG: &[CatalogBaseline] = &[
    CatalogBaseline {
        id: "4T4HA1KMO10S6311",
        name: "AuditBaseline",
        description: "Sets up resources to monitor security and compliance of accounts \
                      in your organization.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "17BSJV3IGJ2QSGA2",
        name: "AWSControlTowerBaseline",
        description: "Sets up resources and mandatory controls for member accounts \
                      within the target OU, required for AWS Control Tower governance.",
        versions: &["1.0", "2.0", "3.0", "4.0"],
        parameters: &["IdentityCenterEnabledForBaseline"],
        detects_drift: true,
        // The one baseline that governs a target organizational unit's accounts, which is
        // what registering that target with AWS Control Tower means.
        establishes_governance: true,
    },
    CatalogBaseline {
        id: "H6C5JFCJJ3CPU3J5",
        name: "BackupAdminBaseline",
        description: "Sets up AWS Backup Audit Manager.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "APO9ATVPBKFRRGLK",
        name: "BackupBaseline",
        description: "Sets up a local AWS Backup vault and attaches multiple AWS Backup \
                      plans.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: true,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "3WPD0NA6TJ9AOMU2",
        name: "BackupCentralVaultBaseline",
        description: "Sets up a central AWS Backup vault in your organization.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "YX7VMZML5IG8EJUD",
        name: "CentralConfigBaseline",
        description: "Sets up central resources for compliance monitoring and auditing \
                      within your organization using AWS Config.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "A6EEAE10F08193F2",
        name: "CentralSecurityRolesBaseline",
        description: "Sets up central resources for security monitoring within your \
                      organization.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "1QBGH2G48YVGDQ3Y",
        name: "ConfigBaseline",
        description: "Sets up AWS Config resources for member accounts within the target \
                      OU, required for Detective Controls enablement. The resources set \
                      up are a subset of resources of AWSControlTowerBaseline.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "LN25R72TTG6IGPTQ",
        name: "IdentityCenterBaseline",
        description: "Sets up shared resources for AWS Identity Center, which prepares \
                      the AWSControlTowerBaseline to set up Identity Center access for \
                      accounts.",
        versions: &["1.0", "4.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
    CatalogBaseline {
        id: "J8HX46AHS5MIKQPD",
        name: "LogArchiveBaseline",
        description: "Sets up a central repository for logs of API activities and \
                      resource configurations from accounts in your organization.",
        versions: &["1.0"],
        parameters: &[],
        detects_drift: false,
        establishes_governance: false,
    },
];

/// `arn:aws:controltower:<region>::baseline/<id>` — the ARN this endpoint serves a
/// catalog baseline under.
///
/// The account segment is EMPTY, which is why this is NOT built through
/// `engine::emulator_arn` the way the per-account resources below are: a baseline is
/// AWS-managed catalog data that no account owns, so there is no account to name in its
/// ARN. `BaselineArn`'s pattern
/// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` admits ZERO
/// digits in the account position for exactly that reason, and the account-less form is
/// the one a caller round-trips back into `GetBaseline` or `EnableBaseline`.
///
/// The resources an account DOES own keep the emulator's canonical account id
/// ([`landing_zone_arn`], [`enabled_baseline_arn`], [`enabled_control_arn`]): an
/// enablement belongs to the caller's account, a baseline belongs to none.
pub fn baseline_arn(region: &str, baseline_id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::baseline/{baseline_id}")
}

/// The catalog entry a `baselineIdentifier` names, or `None` when the identifier is not a
/// baseline ARN or the catalog holds no baseline with its resource id.
///
/// A baseline's identity is the 16-character resource id after `baseline/`: the same
/// catalog is published in every partition, region and account, so the segments before
/// it select nothing and are not compared beyond being the ones a baseline ARN carries
/// ([`is_baseline_arn`]). The id itself is matched exactly, so a value differing from a
/// catalog id in one character names no baseline.
pub fn resolve_baseline(baseline_identifier: &str) -> Option<&'static CatalogBaseline> {
    if !is_baseline_arn(baseline_identifier) {
        return None;
    }
    let (_, baseline_id) = baseline_identifier.rsplit_once(":baseline/")?;
    BASELINE_CATALOG
        .iter()
        .find(|baseline| baseline.id == baseline_id)
}

/// Whether AWS Control Tower detects drift on the baseline `baseline_identifier` names,
/// read off the catalog ([`CatalogBaseline::detects_drift`]).
///
/// The one place that question is asked of a stored enablement, so `settle` and every
/// reporting call agree on which enablements carry a `driftStatusSummary` — in either
/// dimension the model declares for a baseline. An identifier naming no catalog baseline
/// assesses nothing — no enablement can hold one, since `EnableBaseline` refuses an
/// unpublished baseline before anything is persisted.
pub fn assesses_drift(baseline_identifier: &str) -> bool {
    resolve_baseline(baseline_identifier).is_some_and(|baseline| baseline.detects_drift)
}

/// Whether `identifier` is a baseline ARN at all.
///
/// The model's own `BaselineArn` form,
/// `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`, checked
/// segment by segment. `GetBaseline$baselineIdentifier` carries that type so its front
/// door answers this question, but `EnableBaseline$baselineIdentifier` is typed as the
/// generic `Arn` (pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`), which cannot express "is a
/// baseline" — so an ARN of some other service, or one naming some other kind of
/// resource, reaches a handler and is told apart here.
pub fn is_baseline_arn(identifier: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(identifier) else {
        return false;
    };
    !partition.is_empty()
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && service == ARN_NAMESPACE
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && account.len() <= 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && resource.strip_prefix("baseline/").is_some_and(|id| {
            id.len() == 16
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
}

/// How AWS Control Tower IMPLEMENTS a control: the mechanism it deploys to a target to put
/// the control into effect.
///
/// A published property of the control itself, not of one enablement, and the one thing
/// `ResetEnabledControl` is restricted by — it "does not work for controls implemented with
/// SCPs" — so it belongs beside the control's other published facts in [`CONTROL_CATALOG`]
/// and is read off the catalog entry a `controlIdentifier` resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlImplementation {
    /// A preventive control: AWS Control Tower puts it into effect by attaching a service
    /// control policy to the target, so what enforces the control is the policy.
    ServiceControlPolicy,
    /// A detective control: AWS Control Tower puts it into effect by deploying an AWS
    /// Config rule that assesses the resources in the target.
    ConfigRule,
}

/// One entry of this service's CONTROL catalog: a control AWS Control Tower publishes,
/// which a caller names with a `controlIdentifier`.
///
/// A control is NOT an account resource — no call creates, changes or deletes one, and the
/// same controls exist in every account — so the catalog is service-managed data
/// ([`CONTROL_CATALOG`]) rather than a map in the per-account store, exactly like
/// [`BASELINE_CATALOG`] one level up. What an account owns is the ENABLEMENT of a control
/// on a target, which is what `enabled_controls` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogControl {
    /// The control's NAME, which is the resource segment of its AWS Control Tower ARN
    /// (`arn:aws:controltower:<region>::control/<name>`) and the control's identity.
    ///
    /// These names are the ones the AWS Control Tower controls reference guide publishes
    /// for its `AWS-GR_`-prefixed controls. A control whose published name carries a
    /// character `ControlIdentifier`'s pattern (`^arn:aws[0-9a-zA-Z_\-:\/]+$`) does not
    /// admit — a dot, say — cannot reach a handler at all, so the catalog publishes none.
    pub name: &'static str,
    /// The resource segment of the SAME control's AWS Control Catalog ARN
    /// (`arn:<partition>:controlcatalog:::control/<id>`), the newer form of the identifier
    /// a caller may name a control by.
    ///
    /// These ids are opaque in AWS — nothing published derives one from a control's name —
    /// and this emulator publishes its own: chosen once and never minted per request, so an
    /// identifier a caller stores keeps resolving for the life of this emulator. Each is 26
    /// lowercase alphanumeric characters, the form AWS Control Catalog control ARNs carry.
    pub catalog_id: &'static str,
    /// Whether `EnableControl` may enable this control.
    ///
    /// `EnableControlInput$controlIdentifier` is documented as "Only Strongly recommended
    /// and Elective controls are permitted, with the exception of the Region deny
    /// control": the MANDATORY controls AWS Control Tower applies to every governed
    /// account itself are named by a `controlIdentifier` — `ListEnabledControls` reports
    /// them, as the enablements the service writes for a target brought under its
    /// governance ([`apply_governance_controls`]) — but cannot be enabled by a caller, so a
    /// request naming one is refused rather than persisted. False for exactly those.
    ///
    /// [`apply_governance_controls`]: ComAmazonawsControltowerAwscontroltowerapisState::apply_governance_controls
    pub enablable: bool,
    /// The mechanism AWS Control Tower implements this control with.
    ///
    /// `ResetEnabledControl` "does not work for controls implemented with SCPs", so a reset
    /// of an enablement of a control carrying [`ControlImplementation::ServiceControlPolicy`]
    /// is refused rather than started. The preventive controls — the ones whose published
    /// behaviour is to disallow or prohibit an action, the Region deny control among them —
    /// are the ones AWS Control Tower implements with a service control policy; the
    /// detective controls, which report on resources rather than block anything, are
    /// implemented with an AWS Config rule.
    pub implementation: ControlImplementation,
    /// The `EnabledControlParameter` keys this control DECLARES, in a stable order.
    ///
    /// `EnabledControlParameter$key` is "a string denoting the parameter applied to the
    /// control" — a parameter the control itself declares, not a name the caller invents,
    /// so a key outside this set configures nothing and is refused. Most AWS Control Tower
    /// controls take no configuration at all and carry an EMPTY set, for which any
    /// `parameters` entry is refused; the Region deny control is the one the API
    /// reference's own `EnableControl` example configures, with `AllowedRegions`, beside
    /// the exemption keys its documentation publishes. Like the ids, this set is this
    /// emulator's publication and moves with AWS's releases.
    pub parameters: &'static [&'static str],
}

impl CatalogControl {
    /// Whether this control declares `key` as an enablement parameter.
    pub fn declares_parameter(&self, key: &str) -> bool {
        self.parameters.contains(&key)
    }

    /// Whether AWS Control Tower implements this control with a service control policy,
    /// which is the property `ResetEnabledControl` is restricted by.
    pub fn implemented_with_service_control_policy(&self) -> bool {
        self.implementation == ControlImplementation::ServiceControlPolicy
    }
}

/// The controls this emulator publishes, in a stable order.
///
/// A mix of the three kinds of control the AWS Control Tower controls reference guide
/// publishes — the mandatory ones the service applies itself, the strongly recommended and
/// elective ones a caller enables, and the Region deny control — so that every rule
/// `EnableControl` applies to a `controlIdentifier` has catalog entries on both sides of
/// it. It is not AWS's whole catalogue of hundreds of controls: a name outside this set
/// names no control this emulator has.
///
/// Both implementations are published on both sides of `enablable`, so every rule a call
/// applies to a control's IMPLEMENTATION — `ResetEnabledControl`'s restriction to controls
/// not implemented with SCPs — has catalog entries on both sides of it too. Which
/// implementation an entry carries follows the behaviour the controls reference guide
/// publishes for that control: a control that blocks an action is preventive, hence a
/// service control policy, while one that only reports on resources is detective, hence an
/// AWS Config rule — a distinction the names do not settle, since the detective
/// `AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED` reports on buckets rather than blocking
/// anything.
pub const CONTROL_CATALOG: &[CatalogControl] = &[
    // The mandatory controls: AWS Control Tower applies these to every governed account
    // itself, so a caller may not enable them.
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_DELETION_PROHIBITED",
        catalog_id: "5kvme4m5d2b4d7dcf0fqkdmxt3",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_ENCRYPTION_ENABLED",
        catalog_id: "9h2jr6twx1pzn4bq8sm0lyvke5",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_AUDIT_BUCKET_POLICY_CHANGES_PROHIBITED",
        catalog_id: "3fxn7cgp5rmbt8wzd1qkj4shvy",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_CHANGE_PROHIBITED",
        catalog_id: "2mpr9vxtc6gln6tjb5hwqz0dkf",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_ENABLED",
        catalog_id: "1ysc3nmpx8rvg0kdwt2fqhj7zm",
        enablable: false,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_VALIDATION_ENABLED",
        catalog_id: "7dcf0fqkdmxt35kvme4m5d2b4d",
        enablable: false,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CONFIG_CHANGE_PROHIBITED",
        catalog_id: "4bq8sm0lyvke59h2jr6twx1pzn",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_CONFIG_ENABLED",
        catalog_id: "8wzd1qkj4shvy3fxn7cgp5rmbt",
        enablable: false,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_IAM_ROLE_CHANGE_PROHIBITED",
        catalog_id: "6tjb5hwqz0dkf2mpr9vxtc6gln",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_LOG_GROUP_POLICY",
        catalog_id: "0kdwt2fqhj7zm1ysc3nmpx8rvg",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_SNS_SUBSCRIPTION_CHANGE_PROHIBITED",
        catalog_id: "5kvme4m5d2b4d9h2jr6twx1pzn",
        enablable: false,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    // The strongly recommended and elective DETECTIVE controls, which a caller enables on a
    // target and which report on the resources in it through an AWS Config rule.
    CatalogControl {
        name: "AWS-GR_ENCRYPTED_VOLUMES",
        catalog_id: "7dcf0fqkdmxt34bq8sm0lyvke5",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_EBS_OPTIMIZED_INSTANCE",
        catalog_id: "9h2jr6twx1pzn3fxn7cgp5rmbt",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_EC2_VOLUME_INUSE_CHECK",
        catalog_id: "4bq8sm0lyvke58wzd1qkj4shvy",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RDS_INSTANCE_PUBLIC_ACCESS_CHECK",
        catalog_id: "3fxn7cgp5rmbt2mpr9vxtc6gln",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RDS_STORAGE_ENCRYPTED",
        catalog_id: "8wzd1qkj4shvy6tjb5hwqz0dkf",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RESTRICTED_COMMON_PORTS",
        catalog_id: "2mpr9vxtc6gln1ysc3nmpx8rvg",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_ROOT_ACCOUNT_MFA_ENABLED",
        catalog_id: "6tjb5hwqz0dkf0kdwt2fqhj7zm",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED",
        catalog_id: "1ysc3nmpx8rvg5kvme4m5d2b4d",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_S3_BUCKET_PUBLIC_WRITE_PROHIBITED",
        catalog_id: "0kdwt2fqhj7zm7dcf0fqkdmxt3",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS",
        catalog_id: "5kvme4m5d2b4d4bq8sm0lyvke5",
        enablable: true,
        implementation: ControlImplementation::ConfigRule,
        parameters: &[],
    },
    // The strongly recommended and elective PREVENTIVE controls: a caller enables these
    // too, and AWS Control Tower blocks the actions they name with a service control
    // policy.
    CatalogControl {
        name: "AWS-GR_RESTRICT_ROOT_USER",
        catalog_id: "7dcf0fqkdmxt33fxn7cgp5rmbt",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_RESTRICT_ROOT_USER_ACCESS_KEYS",
        catalog_id: "9h2jr6twx1pzn8wzd1qkj4shvy",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_CROSS_REGION_NETWORKING",
        catalog_id: "4bq8sm0lyvke52mpr9vxtc6gln",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_VPC_INTERNET_ACCESS",
        catalog_id: "3fxn7cgp5rmbt6tjb5hwqz0dkf",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    CatalogControl {
        name: "AWS-GR_DISALLOW_VPN_CONNECTIONS",
        catalog_id: "8wzd1qkj4shvy1ysc3nmpx8rvg",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &[],
    },
    // The Region deny control: permitted by the member's own documentation despite not
    // being one of the strongly recommended or elective controls, the one control this
    // catalog publishes with parameters, and preventive — it denies actions outside the
    // Regions it allows, which a service control policy is what enforces.
    CatalogControl {
        name: "AWS-GR_REGION_DENY",
        catalog_id: "2mpr9vxtc6gln0kdwt2fqhj7zm",
        enablable: true,
        implementation: ControlImplementation::ServiceControlPolicy,
        parameters: &["AllowedRegions", "ExemptedPrincipalArns", "ExemptedActions"],
    },
];

/// `arn:aws:controltower:<region>::control/<CONTROL_NAME>` — the AWS Control Tower ARN
/// form of a catalog control, whose resource segment is the control's name.
///
/// The account segment is EMPTY for the reason [`baseline_arn`]'s is, which is also why
/// this is not built through `engine::emulator_arn` the way the per-account resources are:
/// a control is AWS-managed catalog data that no account owns, and only an account-less
/// control ARN names one. [`resolve_control`] reads this form back, so an identifier built
/// here names the control it was built from wherever a `controlIdentifier` is accepted.
pub fn control_arn(region: &str, control_name: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::control/{control_name}")
}

/// Whether `control_identifier` names a MANDATORY control: one AWS Control Tower applies
/// to every target it governs itself, which is exactly the set no caller may enable
/// ([`CatalogControl::enablable`]).
///
/// The one place that question is asked of a STORED enablement, so every call that treats
/// a control the service applied differently from one a caller enabled reads the same
/// answer. Read off the catalog entry the identifier resolves to rather than off its text,
/// because one control is published under two ARN forms; an identifier naming no catalog
/// control is not mandatory, and no stored enablement carries one.
pub fn is_mandatory_control(control_identifier: &str) -> bool {
    resolve_control(control_identifier).is_some_and(|control| !control.enablable)
}

/// Why the control `control_identifier` names is not a caller's to act on, or `None` when it
/// names a control that is.
///
/// A MANDATORY control is the SERVICE's enablement, not a caller's resource
/// ([`is_mandatory_control`]), so no call a caller makes against one — enabling it,
/// disabling it, changing its configuration or re-deploying it — is accepted. This is the
/// one place that refusal's wording is built, so each of those calls refuses the same
/// control in the same words, differing only in `action`: the past participle of what the
/// call would have done ("enabled", "disabled", "updated", "reset"), as the sentence reads
/// it back. The exception named is the Region deny control, which
/// `EnableControlInput$controlIdentifier` documents as permitted and which this catalog
/// publishes as enablable.
pub fn mandatory_control_message(control_identifier: &str, action: &str) -> Option<String> {
    let mandatory = resolve_control(control_identifier).filter(|control| !control.enablable)?;
    Some(format!(
        "control {} is a mandatory control, which AWS Control Tower applies itself; only \
         strongly recommended and elective controls may be {action}, with the exception of \
         the Region deny control",
        mandatory.name
    ))
}

/// The catalog entry a `controlIdentifier` names, or `None` when the identifier is not a
/// control ARN or the catalog holds no control it names.
///
/// A control is named by either of the two ARN forms AWS publishes for it — the AWS
/// Control Tower form, whose resource segment is the control's NAME, and the AWS Control
/// Catalog form, whose resource segment is the control's opaque id — so both resolve to
/// the same entry here. The same catalog is published in every partition, region and
/// account, so the segments before the resource select nothing and are not compared beyond
/// being the ones a control ARN carries; the name and the id are matched exactly, so a
/// value differing from a catalog entry in one character names no control.
pub fn resolve_control(control_identifier: &str) -> Option<&'static CatalogControl> {
    match control_reference(control_identifier)? {
        ControlReference::Name(name) => CONTROL_CATALOG.iter().find(|control| control.name == name),
        ControlReference::CatalogId(id) => CONTROL_CATALOG
            .iter()
            .find(|control| control.catalog_id == id),
    }
}

/// Whether `identifier` is a control ARN at all, in either form AWS publishes.
///
/// `EnableControl$controlIdentifier` is typed as `ControlIdentifier`, whose pattern
/// (`^arn:aws[0-9a-zA-Z_\-:\/]+$`) admits any ARN of any service, so an ARN naming some
/// other kind of resource reaches the handler and is told apart here: one of these two
/// forms naming no catalog entry references a control that does not exist, while anything
/// else is not a control reference at all.
pub fn is_control_arn(identifier: &str) -> bool {
    control_reference(identifier).is_some()
}

/// Why a `controlIdentifier` a caller sent names no catalog control, carrying the wording
/// the refusal reports.
///
/// The member has exactly TWO readings when it resolves to nothing, and they are different
/// failures: a value in one of the published control ARN forms that the catalog holds no
/// entry for REFERENCES a control that does not exist, while a value that is not a control
/// reference at all never named a control and is a malformed input. Which of the two a
/// value is depends on the value alone, so it is settled here rather than at each call
/// site; every handler reading this member maps the variant to its OWN operation's modelled
/// error — the not-found error for [`Self::NoSuchControl`], the validation error for
/// [`Self::NotAControlReference`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedControlIdentifier {
    /// A control ARN, in either published form, that names no catalog entry.
    NoSuchControl(String),
    /// A value that is not a control ARN at all.
    NotAControlReference(String),
}

/// The catalog entry a caller-supplied `controlIdentifier` names, or why it names none
/// ([`UnresolvedControlIdentifier`]).
///
/// The one place a caller's `controlIdentifier` is turned into the control it names, so the
/// same value is classified the same way wherever it is read and whatever other members
/// accompany it: a request that also names the enablement another way is compared against
/// the control this resolved to, never told its identifier mismatches when the identifier
/// names no control at all.
pub fn catalogued_control(
    control_identifier: &str,
) -> Result<&'static CatalogControl, UnresolvedControlIdentifier> {
    match resolve_control(control_identifier) {
        Some(control) => Ok(control),
        None if is_control_arn(control_identifier) => Err(
            UnresolvedControlIdentifier::NoSuchControl(format!(
                "no control matches controlIdentifier {control_identifier}"
            )),
        ),
        None => Err(UnresolvedControlIdentifier::NotAControlReference(format!(
            "controlIdentifier {control_identifier:?} is not the ARN of an AWS Control \
             Tower control; a control is named by its control ARN, \
             arn:aws:controltower:<region>::control/<CONTROL_NAME>, or by its control \
             catalog ARN, arn:aws:controlcatalog:::control/<id>"
        ))),
    }
}

/// How a control ARN names its control: by name in the AWS Control Tower form, by opaque
/// id in the AWS Control Catalog form.
enum ControlReference<'a> {
    /// `arn:<partition>:controltower:<region>::control/<NAME>`.
    Name(&'a str),
    /// `arn:<partition>:controlcatalog:::control/<id>`.
    CatalogId(&'a str),
}

/// The control `identifier` references, or `None` when it is not a control ARN.
///
/// Neither form names an account: a control belongs to no account, which is why the
/// account segment must be empty in both. The Control Catalog form names no region either
/// — that catalog is global — while the Control Tower form carries the region the caller
/// read the control's ARN in.
fn control_reference(identifier: &str) -> Option<ControlReference<'_>> {
    let (partition, service, region, account, resource) = arn_segments(identifier)?;
    if partition.is_empty()
        || !partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        || !account.is_empty()
    {
        return None;
    }
    let resource = resource.strip_prefix("control/")?;
    match service {
        ARN_NAMESPACE
            if region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && is_control_name(resource) =>
        {
            Some(ControlReference::Name(resource))
        }
        "controlcatalog" if region.is_empty() && is_control_catalog_id(resource) => {
            Some(ControlReference::CatalogId(resource))
        }
        _ => None,
    }
}

/// Whether `value` has the form of an AWS Control Tower control name: a non-empty run of
/// the uppercase letters, digits, underscores and hyphens the published names are made of.
fn is_control_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Whether `value` has the form of an AWS Control Catalog control id: 26 lowercase
/// alphanumeric characters.
fn is_control_catalog_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `target` is the ARN of an AWS Organizations organizational unit.
///
/// `EnableBaseline$targetIdentifier` is "the ARN of the target on which the baseline will
/// be enabled. Only OUs are supported as targets" (model docs), and its `Arn` type cannot
/// express that, so the semantic check is here: the ARN must be an Organizations ARN
/// (`arn:<partition>:organizations::<12-digit account>:ou/<organization>/<unit>`) whose
/// resource is an organizational unit, with the `o-` / `ou-` identifier forms the AWS
/// Organizations documentation publishes. The published LENGTH windows of those
/// identifiers are not imposed — the identifiers AWS's own examples carry are shorter
/// than the windows — so what is checked is the structure: an ARN of another service,
/// another resource type, or a value that is not an organizational unit identifier at all
/// names no target a baseline can be applied to.
pub fn is_organizational_unit_arn(target: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(target) else {
        return false;
    };
    if partition.is_empty() || service != "organizations" || !region.is_empty() {
        return false;
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some((organization, organizational_unit)) = resource
        .strip_prefix("ou/")
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    organization.strip_prefix("o-").is_some_and(is_lower_alnum)
        && organizational_unit
            .strip_prefix("ou-")
            .and_then(|rest| rest.split_once('-'))
            .is_some_and(|(root, unit)| is_lower_alnum(root) && is_lower_alnum(unit))
}

/// An ARN's partition, service, region, account and resource segments, or `None` when
/// `value` is not six colon-separated segments beginning with `arn`.
///
/// The resource segment keeps whatever colons it contains, so a caller that expects a
/// slash-delimited resource rejects a colon-delimited one by matching on it.
fn arn_segments(value: &str) -> Option<(&str, &str, &str, &str, &str)> {
    let mut segments = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    )
    else {
        return None;
    };
    Some((partition, service, region, account, resource))
}

/// Whether `value` is a non-empty run of lowercase letters and digits — the character
/// class an AWS Organizations entity identifier is made of.
fn is_lower_alnum(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Why `target` cannot be the target of an enablement — a baseline's or a control's — or
/// `None` when it is an organizational unit ARN.
///
/// The one place that refusal's wording is built. The message is built from the request
/// and from the rule, so it tells the caller what kind of target it may name.
pub fn non_organizational_unit_target_message(target: &str) -> Option<String> {
    if is_organizational_unit_arn(target) {
        return None;
    }
    Some(format!(
        "targetIdentifier {target:?} is not the ARN of an organizational unit; only \
         organizational units are supported as targets, named as \
         arn:aws:organizations::<account>:ou/<organization>/<organizational unit>"
    ))
}

/// Why `version` cannot be enabled of `baseline`, or `None` when that baseline offers it.
///
/// The one place this refusal's wording is built, so `EnableBaseline` and
/// `UpdateEnabledBaseline` refuse an unoffered version in the same words, and the message
/// is built from the request and the catalog so it tells the caller which versions it may
/// ask for.
pub fn unsupported_baseline_version_message(
    baseline: &CatalogBaseline,
    version: &str,
) -> Option<String> {
    if baseline.offers_version(version) {
        return None;
    }
    Some(format!(
        "baselineVersion {version:?} is not a version baseline {} offers; the available \
         versions are {}",
        baseline.name,
        baseline.versions.join(", ")
    ))
}

/// Why `parameters` does not configure `baseline`, or `None` when it does.
///
/// The one place a baseline enablement's parameter list is checked, so `EnableBaseline`
/// and `UpdateEnabledBaseline` refuse the same lists in the same words. A list is
/// unacceptable when it names a key the baseline does not declare (it would configure
/// nothing), repeats a key (one key addresses one parameter, so a list carrying it twice
/// does not describe one configuration) or weighs more than
/// [`MAX_ENABLED_BASELINE_PARAMETER_BYTES`].
pub fn unacceptable_parameters_message(
    baseline: &CatalogBaseline,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    unacceptable_enablement_parameters_message(
        "baseline",
        baseline.name,
        baseline.parameters,
        MAX_ENABLED_BASELINE_PARAMETER_BYTES,
        parameters,
    )
}

/// Why `parameters` does not configure `control`, or `None` when it does.
///
/// The one place a control's parameter list is checked, so `EnableControl` and
/// `UpdateEnabledControl` refuse the same lists in the same words — the rule
/// [`unacceptable_parameters_message`] applies to a baseline's parameters one level up, so
/// both read it off the same walk.
pub fn unacceptable_control_parameters_message(
    control: &CatalogControl,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    unacceptable_enablement_parameters_message(
        "control",
        control.name,
        control.parameters,
        MAX_ENABLED_CONTROL_PARAMETER_BYTES,
        parameters,
    )
}

/// Why `parameters` does not configure the resource named by `kind`/`name`, or `None` when
/// it does.
///
/// The ONE rule both an enabled baseline's and an enabled control's parameter list are
/// checked by: a list is unacceptable when it names a key the resource does not declare (it
/// would configure nothing), repeats a key (one key addresses one parameter, so a list
/// carrying it twice does not describe one configuration) or weighs more than
/// `byte_cap`. `kind` is the word the refusal calls the resource, so the message names the
/// member and what that resource declares.
fn unacceptable_enablement_parameters_message(
    kind: &str,
    name: &str,
    declared: &[&str],
    byte_cap: usize,
    parameters: &[(String, ::aws_smithy_types::Document)],
) -> Option<String> {
    // One pass over a set of the keys already seen, so detecting a repeat costs
    // O(n log n) in the entry count rather than O(n^2).
    let mut seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    let mut weight = 0usize;
    for (key, value) in parameters {
        if !declared.contains(&key.as_str()) {
            return Some(if declared.is_empty() {
                format!(
                    "parameters names {key:?}, which {kind} {name} does not declare; that \
                     {kind} declares no enablement parameters"
                )
            } else {
                format!(
                    "parameters names {key:?}, which {kind} {name} does not declare; the \
                     parameters it declares are {}",
                    declared.join(", ")
                )
            });
        }
        if !seen.insert(key.as_str()) {
            return Some(format!(
                "parameters carries key {key:?} more than once; each enablement parameter \
                 key may be given at most once"
            ));
        }
        // The budget left for this value, so the whole list is measured once and the walk
        // stops at the first node that takes it past the cap.
        let budget = byte_cap.saturating_sub(weight.saturating_add(key.len()));
        let Some(value_weight) = document_weight_within(value, budget) else {
            return Some(format!(
                "parameters weighs more than the {byte_cap} bytes one {kind} enablement's \
                 parameters may hold"
            ));
        };
        weight = weight.saturating_add(key.len()).saturating_add(value_weight);
    }
    None
}

/// Whether `configured` and `requested` describe the SAME control configuration.
///
/// A parameter is addressed by its key and a list may carry a key at most once
/// ([`unacceptable_control_parameters_message`]), so a configuration is the key/value
/// mapping a list denotes and the order the entries were sent in does not distinguish two
/// configurations.
pub fn same_control_configuration(
    configured: &[(String, ::aws_smithy_types::Document)],
    requested: &[(String, ::aws_smithy_types::Document)],
) -> bool {
    fn as_map(
        parameters: &[(String, ::aws_smithy_types::Document)],
    ) -> BTreeMap<&str, &::aws_smithy_types::Document> {
        parameters
            .iter()
            .map(|(key, value)| (key.as_str(), value))
            .collect()
    }
    as_map(configured) == as_map(requested)
}

/// Whether `identifier` is a landing zone ARN at all.
///
/// A landing zone is addressed by the ARN this service reports for it — the only
/// identifier a caller is ever handed for one (`CreateLandingZoneOutput$arn`,
/// `LandingZoneSummary$arn`, both typed `Arn`: length 20..2048, pattern
/// `^arn:aws[0-9a-zA-Z_\-:\/]+$`) — and the form that ARN carries is
/// [`landing_zone_arn`]'s: `arn:<partition>:controltower:<region>:<account>:landingzone/<id>`.
/// `landingZoneIdentifier` is modelled as a plain `String`, so no front-door check
/// applies and the shape check is the handler's; it is checked segment by segment here,
/// the way [`is_baseline_arn`] checks a baseline's. The resource id itself is not
/// constrained beyond being one non-empty segment of `Arn`'s character class: the service
/// publishes no pattern for it, so a shape check must not reject an id of another form.
pub fn is_landing_zone_arn(identifier: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(identifier) else {
        return false;
    };
    (20..=2048).contains(&identifier.len())
        && partition.starts_with("aws")
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && service == ARN_NAMESPACE
        && !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && account.len() == 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && resource.strip_prefix("landingzone/").is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

/// Why `identifier` names no landing zone, or `None` when it is a landing zone ARN.
///
/// The one place this refusal's wording is built, so every operation taking a
/// `landingZoneIdentifier` refuses the same values in the same words. A value that is not
/// a landing zone ARN ([`is_landing_zone_arn`]) is a malformed input rather than a
/// reference to a resource that does not exist, so a caller refuses it with the modelled
/// `ValidationException` and keeps `ResourceNotFoundException` for an ARN of that shape
/// naming no stored landing zone. The message is built from the request and the rule, so
/// it tells the caller what it may send.
pub fn non_landing_zone_identifier_message(identifier: &str) -> Option<String> {
    if is_landing_zone_arn(identifier) {
        return None;
    }
    Some(format!(
        "landingZoneIdentifier {identifier:?} is not the ARN of an AWS Control Tower landing \
         zone; a landing zone is named by the ARN CreateLandingZone and ListLandingZones \
         report for it, of the form \
         arn:aws:controltower:<region>:<account>:landingzone/<id>"
    ))
}

/// What a `landingZoneIdentifier` that names no landing zone this account holds is refused
/// with.
///
/// The one place that refusal's wording is built, so every operation resolving a
/// `landingZoneIdentifier` — the read, the update, the reset and the decommissioning —
/// reports a miss the same way. A miss is reported as this ACCOUNT and Region holding no
/// landing zone rather than as the identifier being unknown, so the value a caller sent is
/// not what the message names: the management account is the account this emulator presents
/// and the Region is the one the request was signed for ([`request_region`]), both read off
/// the request and the emulator's own state.
pub fn unknown_landing_zone_message(region: &str) -> String {
    format!(
        "AWS Control Tower could not find a landing zone identifier value for the management \
         account: {account}, in Region {region}. To continue, provide a valid resource ARN or \
         landingZoneIdentifier string for your landing zone.",
        account = ::engine::emulator_account_id()
    )
}

/// What a baseline-ENABLEMENT call answers in an account Control Tower was never set up in
/// ([`is_set_up`]).
///
/// The baseline enablement APIs act through the service role that setting Control Tower up
/// creates, so without it the service reports that it could not assume that role rather
/// than reporting out of a store it never reached — or, for a call that would enable,
/// update, re-deploy or disable a baseline, rather than writing into it. Written here once,
/// so the reads and the writes that carry it cannot word one precondition two ways. The
/// role is the fixed service role of that name, so the whole message is fixed text; it is a
/// `ValidationException` (400), which is the only client error `ListEnabledBaselines`
/// models at all and which every baseline enablement call models.
///
/// [`is_set_up`]: ComAmazonawsControltowerAwscontroltowerapisState::is_set_up
pub const SETUP_REQUIRED_FOR_BASELINES: &str =
    "AWS Control Tower could not complete the operation because it could not assume the \
     'AWSControlTowerAdmin' role. Check the configuration for this role and try again.";

/// What a control-ENABLEMENT or control-operation call answers in an account Control Tower
/// was never set up in ([`is_set_up`]).
///
/// The control enablement APIs resolve the account's landing zone, so without one the
/// service reports the landing zone as the resource that is missing — a
/// `ResourceNotFoundException` (404) — and names the call that creates it, rather than
/// reporting out of a store it never reached, or writing into it for a call that would
/// enable, update, re-deploy or disable a control. Written here once, so every call that
/// carries it words the one precondition the same way.
///
/// [`is_set_up`]: ComAmazonawsControltowerAwscontroltowerapisState::is_set_up
pub const SETUP_REQUIRED_FOR_CONTROLS: &str =
    "AWS Control Tower cannot complete the operation, because you must create a landing \
     zone first. To continue, create your landing zone from the console, or call the \
     CreateLandingZone API.";

/// A fresh landing zone resource id, used as the ARN's resource segment.
///
/// This service's resource-id form ([`new_resource_id`]), not an operation
/// identifier's.
pub fn new_landing_zone_id() -> String {
    new_resource_id()
}

/// `arn:aws:controltower:<region>:<account>:landingzone/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal.
pub fn landing_zone_arn(region: &str, landing_zone_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("landingzone/{landing_zone_id}"),
    )
}

/// A fresh enabled-control resource id, used as the ARN's resource segment.
///
/// This service's resource-id form ([`new_resource_id`]), not an operation
/// identifier's.
pub fn new_enabled_control_id() -> String {
    new_resource_id()
}

/// `arn:aws:controltower:<region>:<account>:enabledcontrol/<id>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the emulator's
/// canonical account id rather than a literal.
pub fn enabled_control_arn(region: &str, enabled_control_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("enabledcontrol/{enabled_control_id}"),
    )
}

/// The approximate size in bytes of `document`, or `None` as soon as it exceeds `cap`.
///
/// Walks the document with an EXPLICIT stack rather than by recursion, so a deeply
/// nested value cannot overflow the thread's stack, and stops at the first node that
/// takes the running total past `cap`. A container wider than the budget left is refused
/// BEFORE its children are pushed, so both the walk and the stack it uses cost at most
/// `cap` whatever the caller sent. A scalar counts as its machine width and a string,
/// key or structural node as the bytes it holds — an approximation, which is all a bound
/// needs.
pub fn document_weight_within(document: &::aws_smithy_types::Document, cap: usize) -> Option<usize> {
    use ::aws_smithy_types::Document;

    /// What a scalar or a container node itself costs, before its contents.
    const NODE_WEIGHT: usize = 8;

    let mut total = 0usize;
    let mut pending: Vec<&Document> = vec![document];
    while let Some(node) = pending.pop() {
        let weight = match node {
            Document::Null | Document::Bool(_) | Document::Number(_) => NODE_WEIGHT,
            Document::String(value) => value.len(),
            Document::Array(items) => {
                // More children than the whole remaining budget cannot be summarised
                // within it, so the width is refused before the stack grows by it.
                if items.len() > cap.saturating_sub(pending.len()) {
                    return None;
                }
                pending.extend(items.iter());
                NODE_WEIGHT
            }
            Document::Object(members) => {
                if members.len() > cap.saturating_sub(pending.len()) {
                    return None;
                }
                let keys = members.keys().map(String::len).sum::<usize>();
                pending.extend(members.values());
                NODE_WEIGHT.saturating_add(keys)
            }
        };
        total = total.saturating_add(weight);
        if total > cap {
            return None;
        }
    }
    Some(total)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope, or one whose region is not a Region name.
///
/// This value is both built into every ARN this service mints and reported back as the
/// Region an enablement names, a member bounded by [`REGION_NAME_LENGTH`], so a
/// credential scope whose region is no Region name ([`is_region_name`]) names no region
/// at all and is read the same way as a request that carries no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_name(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `(major, minor)` of a `LandingZoneVersion`, for NUMERIC version order.
///
/// `None` when the value is not `<digits>.<digits>` — the model's pattern
/// `^\d+.\d+$` leaves the separator unescaped, so it admits values this cannot order.
fn version_order(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The later of a deployed landing zone version and the catalog's newest version.
///
/// Compared NUMERICALLY per component, so `3.10` is newer than `3.3`. A deployed
/// version at or beyond the catalog's — or one this cannot order — is its own latest
/// available version, so the answer is never older than what is deployed.
///
/// A create or update only accepts a version [`CATALOG_LANDING_ZONE_VERSIONS`] offers, so
/// in practice the answer is the catalog's newest; the comparison is what keeps the
/// invariant when the catalog is retuned around an already-deployed landing zone.
pub fn latest_available_version(deployed: &str, catalog: &str) -> String {
    match (version_order(deployed), version_order(catalog)) {
        (Some(deployed_order), Some(catalog_order)) if catalog_order > deployed_order => {
            catalog.to_string()
        }
        _ => deployed.to_string(),
    }
}

/// The enablements one listing of this service reports, as the set of their ARNs: the
/// records `scoped` selects, plus — only when `include_children` is set — every record
/// that inherited its configuration from one of those, directly or through a chain of
/// inheritance.
///
/// The one place this service answers `includeChildren`, for both of the enablement
/// listings that declare it, so a change to the rule widens both the same way. Which
/// record is a child of which is read off `parent` alone — the `parentIdentifier` the
/// records carry — so no containment is inferred from an identifier's text.
///
/// Terminates: each round either adds a record the answer did not hold or ends the walk,
/// and `records` is capped by its store's own bound ([`MAX_ENABLED_CONTROLS`],
/// [`MAX_ENABLED_BASELINES`]).
fn listed_enablements<Record>(
    records: &HashMap<String, Record>,
    arn: impl Fn(&Record) -> &str,
    parent: impl Fn(&Record) -> Option<&str>,
    scoped: impl Fn(&Record) -> bool,
    include_children: bool,
) -> BTreeSet<String> {
    let mut listed: BTreeSet<String> = records
        .values()
        .filter(|record| scoped(record))
        .map(|record| arn(record).to_string())
        .collect();

    if !include_children {
        return listed;
    }

    // One round per level of inheritance: the records whose parent is already in the
    // answer join it, until a round finds none — so a chain of any depth is included and
    // no record is added twice.
    loop {
        let inherited: Vec<String> = records
            .values()
            .filter(|record| {
                !listed.contains(arn(record))
                    && parent(record).is_some_and(|parent| listed.contains(parent))
            })
            .map(|record| arn(record).to_string())
            .collect();
        if inherited.is_empty() {
            return listed;
        }
        listed.extend(inherited);
    }
}

/// Whether a record carrying `parent` inherited its configuration from nothing: it names
/// no parent, or names one `records` does not hold.
///
/// A record whose parent is not itself stored counts as inheriting from nothing, so a
/// record whose parent has been torn down keeps being listed rather than dropping out of
/// a listing scoped to the directly applied set.
fn inherits_from_nothing<Record>(records: &HashMap<String, Record>, parent: Option<&str>) -> bool {
    parent.is_none_or(|parent| !records.contains_key(parent))
}

/// One `filter` member of a list operation, as a handler matches it: the values the
/// caller sent as a SET, or `None` when they sent that member no list at all.
///
/// A set, so testing one stored record against it costs `O(log k)` rather than a scan,
/// and the whole per-request scan is bounded by the store's own cap
/// ([`MAX_ENABLED_CONTROLS`], [`MAX_ENABLED_BASELINES`], [`MAX_CONTROL_OPERATIONS`]).
///
/// Every enum-typed filter member deserializes to a plain string, but the schema front
/// door has already refused a value the model's value set does not declare (HTTP 400, as
/// `tests/all/front_door_validation.rs` pins), so reaching a handler means each value is
/// one of the modelled ones — and these lists carry a modelled maximum length, so the
/// front door has bounded their lengths too.
pub type Filter<'a> = Option<BTreeSet<&'a str>>;

/// The values of one filter member as a [`Filter`] set, or `None` when the caller sent
/// that member no list at all.
///
/// The one place every list operation of this service turns a sent filter member into the
/// set it matches against, so they all narrow a listing by the same rule. Called BEFORE
/// the state lock is taken, so no caller-sized work happens while the lock is held.
pub fn filter_set(values: Option<&Vec<String>>) -> Filter<'_> {
    values.map(|values| values.iter().map(String::as_str).collect())
}

/// Whether `actual` is selected by a filter member: a member the caller did not send
/// selects every record, and one they did selects the records whose value its set holds.
pub fn selected(filter: &Filter<'_>, actual: &str) -> bool {
    filter.as_ref().is_none_or(|values| values.contains(actual))
}

/// As [`selected`], for a member a record may have no value for at all: an absent value
/// is selected only by an absent filter.
///
/// So filtering on a member only some records carry (a parent identifier, a drift status
/// that is assessed for some resources only) returns just the records that have a value
/// and whose value is among the ones asked for — and a filter on a member NO record here
/// carries selects nothing, which is [`selected_optional(filter, None)`].
///
/// [`selected_optional(filter, None)`]: selected_optional
pub fn selected_optional(filter: &Filter<'_>, actual: Option<&str>) -> bool {
    match (filter.as_ref(), actual) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(values), Some(actual)) => values.contains(actual),
    }
}

/// As [`selected`], for a filter member whose values REFERENCE a resource this service
/// resolves (a baseline, a control) rather than carrying its identity directly.
///
/// The caller's values and the record's own value are both put through `resolve` before
/// they are compared, so a value written in any of the equivalent forms that name one
/// resource selects every record naming that resource — which is the same rule the
/// resolution itself follows ([`resolve_baseline`], [`resolve_control`]). A filter value
/// that names no resource at all selects nothing, since every record here was written
/// for a resource this service publishes.
pub fn selected_resolved<Entry: PartialEq>(
    filter: &Filter<'_>,
    actual: &str,
    resolve: impl Fn(&str) -> Option<Entry>,
) -> bool {
    let Some(values) = filter.as_ref() else {
        return true;
    };
    let Some(actual) = resolve(actual) else {
        return false;
    };
    values
        .iter()
        .any(|value| resolve(value).as_ref() == Some(&actual))
}

/// The parameters a list call narrowed its listing by, in the canonical form the
/// `nextToken` minted from that listing is bound to.
///
/// A token is a position in the listing the call that handed it out was looking at, so it
/// resumes nothing in a listing selected differently: the position it names would be a
/// position in a DIFFERENT sequence of records, and reading it as one would serve a page
/// out of the middle of that sequence and never serve the records before it. A handler
/// therefore states here every request member that narrows what it lists, and [`page`]
/// binds the token it mints to them — which is what `nextToken` is documented to require
/// ("the token to continue the list from a previous API call with the same parameters").
///
/// The rendering is INJECTIVE: each member writes its name and each value with its byte
/// length, and a member the caller omitted is distinguished from one they sent no value
/// for, so no two different narrowings render alike. The values of a list-valued member
/// are written as the SET a handler matches against ([`Filter`]), so repeating one value
/// or sending the same values in another order carries the same key — those calls list the
/// same records in the same order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListingKey {
    rendered: String,
}

impl ListingKey {
    /// The key of a call that has narrowed its listing by nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one list-valued narrowing member — a `filter` member — under `member`.
    pub fn filter(mut self, member: &str, values: &Filter<'_>) -> Self {
        self.push_member(member);
        match values.as_ref() {
            None => self.rendered.push('-'),
            Some(values) => {
                for value in values {
                    self.push_value(value);
                }
            }
        }
        self.rendered.push(';');
        self
    }

    /// Record one single-valued narrowing member under `member`: an absent value narrows
    /// nothing, and is not the same key as any value the caller could have sent.
    pub fn value(mut self, member: &str, value: Option<&str>) -> Self {
        self.push_member(member);
        match value {
            None => self.rendered.push('-'),
            Some(value) => self.push_value(value),
        }
        self.rendered.push(';');
        self
    }

    /// Record one boolean narrowing member under `member`. A modelled default makes the
    /// member present whether or not the caller sent it, so an omitted member and the
    /// default written out carry the same key.
    pub fn flag(self, member: &str, value: bool) -> Self {
        self.value(member, Some(if value { "true" } else { "false" }))
    }

    fn push_member(&mut self, member: &str) {
        let _ = write!(self.rendered, "{}:{member}=", member.len());
    }

    fn push_value(&mut self, value: &str) {
        let _ = write!(self.rendered, "{}:{value},", value.len());
    }
}

/// How far a `nextToken` one list operation handed out reaches, which is both what the
/// token is bound to and what its refusal tells a caller that sent a token it cannot
/// resume.
///
/// A token names a position in the listing the call that handed it out was looking at, so a
/// listing the request narrows (by a `filter` member, by a target) only resumes for a call
/// that repeats those members — which is what `nextToken` is documented to require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope<'params> {
    /// The operation lists ONE whole collection, so its token means the same to every call.
    WholeListing,
    /// The operation narrows its listing from the request, so its token is only meaningful
    /// to a call that sent the same parameters — the ones the handler states here.
    SameParameters(&'params ListingKey),
}

/// The page of `listing` a list call asks for and the `nextToken` to report with it, or
/// the message refusing the `nextToken` the call sent.
///
/// The ONE place every list operation of this service cuts a page out of its listing, so a
/// page boundary cannot drift between them. The rule: an absent or empty token starts at
/// the beginning, any other token resumes at the first entry the previous page did not
/// return — but only a token THIS listing handed out does ([`ListingCursor`]), so a token
/// minted by another operation, or by a call that narrowed its listing differently, is
/// refused rather than read as a position in a listing it names no position in;
/// `maxResults` is how many entries the page holds, every remaining entry when the caller
/// sent none; and the response carries a token exactly while entries remain after the
/// page. `operation` and `scope` are what the token is bound to and what the refusal names
/// (see [`TokenScope`]).
///
/// The floor of 1 on `maxResults` keeps a page from being empty-but-tokened should a
/// model's range ever admit a smaller value than the front door refuses today, and the
/// saturating addition keeps a page size near the integer's maximum from wrapping.
pub fn page<'entries, Entry>(
    operation: &str,
    scope: TokenScope<'_>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    listing: &'entries [Entry],
) -> Result<(&'entries [Entry], Option<String>), String> {
    let cursor = ListingCursor::of(operation, scope);
    let len = listing.len();
    let start = match next_token {
        None | Some("") => 0usize,
        Some(token) => match cursor.position(token) {
            Some(index) if index <= len => index,
            _ => return Err(unhandled_next_token_message(operation, scope, token)),
        },
    };
    let limit = max_results.map_or(len, |max| max.max(1) as usize);
    let end = start.saturating_add(limit).min(len);
    Ok((&listing[start..end], (end < len).then(|| cursor.token(end))))
}

/// The separator between the two parts of a minted token, chosen outside the digits an
/// index is written with so the parts cannot run together.
const TOKEN_PART_SEPARATOR: char = '.';

/// The listing a `nextToken` names a position in: mints the token a page reports and reads
/// back only a token minted for THIS listing.
///
/// A token names a position in a SEQUENCE of records, so it means something only against
/// the sequence it was minted from: the operation that listed it and, for a listing the
/// request narrows, the parameters it was narrowed by. Both are folded into the
/// fingerprint a token carries beside its position, so a token minted elsewhere does not
/// read back as a position here and is refused — the caller is told the token resumes
/// nothing rather than served a page cut out of the middle of a listing it never asked to
/// resume, which would silently drop every record before that position.
///
/// The fingerprint is a hash, not the parameters themselves, so a token stays short
/// whatever a caller filters by and carries none of the request back out. It identifies a
/// listing for as long as the state the token indexes lives — the process — which is
/// exactly as long as any token of this emulator resumes anything.
struct ListingCursor {
    fingerprint: u64,
}

impl ListingCursor {
    /// The cursor for the listing `operation` reports under `scope`.
    fn of(operation: &str, scope: TokenScope<'_>) -> Self {
        let mut hasher = DefaultHasher::new();
        operation.hash(&mut hasher);
        match scope {
            TokenScope::WholeListing => "".hash(&mut hasher),
            TokenScope::SameParameters(key) => key.rendered.hash(&mut hasher),
        }
        Self {
            fingerprint: hasher.finish(),
        }
    }

    /// The token a page ending just before `index` reports.
    fn token(&self, index: usize) -> String {
        format!("{:016x}{TOKEN_PART_SEPARATOR}{index}", self.fingerprint)
    }

    /// The position `token` names in this listing, and `None` when it is not a token this
    /// listing handed out.
    ///
    /// A token resumes AS THE STRING a page returned: its fingerprint must be this
    /// listing's, and its position must be written the way [`Self::token`] writes one (a
    /// `usize`'s canonical decimal — no sign, no leading zeros). `usize::from_str` by
    /// itself admits more than that — a leading `+` and any number of leading zeros — so
    /// the parse is confirmed by rendering the position back and requiring the token to be
    /// exactly what the minting site would have written.
    fn position(&self, token: &str) -> Option<usize> {
        let (fingerprint, position) = token.split_once(TOKEN_PART_SEPARATOR)?;
        if fingerprint != format!("{:016x}", self.fingerprint) {
            return None;
        }
        let index: usize = position.parse().ok()?;
        (position == index.to_string()).then_some(index)
    }
}

/// Why `token` resumes no page of `operation`'s listing.
///
/// The one place this refusal's wording is built, so every list operation refuses a token
/// no page handed out in the same words, and the message is built from the request and the
/// operation's own [`TokenScope`] so it tells the caller what a token it may send came
/// from.
fn unhandled_next_token_message(operation: &str, scope: TokenScope<'_>, token: &str) -> String {
    match scope {
        TokenScope::WholeListing => {
            format!("nextToken {token} was not returned by a previous {operation} call")
        }
        TokenScope::SameParameters(_) => format!(
            "nextToken {token} was not returned by a previous {operation} call with these \
             parameters"
        ),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsControltowerAwscontroltowerapisState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsControltowerAwscontroltowerapisState>)
}

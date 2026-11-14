// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-shield-awsshield-20160616 service.
//!
//! Shield Advanced's control plane is a small set of resources: the account's
//! *subscription* (there is at most one), the *protections* it grants — each covering
//! exactly one protected Amazon Web Services resource — the *protection groups* that
//! bundle protected resources so they are detected on as a collective, the account's
//! emergency contacts, and the role the Shield Response Team is authorized through.
//! Every value a handler answers with is derived from the records kept here, so a write
//! made by one request is what the next request reads.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::date_time::Format as DateTimeFormat;
use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ApplicationLayerAutomaticResponseConfiguration, AttackDetail, AttackStatisticsDataItem,
    AttackSummary, AttackVectorDescription, AttackVolume, AttackVolumeStatistics, EmergencyContact,
    Limit, Protection, ProtectionGroup, ProtectionGroupArbitraryPatternLimits,
    ProtectionGroupLimits, ProtectionGroupPatternTypeLimits, ProtectionLimits, Subscription,
    SubscriptionLimits, SummarizedCounter, Tag, PROTECTED_RESOURCE_TYPE_VALUES,
};

/// The `SubscriptionState` an account with a live Shield Advanced subscription is in.
pub const SUBSCRIPTION_STATE_ACTIVE: &str = "ACTIVE";
/// The `SubscriptionState` an account without a live subscription is in.
pub const SUBSCRIPTION_STATE_INACTIVE: &str = "INACTIVE";

/// The `AutoRenew` value a freshly created subscription carries — the model's own
/// wording on `Subscription$AutoRenew`: "When you initally create a subscription,
/// `AutoRenew` is set to `ENABLED`."
pub const AUTO_RENEW_ENABLED: &str = "ENABLED";

/// The `ProactiveEngagementStatus` a freshly created subscription carries: proactive
/// engagement is off until the account asks for it (`EnableProactiveEngagement` /
/// `AssociateProactiveEngagementDetails`).
pub const PROACTIVE_ENGAGEMENT_STATUS_DISABLED: &str = "DISABLED";

/// The `ProactiveEngagementStatus` of an account that has asked the Shield Response Team
/// to reach out to it (`EnableProactiveEngagement`).
pub const PROACTIVE_ENGAGEMENT_STATUS_ENABLED: &str = "ENABLED";

/// The length of a Shield Advanced subscription, which is also the commitment the
/// account takes on (`Subscription$TimeCommitmentInSeconds`: "The length, in seconds,
/// of the Shield Advanced subscription for the account").
///
/// DOCUMENTED: AWS Shield Advanced is sold with a one-year subscription commitment, so
/// the commitment is one year of seconds and the subscription's `EndTime` is its
/// `StartTime` plus this span.
pub const SUBSCRIPTION_TIME_COMMITMENT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The `resourceType` this service reports for a protection in an error body.
pub const PROTECTION_RESOURCE_TYPE: &str = "protection";
/// The `resourceType` this service reports for the account's subscription in an error
/// body.
pub const SUBSCRIPTION_RESOURCE_TYPE: &str = "subscription";
/// The `resourceType` this service reports for a protection group in an error body.
pub const PROTECTION_GROUP_RESOURCE_TYPE: &str = "protection-group";

/// The `Type` a `LimitsExceededException` names when the account is out of protections.
pub const PROTECTION_LIMIT_TYPE: &str = "protection";
/// The `Type` a `LimitsExceededException` names when the account is out of protection
/// groups.
pub const PROTECTION_GROUP_LIMIT_TYPE: &str = "protection-group";

/// How this service states that the account holds no Shield Advanced subscription, so
/// the account-level resource every subscription operation addresses is absent. One
/// sentence for the whole service: a client separates the same `ResourceNotFoundException`
/// code's cases by this text.
pub const SUBSCRIPTION_NOT_FOUND_MESSAGE: &str = "The subscription does not exist.";

/// How this service states that the protection group a request addresses is absent —
/// deliberately distinct from [`SUBSCRIPTION_NOT_FOUND_MESSAGE`], which shares the
/// `ResourceNotFoundException` code.
pub const PROTECTION_GROUP_NOT_FOUND_MESSAGE: &str =
    "The referenced protection-group does not exist.";

/// How this service states that a protection group's
/// `Pattern`/`Members`/`ResourceType` combination is not one the service accepts. The
/// text names no field and carries no value from the request.
pub const PROTECTION_GROUP_FIELDS_MESSAGE: &str =
    "One or more protection group input fields are incorrect.";

/// How this service states that a `ResourceARN` names no Shield resource it holds.
pub const INVALID_RESOURCE_MESSAGE: &str = "The referenced resource is invalid.";

/// How this service states that a `NextToken` is not one it issued.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The `SummarizedCounter$Name` of the counter reporting the attack's bits per second —
/// the unit `AttackVolume$BitsPerSecond` names, which is the member of this service's own
/// volume shape for the same observation.
pub const ATTACK_COUNTER_BITS_PER_SECOND: &str = "BitsPerSecond";
/// The `SummarizedCounter$Name` of the counter reporting the attack's packets per second.
/// See [`ATTACK_COUNTER_BITS_PER_SECOND`].
pub const ATTACK_COUNTER_PACKETS_PER_SECOND: &str = "PacketsPerSecond";
/// The `SummarizedCounter$Name` of the counter reporting the attack's requests per
/// second. See [`ATTACK_COUNTER_BITS_PER_SECOND`].
pub const ATTACK_COUNTER_REQUESTS_PER_SECOND: &str = "RequestsPerSecond";

/// The `SummarizedCounter$Unit` a bits-per-second counter is measured in, from the `Unit`
/// values this service's model declares (`BITS`, `BYTES`, `PACKETS`, `REQUESTS`).
pub const ATTACK_COUNTER_UNIT_BITS: &str = "BITS";
/// The `SummarizedCounter$Unit` a packets-per-second counter is measured in. See
/// [`ATTACK_COUNTER_UNIT_BITS`].
pub const ATTACK_COUNTER_UNIT_PACKETS: &str = "PACKETS";
/// The `SummarizedCounter$Unit` a requests-per-second counter is measured in. See
/// [`ATTACK_COUNTER_UNIT_BITS`].
pub const ATTACK_COUNTER_UNIT_REQUESTS: &str = "REQUESTS";

/// The `ProtectionGroupPattern` that includes every protected resource of the account.
pub const PROTECTION_GROUP_PATTERN_ALL: &str = "ALL";
/// The `ProtectionGroupPattern` whose membership is the `Members` list the caller names.
pub const PROTECTION_GROUP_PATTERN_ARBITRARY: &str = "ARBITRARY";
/// The `ProtectionGroupPattern` whose membership is every protected resource of the
/// group's `ResourceType`.
pub const PROTECTION_GROUP_PATTERN_BY_RESOURCE_TYPE: &str = "BY_RESOURCE_TYPE";

/// The most protections one account may hold at a time; a create beyond it is refused
/// with the modelled `LimitsExceededException` (HTTP 400).
///
/// ENFORCED ACCOUNT-WIDE, across every protected resource type. It is also what
/// `DescribeSubscription` reports as each type's `Max`, because the modelled
/// `Subscription$Limits` / `ProtectionLimits$ProtectedResourceTypeLimits` express a
/// ceiling per type and no single type can hold more protections than the account does:
/// the per-type figure is this account-wide bound, not a separate per-type allowance.
///
/// CHOSEN: AWS quotas the protections an account may hold but the figure is not in the
/// model. The bound is also what keeps this store finite — a record is released only by
/// `DeleteProtection`.
pub const MAX_PROTECTIONS_PER_ACCOUNT: i64 = 1000;

/// The most protection groups one account may hold at a time, as
/// `SubscriptionLimits$ProtectionGroupLimits` reports it. CHOSEN, like
/// [`MAX_PROTECTIONS_PER_ACCOUNT`].
pub const MAX_PROTECTION_GROUPS: i64 = 1000;

/// The most members an `ARBITRARY`-pattern protection group may name, as
/// `ProtectionGroupPatternTypeLimits` reports it. CHOSEN, like
/// [`MAX_PROTECTIONS_PER_ACCOUNT`].
pub const MAX_ARBITRARY_PATTERN_MEMBERS: i64 = 100;

/// The most Amazon S3 log buckets the Shield Response Team may be authorized to read
/// for one account, which is the modelled length bound of `LogBucketList` (0..10) —
/// the list `DescribeDRTAccess` answers with.
pub const MAX_DRT_LOG_BUCKETS: usize = 10;

/// The page size a listing uses when the caller names no `MaxResults` — the model's own
/// wording on `ListProtectionsRequest$MaxResults`,
/// `ListProtectionGroupsRequest$MaxResults` and
/// `ListResourcesInProtectionGroupRequest$MaxResults` alike: "The default setting is
/// 20."
pub const DEFAULT_LIST_MAX_RESULTS: i32 = 20;

/// The page size `ListProtections` uses when the caller names no `MaxResults`; see
/// [`DEFAULT_LIST_MAX_RESULTS`], which every listing of this service shares.
pub const DEFAULT_LIST_PROTECTIONS_MAX_RESULTS: i32 = DEFAULT_LIST_MAX_RESULTS;

/// How many ids a mint tries before giving up, so no request can spin here. A candidate
/// is a version-4 UUID, so the first one is free in every realistic run.
const MINT_ATTEMPTS: usize = 8;

/// A fresh `ProtectionId`: a version-4 UUID in canonical hyphenated form (`8-4-4-4-12`
/// lowercase hex with the version and variant nibbles set), which is exactly the 36
/// characters `ProtectionId` (len 36..36, pattern `^[a-zA-Z0-9\-]*$`) allows.
///
/// The random bits come from the standard library's [`RandomState`], whose SipHash keys
/// std seeds from the host's randomness once per thread and advances on every `new()`,
/// so two draws in one process do not agree and two processes do not share a sequence.
/// A `uuid` crate dependency would be the obvious source, but it is the one thing this
/// fix cannot afford: a new dependency edits the workspace `Cargo.lock`, the single file
/// every concurrent change to this repository rewrites, and that overlap is what refused
/// this fix's merge. The value is an opaque name — never a credential or a capability —
/// and [`ComAmazonawsShieldAwsshield20160616State::mint_protection_id`] re-draws if the
/// store somehow already holds it.
fn new_uuid() -> String {
    let mut bytes = [0u8; 16];
    for half in 0..2 {
        let draw = RandomState::new().build_hasher().finish().to_be_bytes();
        bytes[half * 8..(half + 1) * 8].copy_from_slice(&draw);
    }
    // Version 4 ("random") and the RFC 4122 variant, so the value is a well-formed v4
    // rather than 16 bytes that merely look like one.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// This service's ARN for a protection. Shield is a global service, so the Region
/// segment of every ARN it issues is empty.
pub fn protection_arn(protection_id: &str) -> String {
    ::engine::emulator_arn("shield", "", &format!("protection/{protection_id}"))
}

/// This service's ARN for the account's subscription. See [`protection_arn`] for the
/// empty Region segment.
pub fn subscription_arn(subscription_id: &str) -> String {
    ::engine::emulator_arn("shield", "", &format!("subscription/{subscription_id}"))
}

/// This service's ARN for a protection group, whose identifier is the name the caller
/// chose. See [`protection_arn`] for the empty Region segment.
pub fn protection_group_arn(protection_group_id: &str) -> String {
    ::engine::emulator_arn(
        "shield",
        "",
        &format!("protection-group/{protection_group_id}"),
    )
}

/// Encode the place a page stopped at as the opaque `NextToken` the next call passes
/// back. `kind` names the listing that issued it, so a token is only ever understood by
/// the listing it came from, and `position` is that listing's own stable coordinate.
pub fn encode_page_token(kind: &str, position: u64) -> String {
    ::hex::encode(format!("{kind}/v1:{position}"))
}

/// The position a token resumes after, or `None` when the token is not one
/// [`encode_page_token`] issued for this `kind` \u2014 the only thing a listing can say
/// about a token it did not mint.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let decoded = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded.strip_prefix(&format!("{kind}/v1:"))?.parse().ok()
}

/// The most tags one Shield resource may carry at a time.
///
/// The modelled `TagList` is `@length 0..200`, and it is the shape of both a tagging
/// request's `Tags` and the `Tags` a read answers with: a resource that held more could
/// not be reported inside the bound the model declares, so 200 is the resource's ceiling
/// and not merely one call's. A call that would take a resource past it is refused.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// Apply a tagging operation's tags to a resource's stored tags: a key the resource
/// already carries takes the new value, a key it does not carry is added, and a key the
/// request does not name is left alone.
pub fn merge_tags(stored: &mut Vec<Tag>, incoming: &[Tag]) {
    for tag in incoming {
        match stored.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => stored.push(tag.clone()),
        }
    }
}

/// The `ProtectedResourceType` of the resource an ARN names, or `None` when the ARN is
/// not in one of the layouts `CreateProtectionRequest$ResourceArn` documents.
///
/// The mapping is exactly that member's documented list of formats:
///
/// * `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<id>`
///   — `APPLICATION_LOAD_BALANCER`
/// * `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/<name>`
///   — `CLASSIC_LOAD_BALANCER`
/// * `arn:aws:cloudfront::<account>:distribution/<id>` — `CLOUDFRONT_DISTRIBUTION`
/// * `arn:aws:globalaccelerator::<account>:accelerator/<id>` — `GLOBAL_ACCELERATOR`
/// * `arn:aws:route53:::hostedzone/<id>` — `ROUTE_53_HOSTED_ZONE`
/// * `arn:aws:ec2:<region>:<account>:eip-allocation/<id>` — `ELASTIC_IP_ALLOCATION`
///
/// Every value this answers is one of [`PROTECTED_RESOURCE_TYPE_VALUES`]. An ARN in no
/// documented layout is stored with no resource type rather than refused: the emulator
/// does not hold the protected resource, so it cannot tell a resource type it has never
/// heard of from one AWS added after this model was published.
pub fn protected_resource_type(resource_arn: &str) -> Option<&'static str> {
    // `arn:aws:<service>:<region>:<account>:<resource>` — the resource portion may
    // itself contain `:`, so it is the sixth field and everything after it.
    let mut fields = resource_arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };

    match service {
        "elasticloadbalancing" => {
            let load_balancer = resource.strip_prefix("loadbalancer/")?;
            if let Some(application) = load_balancer.strip_prefix("app/") {
                // `app/<name>/<id>`: an Application Load Balancer.
                (!application.is_empty()).then_some("APPLICATION_LOAD_BALANCER")
            } else if !load_balancer.is_empty() && !load_balancer.contains('/') {
                // `<name>`: a Classic Load Balancer. A `net/` or `gwy/` load balancer
                // is neither, and is not in the documented list.
                Some("CLASSIC_LOAD_BALANCER")
            } else {
                None
            }
        }
        "cloudfront" => named_resource(resource, "distribution/").map(|_| "CLOUDFRONT_DISTRIBUTION"),
        "globalaccelerator" => named_resource(resource, "accelerator/").map(|_| "GLOBAL_ACCELERATOR"),
        "route53" => named_resource(resource, "hostedzone/").map(|_| "ROUTE_53_HOSTED_ZONE"),
        "ec2" => named_resource(resource, "eip-allocation/").map(|_| "ELASTIC_IP_ALLOCATION"),
        _ => None,
    }
}

/// The identifier after `prefix` in an ARN's resource portion, when the portion is that
/// prefix followed by a non-empty identifier.
fn named_resource<'a>(resource: &'a str, prefix: &str) -> Option<&'a str> {
    let id = resource.strip_prefix(prefix)?;
    (!id.is_empty()).then_some(id)
}

/// How many protections of each protected resource type the subscription allows, as
/// `Subscription$Limits` and `ProtectionLimits$ProtectedResourceTypeLimits` report it.
///
/// The types are exactly the modelled `ProtectedResourceType` values, so the list
/// follows the model rather than a hand-kept copy of it.
pub fn protected_resource_type_limits() -> Vec<Limit> {
    PROTECTED_RESOURCE_TYPE_VALUES
        .iter()
        .map(|resource_type| Limit {
            r#type: Some((*resource_type).to_string()),
            max: MAX_PROTECTIONS_PER_ACCOUNT,
        })
        .collect()
}

/// The `SubscriptionLimits` every subscription of this emulator grants.
pub fn subscription_limits() -> SubscriptionLimits {
    SubscriptionLimits {
        protection_limits: ProtectionLimits {
            protected_resource_type_limits: protected_resource_type_limits(),
        },
        protection_group_limits: ProtectionGroupLimits {
            max_protection_groups: MAX_PROTECTION_GROUPS,
            pattern_type_limits: ProtectionGroupPatternTypeLimits {
                arbitrary_pattern_limits: ProtectionGroupArbitraryPatternLimits {
                    max_members: MAX_ARBITRARY_PATTERN_MEMBERS,
                },
            },
        },
    }
}

/// The account's Shield Advanced subscription, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct SubscriptionRecord {
    pub subscription_arn: String,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub time_commitment_in_seconds: i64,
    pub auto_renew: String,
    pub proactive_engagement_status: String,
}

impl SubscriptionRecord {
    /// The subscription an account takes on by calling `CreateSubscription` at `now`.
    pub fn activated_at(now: SystemTime) -> Self {
        Self {
            subscription_arn: subscription_arn(&new_uuid()),
            start_time: now,
            end_time: now + SUBSCRIPTION_TIME_COMMITMENT,
            time_commitment_in_seconds: SUBSCRIPTION_TIME_COMMITMENT.as_secs() as i64,
            auto_renew: AUTO_RENEW_ENABLED.to_string(),
            proactive_engagement_status: PROACTIVE_ENGAGEMENT_STATUS_DISABLED.to_string(),
        }
    }

    /// Whether the subscription still covers the account at `now`: its commitment runs
    /// from `StartTime` until `EndTime`.
    pub fn is_active_at(&self, now: SystemTime) -> bool {
        now < self.end_time
    }

    /// This record as the modelled `Subscription` a read answers with.
    pub fn to_modeled(&self) -> Subscription {
        Subscription {
            start_time: Some(DateTime::from(self.start_time)),
            end_time: Some(DateTime::from(self.end_time)),
            time_commitment_in_seconds: self.time_commitment_in_seconds,
            auto_renew: Some(self.auto_renew.clone()),
            limits: Some(protected_resource_type_limits()),
            proactive_engagement_status: Some(self.proactive_engagement_status.clone()),
            subscription_limits: subscription_limits(),
            subscription_arn: Some(self.subscription_arn.clone()),
        }
    }
}

/// One protection — a single protected resource — as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ProtectionEntry {
    pub id: String,
    pub name: String,
    pub resource_arn: String,
    pub protection_arn: String,
    /// The `ProtectedResourceType` [`protected_resource_type`] derives from
    /// `resource_arn`, or `None` for an ARN in no documented layout.
    pub resource_type: Option<String>,
    pub health_check_ids: Vec<String>,
    pub application_layer_automatic_response:
        Option<ApplicationLayerAutomaticResponseConfiguration>,
    /// The tags the resource carries: what `CreateProtection` was given, as the
    /// tagging operations (`TagResource`, `ListTagsForResource`) then read and write.
    pub tags: Vec<Tag>,
    /// When the protection was created. Stored for the same reason as `tags`: no member
    /// of the modelled `Protection` carries it, so no implemented read reports it.
    pub created_at: SystemTime,
    /// Position in the account's creation order: what `ListProtections` pages over, so
    /// its order is stable and a token keeps meaning the same place after a delete.
    pub sequence: u64,
}

impl ProtectionEntry {
    /// This record as the modelled `Protection` a read answers with.
    pub fn to_modeled(&self) -> Protection {
        Protection {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            health_check_ids: Some(self.health_check_ids.clone()),
            protection_arn: Some(self.protection_arn.clone()),
            application_layer_automatic_response_configuration: self
                .application_layer_automatic_response
                .clone(),
        }
    }
}

/// One protection group — a set of protected resources detected on as a collective —
/// as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ProtectionGroupEntry {
    /// The `ProtectionGroupId` the caller chose, which is also this group's name.
    pub id: String,
    pub aggregation: String,
    pub pattern: String,
    /// Set only for a `BY_RESOURCE_TYPE` group, which is the one pattern whose
    /// membership is a resource type.
    pub resource_type: Option<String>,
    /// The members an `ARBITRARY` group was given; empty for every other pattern, whose
    /// membership is derived from the account's protections instead.
    pub members: Vec<String>,
    pub protection_group_arn: String,
    /// The tags `CreateProtectionGroup` was given, as the tagging operations then read
    /// and write.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// Position in the account's creation order: what `ListProtectionGroups` pages
    /// over, so its order is stable and a token keeps meaning the same place after a
    /// delete.
    pub sequence: u64,
}

impl ProtectionGroupEntry {
    /// This record as the modelled `ProtectionGroup` a read answers with.
    pub fn to_modeled(&self) -> ProtectionGroup {
        ProtectionGroup {
            protection_group_id: self.id.clone(),
            aggregation: self.aggregation.clone(),
            pattern: self.pattern.clone(),
            resource_type: self.resource_type.clone(),
            members: self.members.clone(),
            protection_group_arn: Some(self.protection_group_arn.clone()),
        }
    }
}

/// One DDoS attack Shield detected against a resource of the account, as this emulator
/// persists it.
///
/// Shield detects attacks rather than being asked to create them, so no modelled
/// operation writes a record here; the attack log is the single place every attack
/// reader of this service (`DescribeAttack`, `DescribeAttackStatistics` and
/// `ListAttacks`) derives its answer from, so an account with no detected attacks and
/// one with several are the same code path.
#[derive(Debug, Clone)]
pub struct AttackRecord {
    /// The `AttackId` that identifies this attack.
    pub attack_id: String,
    /// The ARN of the account resource the attack was directed at.
    pub resource_arn: String,
    /// When Shield started detecting the attack: the instant that places the attack in
    /// a reporting window.
    pub start_time: SystemTime,
    /// When the attack ended, or `None` while it is still ongoing.
    pub end_time: Option<SystemTime>,
    /// The peak bits per second observed, set for a network-level attack —
    /// `AttackVolume$BitsPerSecond`: "A statistics object that uses bits per second as
    /// the unit. This is included for network level attacks."
    pub max_bits_per_second: Option<f64>,
    /// The peak packets per second observed, set for a network-level attack, like
    /// [`AttackRecord::max_bits_per_second`].
    pub max_packets_per_second: Option<f64>,
    /// The peak requests per second observed, set for an application-level attack —
    /// `AttackVolume$RequestsPerSecond` is "included for application level attacks".
    pub max_requests_per_second: Option<f64>,
    /// The attack types Shield observed in this attack, each an `AttackVectorDescription`
    /// `VectorType` (for example `UDP_TRAFFIC`); empty for an attack whose traffic was
    /// attributed to no vector.
    pub attack_vectors: Vec<String>,
}

impl AttackRecord {
    /// This record as the modelled `AttackSummary` a listing answers with
    /// (`AttackSummary`: "Summarizes all DDoS attacks for a specified time period").
    ///
    /// `EndTime` is "the end time of the attack", so it is reported only for an attack
    /// that has ended; an attack still under way carries none.
    pub fn to_summary(&self) -> AttackSummary {
        AttackSummary {
            attack_id: Some(self.attack_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            start_time: Some(DateTime::from(self.start_time)),
            end_time: self.end_time.map(DateTime::from),
            attack_vectors: Some(
                self.attack_vectors
                    .iter()
                    .map(|vector_type| AttackVectorDescription {
                        vector_type: vector_type.clone(),
                    })
                    .collect(),
            ),
        }
    }

    /// This record as the modelled `AttackDetail` a describe answers with
    /// (`AttackDetail`: "The details of a DDoS attack").
    ///
    /// Every member carries what the attack log holds and nothing else: the attack's own
    /// identifier and attacked resource, the instants it spans — an attack still under way
    /// has no `EndTime` ("The time the attack ended") — and its counters
    /// (`AttackDetail$AttackCounters`: "List of counters that describe the attack for the
    /// specified time period"), one per unit the attack was observed in.
    ///
    /// `SubResources` ("If applicable, additional detail about the resource being attacked,
    /// for example, IP address or URL"), `AttackProperties` and `Mitigations` name detail
    /// no record of the log carries, so none is reported rather than invented.
    pub fn to_detail(&self) -> AttackDetail {
        AttackDetail {
            attack_id: Some(self.attack_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            start_time: Some(DateTime::from(self.start_time)),
            end_time: self.end_time.map(DateTime::from),
            attack_counters: Some(self.attack_counters()),
            sub_resources: None,
            attack_properties: None,
            mitigations: None,
        }
    }

    /// One `SummarizedCounter` per unit this attack was observed in, in the order this
    /// service's `AttackVolume` lists the units.
    ///
    /// The log holds the peak of each unit, so a counter reports that peak as its `Max`
    /// (`AttackVolumeStatistics$Max`: "The maximum attack volume observed for the given
    /// unit") and leaves the statistics the log does not hold — the average, the sum and
    /// the number of observations — at the defaults the model gives them. A unit the attack
    /// was never observed in has no counter at all.
    pub fn attack_counters(&self) -> Vec<SummarizedCounter> {
        [
            (
                ATTACK_COUNTER_BITS_PER_SECOND,
                ATTACK_COUNTER_UNIT_BITS,
                self.max_bits_per_second,
            ),
            (
                ATTACK_COUNTER_PACKETS_PER_SECOND,
                ATTACK_COUNTER_UNIT_PACKETS,
                self.max_packets_per_second,
            ),
            (
                ATTACK_COUNTER_REQUESTS_PER_SECOND,
                ATTACK_COUNTER_UNIT_REQUESTS,
                self.max_requests_per_second,
            ),
        ]
        .into_iter()
        .filter_map(|(name, unit, peak)| {
            peak.map(|max| SummarizedCounter {
                name: Some(name.to_string()),
                unit: Some(unit.to_string()),
                max,
                ..SummarizedCounter::default()
            })
        })
        .collect()
    }
}

/// How many calendar years back the attack statistics window reaches, as
/// `DescribeAttackStatistics` documents its own range: "the time range of midnight UTC,
/// one year ago, to midnight UTC, today".
const ATTACK_STATISTICS_WINDOW_YEARS: i32 = 1;

/// The last day of February in a year that is not a leap year — where a 29 February
/// lands when it is carried back to a year that has no 29 February.
const LAST_SHORT_FEBRUARY_DAY: &str = "02-28";

/// The UTC calendar year, and the `MM-DD` within it, that an instant falls in.
///
/// The civil-date conversion is the modelled timestamp type's own
/// (`::aws_smithy_types::DateTime`, which every response of this service already renders
/// through), so this service keeps no calendar of its own.
fn utc_year_and_month_day(instant: SystemTime) -> Option<(i32, String)> {
    let rendered = DateTime::from(instant).fmt(DateTimeFormat::DateTime).ok()?;
    let date = rendered.split('T').next()?;
    let (year, month_day) = date.split_once('-')?;
    Some((year.parse().ok()?, month_day.to_string()))
}

/// Midnight UTC of a calendar date, or `None` when the year has no such date (only a
/// 29 February can fail here). The date is resolved by the modelled timestamp type, so
/// an impossible date is rejected rather than wrapped into the next month.
fn midnight_utc(year: i32, month_day: &str) -> Option<SystemTime> {
    let rendered = format!("{year:04}-{month_day}T00:00:00Z");
    let parsed = DateTime::from_str(&rendered, DateTimeFormat::DateTime).ok()?;
    SystemTime::try_from(parsed).ok()
}

/// The window `DescribeAttackStatistics` reports over at `now`, as `[from, to)` —
/// midnight UTC one year before the current UTC day, up to (and excluding) midnight UTC
/// of the current UTC day.
///
/// The span is a calendar year rather than a fixed count of days, so the lower bound is
/// midnight of the same month and day one year earlier; a 29 February has no counterpart
/// in the preceding year and so falls back to that year's last day of February.
pub fn attack_statistics_window(now: SystemTime) -> (SystemTime, SystemTime) {
    let Some((year, month_day)) = utc_year_and_month_day(now) else {
        return (now, now);
    };
    let to_exclusive = midnight_utc(year, &month_day).unwrap_or(now);
    let earlier_year = year - ATTACK_STATISTICS_WINDOW_YEARS;
    let from_inclusive = midnight_utc(earlier_year, &month_day)
        .or_else(|| midnight_utc(earlier_year, LAST_SHORT_FEBRUARY_DAY))
        .unwrap_or(to_exclusive);
    (from_inclusive, to_exclusive)
}

/// Which of this service's taggable resources an ARN names.
#[derive(Debug, Clone)]
enum TaggableResource {
    Protection(String),
    ProtectionGroup(String),
}

/// Per-service state for com-amazonaws-shield-awsshield-20160616. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsShieldAwsshield20160616State {
    /// The account's Shield Advanced subscription: `None` until `CreateSubscription`.
    pub subscription: Option<SubscriptionRecord>,
    /// Every live protection, keyed by its `ProtectionId` — the primary index.
    pub protections: HashMap<String, ProtectionEntry>,
    /// `ResourceArn` -> `ProtectionId`: the uniqueness index that refuses a second
    /// protection for one resource and resolves a describe-by-ARN.
    pub protections_by_resource_arn: HashMap<String, String>,
    /// Every live protection group, keyed by its `ProtectionGroupId` — which the caller
    /// chooses, so the key is also the group's name.
    pub protection_groups: HashMap<String, ProtectionGroupEntry>,
    /// The account's emergency contacts, as `UpdateEmergencyContactSettings` last set
    /// them: a subscription starts with none.
    pub emergency_contacts: Vec<EmergencyContact>,
    /// The role the Shield Response Team is authorized through (`AssociateDRTRole`):
    /// `None` while the account has authorized none.
    pub drt_role_arn: Option<String>,
    /// The Amazon S3 log buckets the Shield Response Team may read, in the order the
    /// account authorized them (`AssociateDRTLogBucket`, withdrawn again by
    /// `DisassociateDRTLogBucket`): empty while the account has authorized none.
    pub drt_log_buckets: Vec<String>,
    /// Every DDoS attack Shield has detected against a resource of the account: the log
    /// `DescribeAttackStatistics` aggregates. Empty for an account no attack has hit.
    pub attacks: Vec<AttackRecord>,
    /// The creation order counter behind [`ProtectionEntry::sequence`] and
    /// [`ProtectionGroupEntry::sequence`].
    next_sequence: u64,
}

impl ComAmazonawsShieldAwsshield20160616State {
    /// The attack statistics for the window `[from_inclusive, to_exclusive)`, as
    /// `DescribeAttackStatistics` answers with them.
    ///
    /// One data record summarises the whole window (`AttackStatisticsDataItem`: "A
    /// single attack statistics data record ... returned by `DescribeAttackStatistics`
    /// along with a time range indicating the time period that the attack statistics
    /// apply to"): `AttackCount` is how many stored attacks started inside the window,
    /// and the volume statistics are the maxima the counted attacks reported, per unit.
    ///
    /// The `AttackVolume` block is part of the record the window always summarises, so it
    /// is reported for every data item; it is its three unit members that carry whether
    /// anything was observed, each set only when a counted attack reported that unit. A
    /// window in which no counted attack reported any unit is therefore an `AttackVolume`
    /// with no member set — `AttackVolume`: "If the accompanying `AttackCount` in the
    /// statistics object is zero, this setting might be empty."
    pub fn attack_statistics_in(
        &self,
        from_inclusive: SystemTime,
        to_exclusive: SystemTime,
    ) -> Vec<AttackStatisticsDataItem> {
        let counted: Vec<&AttackRecord> = self
            .attacks
            .iter()
            .filter(|attack| {
                attack.start_time >= from_inclusive && attack.start_time < to_exclusive
            })
            .collect();

        let peak = |unit: fn(&AttackRecord) -> Option<f64>| -> Option<AttackVolumeStatistics> {
            counted
                .iter()
                .filter_map(|attack| unit(attack))
                .max_by(f64::total_cmp)
                .map(|max| AttackVolumeStatistics { max })
        };
        let volume = AttackVolume {
            bits_per_second: peak(|attack| attack.max_bits_per_second),
            packets_per_second: peak(|attack| attack.max_packets_per_second),
            requests_per_second: peak(|attack| attack.max_requests_per_second),
        };

        vec![AttackStatisticsDataItem {
            attack_count: counted.len() as i64,
            attack_volume: Some(volume),
        }]
    }

    /// The attack of the account's log with this `AttackId`, or `None` when no attack
    /// Shield detected against the account carries it.
    pub fn attack_by_id(&self, attack_id: &str) -> Option<&AttackRecord> {
        self.attacks
            .iter()
            .find(|attack| attack.attack_id == attack_id)
    }

    /// The `SubscriptionState` the account is in at `now`.
    pub fn subscription_state_at(&self, now: SystemTime) -> &'static str {
        match self.subscription.as_ref() {
            Some(subscription) if subscription.is_active_at(now) => SUBSCRIPTION_STATE_ACTIVE,
            _ => SUBSCRIPTION_STATE_INACTIVE,
        }
    }

    /// A `ProtectionId` no stored protection holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates were all taken.
    pub fn mint_protection_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| new_uuid())
            .find(|candidate| !self.protections.contains_key(candidate))
    }

    /// Store a protection in creation order and index it by the ARN it protects.
    pub fn insert_protection(&mut self, mut entry: ProtectionEntry) {
        entry.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.protections_by_resource_arn
            .insert(entry.resource_arn.clone(), entry.id.clone());
        self.protections.insert(entry.id.clone(), entry);
    }

    /// The protection with this `ProtectionId`.
    pub fn protection_by_id(&self, protection_id: &str) -> Option<&ProtectionEntry> {
        self.protections.get(protection_id)
    }

    /// The protection covering this `ResourceArn`.
    pub fn protection_by_resource_arn(&self, resource_arn: &str) -> Option<&ProtectionEntry> {
        let protection_id = self.protections_by_resource_arn.get(resource_arn)?;
        self.protections.get(protection_id)
    }

    /// Forget a protection (and its resource-ARN index entry), as `DeleteProtection`
    /// does.
    pub fn remove_protection(&mut self, protection_id: &str) -> Option<ProtectionEntry> {
        let entry = self.protections.remove(protection_id)?;
        self.protections_by_resource_arn.remove(&entry.resource_arn);
        Some(entry)
    }

    /// Every stored protection in creation order — the stable order `ListProtections`
    /// pages over.
    pub fn protections_in_creation_order(&self) -> Vec<&ProtectionEntry> {
        let mut ordered: Vec<&ProtectionEntry> = self.protections.values().collect();
        ordered.sort_by_key(|entry| entry.sequence);
        ordered
    }

    /// Store a protection group in creation order.
    pub fn insert_protection_group(&mut self, mut entry: ProtectionGroupEntry) {
        entry.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.protection_groups.insert(entry.id.clone(), entry);
    }

    /// The protection group with this `ProtectionGroupId`.
    pub fn protection_group_by_id(
        &self,
        protection_group_id: &str,
    ) -> Option<&ProtectionGroupEntry> {
        self.protection_groups.get(protection_group_id)
    }

    /// Every stored protection group in creation order — the stable order
    /// `ListProtectionGroups` pages over.
    pub fn protection_groups_in_creation_order(&self) -> Vec<&ProtectionGroupEntry> {
        let mut ordered: Vec<&ProtectionGroupEntry> = self.protection_groups.values().collect();
        ordered.sort_by_key(|entry| entry.sequence);
        ordered
    }

    /// The protected resources a group currently includes, in the account's protection
    /// creation order.
    ///
    /// Membership follows the group's `Pattern`, as `ProtectionGroup$Pattern` documents
    /// it: an `ARBITRARY` group includes exactly the members it was given, a
    /// `BY_RESOURCE_TYPE` group every protected resource of its type ("Newly protected
    /// resources of this type are automatically added to the group"), and an `ALL`
    /// group every protected resource of the account.
    pub fn resources_in_protection_group(&self, group: &ProtectionGroupEntry) -> Vec<String> {
        if group.pattern == PROTECTION_GROUP_PATTERN_ARBITRARY {
            return group.members.clone();
        }
        self.protections_in_creation_order()
            .into_iter()
            .filter(|entry| {
                group.pattern != PROTECTION_GROUP_PATTERN_BY_RESOURCE_TYPE
                    || entry.resource_type.as_deref() == group.resource_type.as_deref()
            })
            .map(|entry| entry.resource_arn.clone())
            .collect()
    }

    /// Whether a protection group's `Pattern`, `ResourceType` and `Members` are a
    /// combination this service accepts.
    ///
    /// The three members constrain each other, in their own documented words.
    /// `Members`: "You must set this when you set `Pattern` to `ARBITRARY` and you must
    /// not set it for any other `Pattern` setting"; `ResourceType`: "You must set this
    /// when you set `Pattern` to `BY_RESOURCE_TYPE` and you must not set it for any
    /// other `Pattern` setting". A `Members` entry is an ARN of a resource to include,
    /// and a protection group groups *protected* resources, so each named member must be
    /// a resource the account protects. An empty list is not a set one: a group whose
    /// membership is spelled out explicitly has at least one member.
    pub fn protection_group_fields_accepted(
        &self,
        pattern: &str,
        resource_type: Option<&str>,
        members: &[String],
    ) -> bool {
        let wants_members = pattern == PROTECTION_GROUP_PATTERN_ARBITRARY;
        let wants_resource_type = pattern == PROTECTION_GROUP_PATTERN_BY_RESOURCE_TYPE;
        if members.is_empty() == wants_members || resource_type.is_some() != wants_resource_type {
            return false;
        }
        members
            .iter()
            .all(|member| self.protection_by_resource_arn(member).is_some())
    }

    /// Which taggable resource of this service an ARN names, or `None` when the account
    /// holds no such resource.
    fn taggable_resource(&self, resource_arn: &str) -> Option<TaggableResource> {
        if let Some(entry) = self
            .protections
            .values()
            .find(|entry| entry.protection_arn == resource_arn)
        {
            return Some(TaggableResource::Protection(entry.id.clone()));
        }
        let group = self
            .protection_groups
            .values()
            .find(|entry| entry.protection_group_arn == resource_arn)?;
        Some(TaggableResource::ProtectionGroup(group.id.clone()))
    }

    /// The tags the resource this ARN names carries, or `None` when the account holds no
    /// such resource.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&Vec<Tag>> {
        match self.taggable_resource(resource_arn)? {
            TaggableResource::Protection(id) => self.protections.get(&id).map(|entry| &entry.tags),
            TaggableResource::ProtectionGroup(id) => {
                self.protection_groups.get(&id).map(|entry| &entry.tags)
            }
        }
    }

    /// The tags the resource this ARN names carries, for a tagging operation to change;
    /// `None` when the account holds no such resource.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        match self.taggable_resource(resource_arn)? {
            TaggableResource::Protection(id) => {
                self.protections.get_mut(&id).map(|entry| &mut entry.tags)
            }
            TaggableResource::ProtectionGroup(id) => self
                .protection_groups
                .get_mut(&id)
                .map(|entry| &mut entry.tags),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsShieldAwsshield20160616State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsShieldAwsshield20160616State>)
}

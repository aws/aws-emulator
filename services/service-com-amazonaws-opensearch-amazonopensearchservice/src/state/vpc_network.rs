// SPDX-License-Identifier: CC0-1.0

//! The network a VPC-access domain or a VPC endpoint is placed in: facts this service
//! RESOLVES rather than holds.
//!
//! `VPCOptions` names the subnets and security groups a placement is created in, and
//! `VPCDerivedInfo` reports back the two members the model says are DERIVED from those
//! subnets: `VPCId` — "the ID for your VPC. Amazon VPC generates this value when you
//! create a VPC" — and `AvailabilityZones`, "the list of Availability Zones associated
//! with the VPC subnets". Neither is a resource of this service: a subnet is Amazon
//! EC2's, so which VPC and which zone one is in is EC2's to say, and what a request
//! names is resolved against EC2 — reached the way any emulated service reaches another,
//! through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate.
//!
//! Everything built here is grounded in what EC2 ANSWERED: a subnet it reports nothing
//! for is left unresolved, and an emulator with no EC2 mounted knows nothing about the
//! account's network, so nothing is what it refuses on — a refusal is never assumed from
//! silence. That is the reading a service that only GATES on the network takes, as
//! `service_common::network` records; this service keeps what EC2 answered as well,
//! because `VPCDerivedInfo` reports it, and keeps it only for the subnets EC2 resolved.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// The EC2 filter that selects subnets BY ID (`subnet-id`, listed by `DescribeSubnets`'s
/// own `Filters` documentation).
///
/// A filter rather than `SubnetIds`: an identifier the account has no subnet for is then
/// an absent row instead of a failure of the whole call, so every identifier the request
/// named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most identifiers asked about in ONE hop to EC2.
///
/// A hop is evaluated against every record the account stores of that kind, so this caps
/// the identifiers one question carries; the identifiers a request names beyond it are
/// asked about in further hops ([`MAX_RESOLVED_HOPS`] of them).
const IDS_PER_HOP: usize = 64;

/// The most hops one resolution makes, and so — with [`IDS_PER_HOP`] — the most
/// identifiers it resolves.
///
/// CHOSEN as a process-safety ceiling, not a published quota: the model declares no
/// `@length` on `VPCOptions$SubnetIds` or `VPCOptions$SecurityGroupIds`, so without one
/// the work a single request causes would be sized by that request. A VPC endpoint is
/// placed with one subnet per Availability Zone, so the product is far above what any real
/// placement names; identifiers past it are left unresolved, which — like an EC2 that
/// cannot be asked — grounds nothing.
const MAX_RESOLVED_HOPS: usize = 8;

/// The identifiers ONE resolution asks EC2 about: the ones `ids` names, each once and in
/// the order they were named, capped at what [`IDS_PER_HOP`] hops of
/// [`MAX_RESOLVED_HOPS`] resolve.
///
/// Deduplicated so a request that repeats an identifier costs one question, and capped so a
/// caller cannot size this emulator's work by the length of its list.
fn asked_ids(ids: &[String]) -> Vec<String> {
    let ceiling = IDS_PER_HOP * MAX_RESOLVED_HOPS;
    let mut asked: Vec<String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in ids {
        if asked.len() == ceiling {
            break;
        }
        if seen.insert(id.as_str()) {
            asked.push(id.clone());
        }
    }
    asked
}

/// One subnet of the account, reduced to what this service reads off it: which VPC it
/// belongs to and which Availability Zone it sits in, as EC2 reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetPlacement {
    /// The identifier the request named, which is the one EC2 answered for.
    pub subnet_id: String,
    /// The VPC EC2 reports the subnet in, absent where its record carries none.
    pub vpc_id: Option<String>,
    /// The Availability Zone EC2 reports the subnet in, absent where its record carries
    /// none.
    pub availability_zone: Option<String>,
}

/// What the service that OWNS subnets answered about the subnets a request named: the
/// subnets it reported, and which identifiers it answered about at all.
///
/// The two travel together for the reason [`ResolvedSecurityGroups`] carries both: a subnet
/// ABSENT from the answer means one of two different things — EC2 holds no such subnet, or
/// EC2 was never asked (no EC2 in this process, a hop that failed, an identifier past the
/// resolution's own ceiling) — and only the first is evidence about the account's network
/// ([`unknown_subnet_refusal`]).
#[derive(Debug, Clone, Default)]
pub struct ResolvedSubnets {
    /// The subnets EC2 reported, in the order the request named them.
    pub placements: Vec<SubnetPlacement>,
    /// The identifiers EC2 ANSWERED about — the ones a question was put to it about and
    /// answered. An identifier outside this list is one nothing is known about.
    answered: Vec<String>,
}

impl ResolvedSubnets {
    /// Whether EC2 answered about `subnet_id`, so what it reported for it — a subnet, or
    /// nothing — is evidence about the account's network.
    #[must_use]
    pub fn answered_about(&self, subnet_id: &str) -> bool {
        self.answered.iter().any(|asked| asked == subnet_id)
    }

    /// Whether EC2 reported a subnet for `subnet_id`.
    fn reported(&self, subnet_id: &str) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.subnet_id == subnet_id)
    }
}

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and nothing leaves this
/// process. The credentials are the callback's, which sign this in-process hop and never
/// appear in a response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// rather than interpolated as given: `None` for a value that would name an authority the
/// caller chose instead of the peer service this emulator asks.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return None;
    };
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // the callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// Every subnet of `subnet_ids` the account has in `region`, as EC2 reports it, in the
/// order the request named them, beside the identifiers EC2 ANSWERED about.
///
/// An identifier EC2 reports no subnet for is simply ABSENT from the placements, and so is
/// every identifier when there is no EC2 to ask: this resolves what the account's network
/// says, and says nothing of its own about what it could not resolve — which is why the
/// identifiers EC2 answered about travel back beside the subnets ([`ResolvedSubnets`]).
///
/// EVERY identifier the request named is asked about, deduplicated and in hops of
/// [`IDS_PER_HOP`] ([`asked_ids`]) — so a placement is resolved whole rather than only as
/// far as one hop reaches, and a caller cannot push a subnet past the question by listing
/// more of them.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> ResolvedSubnets {
    let asked = asked_ids(subnet_ids);
    if asked.is_empty() {
        return ResolvedSubnets::default();
    }
    let Some(client) = ec2(ctx, region) else {
        return ResolvedSubnets::default();
    };
    let mut reported: Vec<::aws_sdk_ec2::types::Subnet> = Vec::new();
    // The identifiers a hop that ANSWERED carried: what EC2 said nothing about is the rest,
    // and a hop that failed leaves its own identifiers among them.
    let mut answered: Vec<String> = Vec::new();
    for hop in asked.chunks(IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(hop.to_vec()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => {
                reported.extend(output.subnets().iter().cloned());
                answered.extend(hop.iter().cloned());
            }
            Err(error) => {
                // At `warn`: a deployment where this hop fails is one where the network
                // behind an endpoint is never resolved, which is worth seeing — and it IS
                // why an identifier did not resolve. The hops already answered are kept:
                // every placement they resolved is still what the account's network says.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a request named could not be resolved against EC2"
                );
            }
        }
    }
    // Indexed by identifier ONCE, so resolving a placement costs a logarithmic lookup per
    // identifier rather than a scan of every row EC2 answered.
    let by_id: BTreeMap<&str, &::aws_sdk_ec2::types::Subnet> = reported
        .iter()
        .filter_map(|subnet| subnet.subnet_id().map(|id| (id, subnet)))
        .collect();
    // Over the identifiers that were ASKED about — each once, and capped — in the order the
    // REQUEST named them, so what a caller is told about is the subnet it listed rather than
    // whichever row EC2 happened to answer first, and a repeated identifier is one placement
    // rather than a copy per mention.
    let placements = asked
        .iter()
        .filter_map(|subnet_id| {
            let subnet = by_id.get(subnet_id.as_str())?;
            Some(SubnetPlacement {
                subnet_id: subnet_id.clone(),
                vpc_id: subnet.vpc_id().map(str::to_owned),
                availability_zone: subnet.availability_zone().map(str::to_owned),
            })
        })
        .collect();
    ResolvedSubnets {
        placements,
        answered,
    }
}

/// How this service states that an identifier names no subnet of the account: the fixed
/// text around the identifier the REQUEST sent, in single quotes.
///
/// A subnet belongs to the service that owns subnets, so what the caller is told is that
/// the identifier it sent names none there — the identifier is the one variable part, and
/// nothing else of the request appears.
fn subnet_does_not_exist(subnet_id: &str) -> String {
    format!("The subnet ID '{subnet_id}' does not exist")
}

/// The first entry of `subnet_ids` the account holds no subnet for, and the refusal naming
/// it, or `None` where every entry names a subnet — which includes every entry EC2 was not
/// asked about.
///
/// `VPCOptions$SubnetIds` names subnets of the service that OWNS them, so an identifier
/// that service answered no subnet for names nothing a placement could be created in: the
/// call is refused with the identifier the request sent and nothing is written. The mirror
/// of [`unknown_security_group_refusal`], grounded the same way
/// ([`ResolvedSubnets::answered_about`]): an identifier EC2 was never asked about — an
/// emulator with no EC2 mounted, a hop that failed — is evidence of nothing and refuses
/// nothing.
pub fn unknown_subnet_refusal(
    subnet_ids: Option<&[String]>,
    resolved: &ResolvedSubnets,
) -> Option<ValidationException> {
    let unknown = subnet_ids?
        .iter()
        .find(|id| resolved.answered_about(id) && !resolved.reported(id))?;
    Some(ValidationException {
        message: Some(subnet_does_not_exist(unknown)),
    })
}

/// The ONE VPC every resolved subnet belongs to, or `None` where they resolve to none or
/// to more than one.
///
/// `VPCDerivedInfo$VPCId` is a single VPC, so a set of subnets that does not agree on one
/// names no VPC to report — which is the same answer as a placement nothing could be
/// resolved for. A CALLER naming subnets of two VPCs is told so instead of being given
/// such a record ([`subnets_span_two_vpcs`]).
pub fn placed_vpc(placements: &[SubnetPlacement]) -> Option<String> {
    let mut vpcs = placements.iter().filter_map(|placement| placement.vpc_id.as_deref());
    let first = vpcs.next()?;
    vpcs.all(|vpc| vpc == first).then(|| first.to_owned())
}

/// The two subnets of a placement that EC2 puts in DIFFERENT VPCs — the first such pair in
/// the order the request named them — or `None` where every resolved subnet is in one VPC
/// (which includes a placement nothing resolved for).
///
/// A VPC-access placement — a domain's or an endpoint's — lives in exactly one VPC, and
/// the subnets it is created in are what name it, so a placement whose subnets disagree
/// names no VPC at all: the call that asked for it is refused rather than stored with no
/// `VPCId` — a record whose own VPC nothing could state afterwards.
pub fn subnets_span_two_vpcs(placements: &[SubnetPlacement]) -> Option<(&SubnetPlacement, &SubnetPlacement)> {
    let mut placed = placements
        .iter()
        .filter(|placement| placement.vpc_id.is_some());
    let first = placed.next()?;
    placed
        .find(|placement| placement.vpc_id != first.vpc_id)
        .map(|other| (first, other))
}

/// How this service states that the subnets a VPC-access placement names do not all
/// belong to ONE VPC: one fixed sentence with no variable part.
///
/// A placement lives in exactly one VPC — `VPCDerivedInfo$VPCId` is a single VPC derived
/// from those subnets — so a set of subnets that disagrees names no placement at all, and
/// what the caller is told is the requirement the set failed rather than which of its
/// members is the odd one out.
pub const SUBNETS_NOT_IN_ONE_VPC: &str = "The subnets must be in the same VPC.";

/// The `ValidationException` payload ([`SUBNETS_NOT_IN_ONE_VPC`]) for a request whose
/// `VPCOptions$SubnetIds` resolve, in EC2, to subnets of more than one VPC.
pub fn subnets_not_in_one_vpc() -> ValidationException {
    ValidationException {
        message: Some(SUBNETS_NOT_IN_ONE_VPC.to_string()),
    }
}

/// How the service names the number of subnets a placement takes, for the sentence the
/// refusal below states: the counts a domain is placed in are [`SINGLE_ZONE`] and the
/// values of [`ZONE_COUNT_RANGE`], and the service states such a count in words rather
/// than in digits (the same way it words how many nodes a zone deployment takes).
const SUBNET_COUNT_WORDS: [(i32, &str); 3] = [(1, "one"), (2, "two"), (3, "three")];

/// How many subnets a VPC-access domain whose cluster is `cluster` is placed with: ONE per
/// Availability Zone the cluster is placed in — [`SINGLE_ZONE`] where it asks for no zone
/// awareness, and the zone count it asks for where it does
/// ([`super::requested_zone_count`]).
///
/// `None` where the cluster asks for zone awareness with a count the service does not place
/// a domain in: that count names no number of zones at all, which
/// [`super::zone_count_refusal`] states itself, so there is no number of subnets to require.
fn placement_subnet_count(cluster: Option<&ClusterConfig>) -> Option<i32> {
    match cluster {
        Some(cluster) if cluster.zone_awareness_enabled == Some(true) => {
            super::requested_zone_count(cluster)
        }
        // A cluster that asks for no zone awareness — and the absence of a cluster
        // configuration, which asks for the service's own defaults, zone awareness off
        // among them — occupies a single zone.
        _ => Some(SINGLE_ZONE),
    }
}

/// Whether the subnets a VPC-access domain is placed in are as many as the Availability
/// Zones its cluster is placed in, and the refusal stating that number otherwise.
///
/// `VPCOptions$SubnetIds` is documented "if your domain uses multiple Availability Zones,
/// you need to provide two subnet IDs, one per zone. Otherwise, provide only one", and
/// `ClusterConfig$ZoneAwarenessEnabled` is the switch that decides how many zones the
/// domain uses: so a VPC-access domain names exactly ONE subnet per zone it is placed in
/// ([`placement_subnet_count`]), and a list of any other length describes a placement the
/// service does not provision — it is refused rather than stored, so no domain is left
/// behind naming subnets its own topology contradicts.
///
/// `None` where the call names no subnet list at all (a domain reached on a public endpoint
/// has no placement to judge) and where the cluster's zone count is not one the service
/// places a domain in — [`super::zone_count_refusal`] states that count itself.
pub fn subnet_count_refusal(
    cluster: Option<&ClusterConfig>,
    subnet_ids: Option<&[String]>,
) -> Option<ValidationException> {
    let named = subnet_ids?.len();
    let required = placement_subnet_count(cluster)?;
    // Compared as a wide signed count, so a list of any length is judged without a cast
    // that could wrap it into the required number.
    if i64::try_from(named).is_ok_and(|named| named == i64::from(required)) {
        return None;
    }
    let (_, word) = SUBNET_COUNT_WORDS
        .into_iter()
        .find(|(count, _)| *count == required)?;
    let plural = if required == SINGLE_ZONE { "" } else { "s" };
    Some(ValidationException {
        message: Some(format!("You must specify exactly {word} subnet{plural}.")),
    })
}

/// The EC2 filter that selects security groups BY ID (`group-id`, listed by
/// `DescribeSecurityGroups`'s own `Filters` documentation).
///
/// A filter rather than `GroupIds`, for the reason [`SUBNET_ID_FILTER`] is one: an
/// identifier the account has no group for is then an absent row instead of a failure of
/// the whole call, so every identifier the request named is still resolved against what EC2
/// does hold.
const SECURITY_GROUP_ID_FILTER: &str = "group-id";

/// One security group of the account, reduced to what this service reads off it: which VPC
/// EC2 reports it governs traffic in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityGroupPlacement {
    /// The identifier the request named, which is the one EC2 answered for.
    pub group_id: String,
    /// The VPC EC2 reports the group governs traffic in, absent where its record carries
    /// none.
    pub vpc_id: Option<String>,
}

/// What the service that OWNS security groups answered about the groups a request named:
/// the groups it reported, and which identifiers it answered about at all.
///
/// The two travel together because a group that is ABSENT from the answer means one of two
/// different things — EC2 holds no such group, or EC2 was never asked (no EC2 in this
/// process, a hop that failed, an identifier past the resolution's own ceiling) — and only
/// the first is evidence about the account's network ([`unknown_security_group_refusal`]).
#[derive(Debug, Clone, Default)]
pub struct ResolvedSecurityGroups {
    /// The groups EC2 reported, in the order the request named them.
    pub placements: Vec<SecurityGroupPlacement>,
    /// The identifiers EC2 ANSWERED about — the ones a question was put to it about and
    /// answered. An identifier outside this list is one nothing is known about.
    answered: Vec<String>,
}

impl ResolvedSecurityGroups {
    /// Whether EC2 answered about `group_id`, so what it reported for it — a group, or
    /// nothing — is evidence about the account's network.
    #[must_use]
    pub fn answered_about(&self, group_id: &str) -> bool {
        self.answered.iter().any(|asked| asked == group_id)
    }

    /// Whether EC2 reported a security group for `group_id`.
    fn reported(&self, group_id: &str) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.group_id == group_id)
    }
}

/// Every security group of `group_ids` the account has in `region`, as EC2 reports it, in
/// the order the request named them.
///
/// A security group is Amazon EC2's resource, so which VPC one governs traffic in — and
/// whether the account holds one at all — is EC2's to say, and this resolves exactly that:
/// an identifier EC2 reports no group for is simply ABSENT from the answer, and so is every
/// identifier when there is no EC2 to ask. Nothing of this service's own is said about what
/// could not be resolved, which is why the identifiers EC2 ANSWERED about travel back beside
/// the groups ([`ResolvedSecurityGroups`]).
///
/// Asked about in the same hops a placement's subnets are ([`asked_ids`]), for the same
/// reason.
pub async fn resolve_security_groups(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_ids: &[String],
) -> ResolvedSecurityGroups {
    let asked = asked_ids(group_ids);
    if asked.is_empty() {
        return ResolvedSecurityGroups::default();
    }
    let Some(client) = ec2(ctx, region) else {
        return ResolvedSecurityGroups::default();
    };
    let mut reported: Vec<::aws_sdk_ec2::types::SecurityGroup> = Vec::new();
    // The identifiers a hop that ANSWERED carried: what EC2 said nothing about is the rest,
    // and a hop that failed leaves its own identifiers among them.
    let mut answered: Vec<String> = Vec::new();
    for hop in asked.chunks(IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SECURITY_GROUP_ID_FILTER)
            .set_values(Some(hop.to_vec()))
            .build();
        match client
            .describe_security_groups()
            .filters(by_id)
            .send()
            .await
        {
            Ok(output) => {
                reported.extend(output.security_groups().iter().cloned());
                answered.extend(hop.iter().cloned());
            }
            Err(error) => {
                // At `warn`, for the reason the subnet hop warns: a deployment where this
                // hop fails is one where the network a placement is judged against is
                // never resolved, and it IS why an identifier did not resolve.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the security groups a request named could not be resolved against EC2"
                );
            }
        }
    }
    // Indexed and walked the way the subnets above are, for the same reasons: one lookup
    // per identifier asked about, each identifier once, in the order the REQUEST named them.
    let by_id: BTreeMap<&str, &::aws_sdk_ec2::types::SecurityGroup> = reported
        .iter()
        .filter_map(|group| group.group_id().map(|id| (id, group)))
        .collect();
    let placements = asked
        .iter()
        .filter_map(|group_id| {
            let group = by_id.get(group_id.as_str())?;
            Some(SecurityGroupPlacement {
                group_id: group_id.clone(),
                vpc_id: group.vpc_id().map(str::to_owned),
            })
        })
        .collect();
    ResolvedSecurityGroups {
        placements,
        answered,
    }
}

/// The prefix every security group identifier carries, which is also the form the refusal
/// below tells a caller to write: a security group is Amazon EC2's resource and `sg-` is the
/// prefix that service issues one under.
const SECURITY_GROUP_ID_PREFIX: &str = "sg-";

/// How many hexadecimal digits follow that prefix: the short and the long identifier form
/// EC2 issues a group id in (8 and 17 digits), which is the whole of the id space — a value
/// of any other width names no group any account could hold.
const SECURITY_GROUP_ID_DIGITS: [usize; 2] = [8, 17];

/// Whether `id` is an identifier a security group could be held under at all: the `sg-`
/// prefix followed by one of the two widths of hexadecimal digits EC2 issues
/// ([`SECURITY_GROUP_ID_DIGITS`]).
///
/// The digits are LOWERCASE hexadecimal, which is the only case that service issues an
/// identifier in, so an uppercase digit names no group either.
fn names_a_security_group(id: &str) -> bool {
    let Some(digits) = id.strip_prefix(SECURITY_GROUP_ID_PREFIX) else {
        return false;
    };
    SECURITY_GROUP_ID_DIGITS.contains(&digits.len())
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The fixed text both identifier refusals open with, followed by the offending value from
/// the REQUEST in double quotes — the one rendering of "this value is not an identifier of
/// the id space it was given for", so the two classes differ only in what follows it.
fn invalid_id(id: &str) -> String {
    format!("Invalid id: \"{id}\"")
}

/// The first entry of `group_ids` that is not of the security-group identifier FORM, and the
/// refusal naming it, or `None` where every entry could name a group.
///
/// A value outside that form names nothing in any account, so it is refused for its SYNTAX —
/// a fact of the id space rather than of the account's network, which is why this resolves
/// nothing, and why it is decided before existence: a value that is no identifier cannot be
/// looked up. The sentence closes with the form the caller should have written.
pub fn malformed_security_group_refusal(
    group_ids: Option<&[String]>,
) -> Option<ValidationException> {
    let malformed = group_ids?.iter().find(|id| !names_a_security_group(id))?;
    Some(ValidationException {
        message: Some(format!(
            "{} (expecting \"{SECURITY_GROUP_ID_PREFIX}...\")",
            invalid_id(malformed)
        )),
    })
}

/// The first entry of `group_ids` the account holds no security group for, and the refusal
/// naming it, or `None` where every entry names a group — which includes every entry EC2 was
/// not asked about.
///
/// `VPCOptions$SecurityGroupIds` names groups of the service that OWNS them, so an identifier
/// that service answered for no group names nothing a network interface of the placement
/// could carry: the call is refused with the identifier the request sent, and nothing is
/// written. Grounded only in what EC2 ANSWERED
/// ([`ResolvedSecurityGroups::answered_about`]): an identifier it was never asked about — an
/// emulator with no EC2 mounted, a hop that failed — is evidence of nothing and refuses
/// nothing.
pub fn unknown_security_group_refusal(
    group_ids: Option<&[String]>,
    resolved: &ResolvedSecurityGroups,
) -> Option<ValidationException> {
    let unknown = group_ids?
        .iter()
        .find(|id| resolved.answered_about(id) && !resolved.reported(id))?;
    Some(ValidationException {
        message: Some(invalid_id(unknown)),
    })
}

/// How this service states that the security groups a VPC-access placement names do not
/// govern traffic in the VPC its subnets are in: one fixed sentence with no variable part,
/// because what the caller has to fix is WHICH network the pair describes rather than
/// anything about one value.
pub const SECURITY_GROUPS_NOT_IN_SUBNET_VPC: &str =
    "Subnets and security groups must be in the same VPC.";

/// The first resolved security group that governs traffic in a VPC OTHER than `vpc_id`, in
/// the order the request named them, or `None` where every resolved group governs traffic
/// in that VPC.
///
/// A group EC2 resolved no VPC for is not evidence of governing another one, so it is not
/// what this answers — only a group EC2 placed in a DIFFERENT VPC is.
pub fn security_group_outside_vpc<'a>(
    vpc_id: &str,
    groups: &'a [SecurityGroupPlacement],
) -> Option<&'a SecurityGroupPlacement> {
    groups.iter().find(|group| {
        group
            .vpc_id
            .as_deref()
            .is_some_and(|governs| governs != vpc_id)
    })
}

/// The `ValidationException` payload ([`SECURITY_GROUPS_NOT_IN_SUBNET_VPC`]) for a
/// placement whose security groups do not govern traffic in `vpc_id`, the VPC the placement
/// itself lives in.
///
/// A VPC-access placement — a domain's or an endpoint's — is reached through network
/// interfaces that live in its subnets, and a security group governs traffic only to
/// interfaces of the VPC it belongs to: a group of another VPC can never be attached to
/// them, so the call asking for that pair is refused rather than stored as a placement
/// nothing could reach. Decided from what EC2 ANSWERED on both sides: a placement whose own
/// VPC is not known (`None`) and a group EC2 resolved nothing for ground no refusal.
pub fn security_groups_outside_vpc_refusal(
    vpc_id: Option<&str>,
    groups: &[SecurityGroupPlacement],
) -> Option<ValidationException> {
    security_group_outside_vpc(vpc_id?, groups)?;
    Some(ValidationException {
        message: Some(SECURITY_GROUPS_NOT_IN_SUBNET_VPC.to_string()),
    })
}

/// The Availability Zones the resolved subnets sit in, in the order the request named the
/// subnets, each zone once; `None` where no resolved subnet names a zone.
///
/// Membership is kept in a set rather than rescanned per zone, so the work is logarithmic in
/// the zones already seen however many subnets a request named.
pub fn placed_zones(placements: &[SubnetPlacement]) -> Option<Vec<String>> {
    let mut zones: Vec<String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for zone in placements
        .iter()
        .filter_map(|placement| placement.availability_zone.as_deref())
    {
        if seen.insert(zone) {
            zones.push(zone.to_owned());
        }
    }
    (!zones.is_empty()).then_some(zones)
}

/// The first resolved subnet that is NOT in `vpc_id`, in the order the request named
/// them, or `None` where every resolved subnet is in that VPC.
///
/// A subnet EC2 resolved no VPC for is not evidence of being outside one, so it is not
/// what this answers — only a subnet EC2 placed in ANOTHER VPC is.
pub fn subnet_outside_vpc<'a>(
    vpc_id: &str,
    placements: &'a [SubnetPlacement],
) -> Option<&'a SubnetPlacement> {
    placements.iter().find(|placement| {
        placement
            .vpc_id
            .as_deref()
            .is_some_and(|placed| placed != vpc_id)
    })
}

/// `VPCOptions` as the service reports it back, with the two members EC2 derives from the
/// subnets filled in from what EC2 answered for them.
///
/// [`stored_vpc_options`] is what stores the members the REQUEST carried; this adds the
/// derived ones, and adds nothing where the subnets resolved to nothing.
pub fn placed_vpc_options(input: VPCOptions, placements: &[SubnetPlacement]) -> VPCDerivedInfo {
    let mut options = stored_vpc_options(input);
    options.vpc_id = placed_vpc(placements);
    options.availability_zones = placed_zones(placements);
    options
}

/// The Service Quotas service code the account's Amazon VPC quotas are published under.
const VPC_QUOTA_SERVICE_CODE: &str = "vpc";

/// The `QuotaCode` of "Security groups per network interface" — "the maximum number of
/// security groups per network interface" — in the account's Amazon VPC quotas.
const SECURITY_GROUPS_PER_INTERFACE_QUOTA: &str = "L-2AFB9258";

/// The value that quota is in force at where this emulator cannot read the account's own:
/// an engine with no Service Quotas store mounted, or one whose catalog does not publish
/// the row. It is AWS's published DEFAULT value for the same quota, so the ceiling is the
/// account's wherever the account can be asked and the published default otherwise.
const SECURITY_GROUPS_PER_INTERFACE_DEFAULT: usize = 5;

/// How many security groups ONE elastic network interface of this account may carry in
/// `region`: the value the account's "Security groups per network interface" quota is in
/// force at.
///
/// The quota is the account's and ADJUSTABLE, so it is read out of the Service Quotas
/// store — the store `service-quotas` serves `GetServiceQuota` from, where an applied
/// increase is recorded — rather than being a number this service decides. A value below
/// one would admit no placement at all, so the floor is one group.
fn security_groups_per_interface<Eng: Send + Sync + 'static>(engine: &Eng, region: &str) -> usize {
    let applied = ::service_com_amazonaws_servicequotas_servicequotasv20190624::state::service_state(
        engine,
    )
    .and_then(|quotas| {
        quotas.lock().applied_value(
            region,
            VPC_QUOTA_SERVICE_CODE,
            SECURITY_GROUPS_PER_INTERFACE_QUOTA,
            None,
        )
    });
    applied
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map_or(SECURITY_GROUPS_PER_INTERFACE_DEFAULT, |value| {
            value as usize
        })
}

/// Whether the security groups `options` names fit on the network interface the placement
/// is reached through, and the refusal stating the ceiling otherwise.
///
/// `VPCOptions$SecurityGroupIds` carries no modelled cardinality bound, and the groups it
/// names are attached to the elastic network interface the placement is served by — so the
/// ceiling is the account's own "Security groups per network interface" quota
/// ([`security_groups_per_interface`]). A request naming more groups than that asks for an
/// interface the account's network cannot carry, so it is refused rather than stored, and
/// the placement keeps exactly the groups it had.
///
/// The sentence states the limit in force and the placement the groups were named for
/// (`placement`, one of [`VPC_ENDPOINT_PLACEMENT`] and [`DOMAIN_PLACEMENT`]): the two
/// calls that take `VPCOptions` are about different resources, and a caller is told which
/// one it was refused about.
pub fn security_groups_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    options: Option<&VPCOptions>,
    placement: &str,
) -> Option<ValidationException> {
    let named = options?.security_group_ids.as_ref()?.len();
    let ceiling = security_groups_per_interface(engine, region);
    if named <= ceiling {
        return None;
    }
    Some(ValidationException {
        message: Some(format!(
            "You’ve exceeded the limit for security groups. You can select up to {ceiling} \
             security groups for {placement}."
        )),
    })
}

/// What the refusal names an interface VPC ENDPOINT's placement as.
pub const VPC_ENDPOINT_PLACEMENT: &str = "the VPC endpoint";

/// What it names a VPC-access DOMAIN's placement as — the same statement about the same
/// quota, for the other resource this service places in a VPC.
pub const DOMAIN_PLACEMENT: &str = "the domain";

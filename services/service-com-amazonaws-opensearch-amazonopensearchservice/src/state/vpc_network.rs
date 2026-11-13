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

use std::collections::BTreeSet;

use super::*;

/// The EC2 filter that selects subnets BY ID (`subnet-id`, listed by `DescribeSubnets`'s
/// own `Filters` documentation).
///
/// A filter rather than `SubnetIds`: an identifier the account has no subnet for is then
/// an absent row instead of a failure of the whole call, so every identifier the request
/// named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers asked about in ONE hop to EC2.
///
/// A hop is evaluated against every subnet the account stores, so this caps the
/// identifiers one question carries; the identifiers a request names beyond it are asked
/// about in further hops ([`MAX_RESOLVED_SUBNET_HOPS`] of them).
const SUBNET_IDS_PER_HOP: usize = 64;

/// The most hops one call makes, and so — with [`SUBNET_IDS_PER_HOP`] — the most
/// identifiers it resolves.
///
/// CHOSEN as a process-safety ceiling, not a published quota: the model declares no
/// `@length` on `VPCOptions$SubnetIds`, so without one the work a single request causes
/// would be sized by that request. A VPC endpoint is placed with one subnet per
/// Availability Zone, so the product is far above what any real placement names;
/// identifiers past it are left unresolved, which — like an EC2 that cannot be asked —
/// grounds nothing.
const MAX_RESOLVED_SUBNET_HOPS: usize = 8;

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
/// order the request named them.
///
/// An identifier EC2 reports no subnet for is simply ABSENT from the answer, and so is
/// every identifier when there is no EC2 to ask: this resolves what the account's network
/// says, and says nothing of its own about what it could not resolve.
///
/// EVERY identifier the request named is asked about, deduplicated and in hops of
/// [`SUBNET_IDS_PER_HOP`] — so a placement is resolved whole rather than only as far as
/// one hop reaches, and a caller cannot push a subnet past the question by listing more of
/// them. The work is bounded by [`MAX_RESOLVED_SUBNET_HOPS`] hops whatever the request
/// carries.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Vec<SubnetPlacement> {
    let ceiling = SUBNET_IDS_PER_HOP * MAX_RESOLVED_SUBNET_HOPS;
    let mut asked: Vec<String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for subnet_id in subnet_ids {
        if asked.len() == ceiling {
            break;
        }
        if seen.insert(subnet_id.as_str()) {
            asked.push(subnet_id.clone());
        }
    }
    if asked.is_empty() {
        return Vec::new();
    }
    let Some(client) = ec2(ctx, region) else {
        return Vec::new();
    };
    let mut reported: Vec<::aws_sdk_ec2::types::Subnet> = Vec::new();
    for hop in asked.chunks(SUBNET_IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(hop.to_vec()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => reported.extend(output.subnets().iter().cloned()),
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
    // In the order the REQUEST named them, so what a caller is told about is the subnet
    // it listed rather than whichever row EC2 happened to answer first.
    subnet_ids
        .iter()
        .filter_map(|subnet_id| {
            let subnet = reported
                .iter()
                .find(|subnet| subnet.subnet_id() == Some(subnet_id.as_str()))?;
            Some(SubnetPlacement {
                subnet_id: subnet_id.clone(),
                vpc_id: subnet.vpc_id().map(str::to_owned),
                availability_zone: subnet.availability_zone().map(str::to_owned),
            })
        })
        .collect()
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

/// The Availability Zones the resolved subnets sit in, in the order the request named the
/// subnets, each zone once; `None` where no resolved subnet names a zone.
pub fn placed_zones(placements: &[SubnetPlacement]) -> Option<Vec<String>> {
    let mut zones: Vec<String> = Vec::new();
    for zone in placements
        .iter()
        .filter_map(|placement| placement.availability_zone.as_deref())
    {
        if !zones.iter().any(|seen| seen == zone) {
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

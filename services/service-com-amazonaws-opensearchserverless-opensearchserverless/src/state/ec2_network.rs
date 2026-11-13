// SPDX-License-Identifier: CC0-1.0

//! The EC2 network a managed VPC endpoint is placed in, resolved against the service that
//! OWNS it.
//!
//! `CreateVpcEndpoint` names the VPC an endpoint is reached from and the subnets its network
//! interfaces go in, and `UpdateVpcEndpoint` names more subnets to add. None of those is a
//! resource of this service: a VPC and a subnet are Amazon EC2's, so whether one exists is
//! EC2's to answer, and the EC2 service mounted in THIS emulator is asked — `DescribeVpcs`
//! and `DescribeSubnets` over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate — BEFORE an endpoint id is minted or anything is stored.
//!
//! NOTHING here reaches the network. The callback is the framework's in-process dispatcher:
//! it hands the request to a service registered in this same process, so no socket is opened
//! and no address out of the request is contacted. The Region label the caller signed for
//! selects which of this emulator's own regional state answers, not a host.
//!
//! What a refusal SAYS is this service's own sentence about the value the REQUEST carried
//! ([`vpc_absent_message`], [`subnet_absent_message`]); EC2's answer decides WHETHER the
//! request is refused, not how the refusal reads, and nothing of EC2's own error travels to
//! the caller.
//!
//! This service only GATES on existence — it keeps nothing of the VPC or the subnet beyond
//! the ids the request carried — so an emulator with no EC2 mounted learns nothing to refuse
//! a request on and refuses nothing: a refusal is grounded in what EC2 reported, never
//! assumed from silence, and an attempt that answered nothing is reported at `warn`.

/// An AWS SDK client for EC2 over the in-process callback into this same emulator, or `None`
/// when `region` is not a Region LABEL.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// dispatches IN PROCESS to a service registered in this very emulator, so no socket is
/// opened and nothing leaves the host. The endpoint URL is only the address the SDK needs to
/// name the service and Region it is addressing — the callback's HTTP client never resolves
/// or connects to it — which is also why there is nothing to retry, time out, watch for a
/// stalled stream or cache an identity for.
///
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends up in that
/// URL's authority, so it is read as a Region label (`service_common::arn::region_label`, the
/// one place this tree states that grammar) instead of being interpolated as given: a value
/// carrying a host or path delimiter would name an authority the caller chose rather than the
/// peer service this emulator asks.
fn ec2_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
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
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// The name of the EC2 filter that selects a subnet BY ID, the way the subnet-placing
/// services of this tree resolve a placement (`services/common/src/network.rs`).
///
/// Filtering rather than naming the ids as `subnetIds` keeps an id EC2 holds no subnet for on
/// the SUCCESS path: the read answers the subnets it resolved and the ones it did not come out
/// of the diff, so the refusal names the very value the request carried, in ONE read for the
/// whole list.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The name of the EC2 filter that selects a VPC by id, for the same reason
/// [`SUBNET_ID_FILTER`] is used.
const VPC_ID_FILTER: &str = "vpc-id";

/// The prefix of every EC2 error code that says a value names no VPC of the account:
/// `InvalidVpcID.NotFound` for an id no VPC carries, `InvalidVpcID.Malformed` for a value
/// that is not a VPC id at all. Both say the same thing about a placement.
const VPC_REFUSAL_PREFIX: &str = "InvalidVpcID.";

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` and `InvalidSubnetID.Malformed`.
const SUBNET_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// How this service words a `vpcId` that names no VPC of the account.
///
/// The sentence is this service's own, in the form it words a resource an id addresses that
/// is not there — `The <Kind> with Id '<id>' does not exist.`, the sentence
/// [`super::index_collection_not_found_message`] words for a collection — with the id
/// SINGLE-QUOTED and a trailing period. The only variable part is the value the request
/// carried.
pub fn vpc_absent_message(vpc_id: &str) -> String {
    format!("The Vpc with Id '{vpc_id}' does not exist.")
}

/// How this service words a `subnetIds` entry that names no subnet of the account; the same
/// sentence [`vpc_absent_message`] words for a VPC, naming the subnet instead.
pub fn subnet_absent_message(subnet_id: &str) -> String {
    format!("The Subnet with Id '{subnet_id}' does not exist.")
}

/// Why the network a request names cannot be placed in, as the MESSAGE a
/// `ValidationException` states — or `None` when EC2 resolved every id the request carried,
/// and also when there is no EC2 to ask.
///
/// The VPC is resolved first, since a subnet exists only inside one: a request whose VPC
/// names nothing is answered about that VPC rather than about the subnets beside it.
///
/// The caller-sized subnet list is the one the handler has already held to its own bound, so
/// the resolving reads below are not sized by the request.
pub async fn unresolved_network(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: Option<&str>,
    subnet_ids: &[String],
) -> Option<String> {
    let names_a_vpc = vpc_id.is_some_and(|id| !id.is_empty());
    if !names_a_vpc && subnet_ids.is_empty() {
        return None;
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an unreachable EC2
    // gets, which grounds no refusal.
    let ec2 = ec2_client(ctx, region)?;
    if let Some(vpc_id) = vpc_id.filter(|id| !id.is_empty()) {
        if let Some(refusal) = unresolved_vpc(&ec2, vpc_id).await {
            return Some(refusal);
        }
    }
    unresolved_subnets(&ec2, subnet_ids).await
}

/// What EC2 answers about the VPC `vpc_id` names.
async fn unresolved_vpc(ec2: &::aws_sdk_ec2::Client, vpc_id: &str) -> Option<String> {
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ID_FILTER)
        .values(vpc_id.to_string())
        .build();
    let error = match ec2.describe_vpcs().filters(by_id).send().await {
        Ok(output) => {
            // A read that resolved but reports no VPC for the id the request named says the
            // same thing a refusal does, and about the same id.
            if output.vpcs().iter().any(|vpc| vpc.vpc_id() == Some(vpc_id)) {
                return None;
            }
            return Some(vpc_absent_message(vpc_id));
        }
        Err(error) => error,
    };
    grounds_a_refusal(&error, VPC_REFUSAL_PREFIX, "the VPC").then(|| vpc_absent_message(vpc_id))
}

/// What EC2 answers about the subnets `subnet_ids` names: ONE read filtered by id, so an id
/// it holds no subnet for comes out of the diff between what was asked for and what was
/// reported, and the refusal names that very id.
///
/// The list is the one the handler has already held to its own maximum, so neither the read
/// nor the diff is sized by the request.
async fn unresolved_subnets(ec2: &::aws_sdk_ec2::Client, subnet_ids: &[String]) -> Option<String> {
    if subnet_ids.is_empty() {
        return None;
    }
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(SUBNET_ID_FILTER)
        .set_values(Some(subnet_ids.to_vec()))
        .build();
    let error = match ec2.describe_subnets().filters(by_id).send().await {
        Ok(output) => {
            let reported: Vec<&str> = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::subnet_id)
                .collect();
            let absent = subnet_ids
                .iter()
                .find(|id| !reported.contains(&id.as_str()))?;
            return Some(subnet_absent_message(absent));
        }
        Err(error) => error,
    };
    // A REFUSED read names no one id — a filtered read is not refused for an id that
    // resolves to nothing, so this is the malformed-value answer — and the request is
    // answered about the first id of the list it carried.
    let named = subnet_ids.first()?;
    grounds_a_refusal(&error, SUBNET_REFUSAL_PREFIX, "the subnets")
        .then(|| subnet_absent_message(named))
}

/// Whether an EC2 read that FAILED grounds a refusal: its error code says one of the ids
/// names no such resource of the account.
///
/// Anything else is this emulator failing to ASK rather than EC2 reporting a bad id — there
/// is no EC2 mounted, or the answer could not be read — and nothing is what a request is
/// refused on then. It is worth a warning, because a deployment with no EC2 to ask is one
/// where this gate does not run.
fn grounds_a_refusal<
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    refusal_prefix: &str,
    named: &str,
) -> bool {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if error
        .code()
        .is_some_and(|code| code.starts_with(refusal_prefix))
    {
        return true;
    }
    ::tracing::warn!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "{named} a request named could not be resolved against EC2"
    );
    false
}

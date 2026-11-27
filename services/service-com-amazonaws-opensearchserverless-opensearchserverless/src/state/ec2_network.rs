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
//! What a refusal SAYS is what EC2 ANSWERED: the sentence EC2 states about the id the
//! request carried, carried through verbatim inside the envelope the SDK layer that relayed
//! the call states around a failed call — the service it was made to, the status that service
//! answered with, that call's own request id and how many attempts it took
//! ([`refusal_ec2_answered`], over this workspace's one rendering of that envelope,
//! `service_common::relayed_refusal`). This service words no sentence of its own for a
//! placement it does not own: a client reads AWS's own sentence about the value it sent, and
//! which service refused it.
//!
//! The SUBNETS are resolved first, because they are where the request places the endpoint's
//! network interfaces: a request whose subnets resolve to nothing is answered about them even
//! when the VPC beside them resolves to nothing too.
//!
//! This service only GATES on existence — it keeps nothing of the VPC or the subnet beyond
//! the ids the request carried, and the one thing EC2 ANSWERED about the VPC that outlives
//! the call ([`Placement::in_default_vpc`], which is where a client that built no network of
//! its own is) — so an emulator with no EC2 mounted learns nothing to refuse a request on and
//! refuses nothing: a refusal is grounded in what EC2 reported, never assumed from silence,
//! and an attempt that answered nothing is reported at `warn`.

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

/// The prefix of every EC2 error code that says a value names no VPC of the account:
/// `InvalidVpcID.NotFound` for an id no VPC carries, `InvalidVpcID.Malformed` for a value
/// that is not a VPC id at all. Both say the same thing about a placement.
const VPC_REFUSAL_PREFIX: &str = "InvalidVpcID.";

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` and `InvalidSubnetID.Malformed`.
const SUBNET_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The called service's name as the SDK layer that relayed the call reports it in a relayed
/// refusal's envelope — neither EC2's endpoint prefix nor the casing of its `sdkId`.
const EC2_RELAY_SERVICE: &str = "Ec2";

/// What a request's network placement resolved to against EC2.
#[derive(Debug, Default)]
pub struct Placement {
    /// Why the network a request names cannot be placed in, as the MESSAGE a
    /// `ValidationException` states — `None` when EC2 resolved every id the request carried,
    /// and also when there is no EC2 to ask.
    pub refusal: Option<String>,
    /// Whether EC2 reports the named VPC as the ACCOUNT'S DEFAULT one — the network AWS gives
    /// an account that created none of its own.
    ///
    /// `None` when nothing answered about it: no VPC was named, there was no EC2 to ask, or
    /// the read did not resolve. Reported because a refusal elsewhere in this crate is
    /// grounded in what EC2 said, never assumed from silence, and so is what this value is
    /// read for ([`super::network_access`]).
    pub in_default_vpc: Option<bool>,
}

/// Why the network a request names cannot be placed in, as the MESSAGE a
/// `ValidationException` states — or `None` when EC2 resolved every id the request carried,
/// and also when there is no EC2 to ask ([`Placement::refusal`]).
pub async fn unresolved_network(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: Option<&str>,
    subnet_ids: &[String],
) -> Option<String> {
    placement(ctx, region, vpc_id, subnet_ids).await.refusal
}

/// What EC2 answers about the placement a request names: whether it can be placed there at
/// all, and which network that is.
///
/// The SUBNETS are resolved first: they are where the request places the endpoint's network
/// interfaces, so a request whose subnets EC2 holds none of is answered about them rather
/// than about the VPC beside them — which, a subnet existing only inside a VPC, resolves to
/// nothing whenever they do.
///
/// The caller-sized subnet list is the one the handler has already held to its own bound, so
/// the resolving reads below are not sized by the request.
pub async fn placement(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: Option<&str>,
    subnet_ids: &[String],
) -> Placement {
    let names_a_vpc = vpc_id.is_some_and(|id| !id.is_empty());
    if !names_a_vpc && subnet_ids.is_empty() {
        return Placement::default();
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an unreachable EC2
    // gets, which grounds no refusal and places nothing.
    let Some(ec2) = ec2_client(ctx, region) else {
        return Placement::default();
    };
    if let Some(refusal) = unresolved_subnets(&ec2, subnet_ids).await {
        return Placement {
            refusal: Some(refusal),
            in_default_vpc: None,
        };
    }
    match vpc_id.filter(|id| !id.is_empty()) {
        Some(vpc_id) => resolved_vpc(&ec2, vpc_id).await,
        None => Placement::default(),
    }
}

/// What EC2 answers about the VPC `vpc_id` names: why a request cannot be placed in it, and
/// whether it is the account's default network.
///
/// The id is NAMED to EC2 (`DescribeVpcsRequest$VpcIds`) rather than filtered on, because an
/// id EC2 holds no VPC of refuses that read rather than leaving a row out — which is what
/// makes EC2 the author of the sentence the caller is answered with, about the very value the
/// request carried.
async fn resolved_vpc(ec2: &::aws_sdk_ec2::Client, vpc_id: &str) -> Placement {
    let error = match ec2.describe_vpcs().vpc_ids(vpc_id.to_string()).send().await {
        Ok(output) => {
            let reported = output
                .vpcs()
                .iter()
                .find(|vpc| vpc.vpc_id() == Some(vpc_id));
            if reported.is_none() {
                // A read EC2 ANSWERED without the row for the id it was asked about states no
                // sentence about that id, and this service words none for it — so the gate
                // refuses nothing here, which is worth a warning, because it is a deployment
                // where the gate has stopped running.
                ::tracing::warn!(
                    ?vpc_id,
                    "EC2 answered the VPC read without the VPC it was asked about, so the \
                     placement is refused on nothing"
                );
            }
            // EC2's own report of whether this is the account's default network; a read that
            // resolved the VPC without stating it states nothing about it.
            return Placement {
                refusal: None,
                in_default_vpc: reported.and_then(::aws_sdk_ec2::types::Vpc::is_default),
            };
        }
        Err(error) => error,
    };
    Placement {
        refusal: refusal_ec2_answered(&error, VPC_REFUSAL_PREFIX, "the VPC"),
        in_default_vpc: None,
    }
}

/// What EC2 answers about the subnets `subnet_ids` names: ONE read NAMING every id
/// (`DescribeSubnetsRequest$SubnetIds`), so an id EC2 holds no subnet of refuses that read
/// and EC2's own sentence about that very id is what the request is answered with.
///
/// The list is the one the handler has already held to its own maximum, so the read is not
/// sized by the request.
async fn unresolved_subnets(ec2: &::aws_sdk_ec2::Client, subnet_ids: &[String]) -> Option<String> {
    if subnet_ids.is_empty() {
        return None;
    }
    let error = match ec2
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await
    {
        // EC2 answers for an id it holds no subnet of by refusing the read rather than
        // leaving the row out, so an answer it GAVE is one it resolved every named id of. An
        // answer that somehow omits one states no sentence about it, and a sentence of this
        // service's own would be this emulator wording EC2's refusal for it — so the gate
        // refuses nothing then, at a warning, because it is a deployment where the gate has
        // stopped running.
        Ok(output) => {
            let reported: Vec<&str> = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::subnet_id)
                .collect();
            if let Some(absent) = subnet_ids
                .iter()
                .find(|id| !reported.contains(&id.as_str()))
            {
                ::tracing::warn!(
                    subnet_id = ?absent,
                    "EC2 answered the subnet read without a subnet it was asked about, so the \
                     placement is refused on nothing"
                );
            }
            return None;
        }
        Err(error) => error,
    };
    refusal_ec2_answered(&error, SUBNET_REFUSAL_PREFIX, "the subnets")
}

/// The refusal an EC2 read that FAILED grounds: EC2's own sentence about the id it refused,
/// carried through verbatim, followed by the envelope the SDK layer that relayed the call
/// states around a failed call — the service it was made to, the status EC2 answered it with,
/// that answer's own request id and the one attempt it took
/// (`service_common::relayed_refusal`, this workspace's one rendering of that envelope). The
/// request id belongs to the relayed CALL, so it is the one EC2's own answer carried, and is
/// never derived from anything in the request.
///
/// What EC2 answered refuses the request only when it says so in its own error code AND
/// states a sentence to carry through, with the status read off that very answer: an envelope
/// pairing a service's name with a status it did not give misattributes the failure.
///
/// Anything else is this emulator failing to ASK rather than EC2 reporting a bad id — there
/// is no EC2 mounted, or the answer could not be read — and nothing is what a request is
/// refused on then. It is worth a warning, because a deployment with no EC2 to ask is one
/// where this gate does not run.
fn refusal_ec2_answered<
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    refusal_prefix: &str,
    named: &str,
) -> Option<String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let service_error = error.as_service_error();
    let code = service_error
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default();
    let reason = service_error
        .and_then(ProvideErrorMetadata::message)
        .unwrap_or_default();
    let answered = error.raw_response();
    let refuses_these_ids = code.starts_with(refusal_prefix) && !reason.is_empty();
    let Some(answered) = answered.filter(|_| refuses_these_ids) else {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "{named} a request named could not be resolved against EC2"
        );
        return None;
    };
    let relayed = ::service_common::relayed_refusal::RelayedCall::once(
        EC2_RELAY_SERVICE,
        answered.status().as_u16(),
    );
    Some(format!(
        "{reason}{}",
        ::service_common::relayed_refusal::relay_envelope_answered(relayed, answered.headers())
    ))
}

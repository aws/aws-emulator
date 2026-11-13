// SPDX-License-Identifier: CC0-1.0

//! The account's subnets: a fact this service RESOLVES rather than holds.
//!
//! Every create in this API that places a resource in a client VPC takes the subnets the
//! elastic network interfaces go in (`BrokerNodeGroupInfo$ClientSubnets`,
//! `VpcConfig$SubnetIds`, `CreateVpcConnectionRequest$ClientSubnets`,
//! `KafkaClusterClientVpcConfig$SubnetIds`). A subnet is not a resource of this service:
//! it is Amazon EC2's, so what a request names is resolved against EC2, reached the way
//! any emulated service reaches another — through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate.
//!
//! Every refusal built here is grounded in what EC2 ANSWERED. An emulator with no EC2
//! mounted knows nothing about the account's subnets, and nothing is what it refuses on: a
//! refusal is never assumed from silence.
//!
//! What the CALLER is told, though, is Amazon MSK's own sentence for a placement it could
//! not resolve ([`CLIENT_SUBNETS_ABSENT_MESSAGE`]) — fixed text, with no subnet id of the
//! request and nothing of EC2's own answer in it. MSK answers for the request it was given
//! rather than reporting the call it made on the caller's behalf; the member at fault is
//! named by the refusal's `invalidParameter`, which is the handler's to pass.

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. `region` is the
/// request's own signing region.
///
/// This crate's ONE such seam, re-exported below for every module that resolves a fact
/// another service owns — the subnets here, the SCRAM secrets in
/// [`super::scram_secret_admission`] — so there is a single place that says how this
/// service calls a peer.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:expr, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url($endpoint)
            // The transport is in-process: there is nothing to retry, time out or watch
            // for a stalled stream, and each of those would need a sleep implementation
            // this client is not given. The same goes for caching the identity: the
            // credentials are handed over directly.
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

pub(crate) use callback_client;

/// How many subnets ONE create may name as a placement.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota (the model declares no
/// `length` on any of those lists): the list comes from the request, and it is both cloned
/// into the resolving call below and compared against what EC2 reports, so without a
/// ceiling one create would size that work. The clamp is applied BEFORE either, and it is
/// the same ceiling this service already puts on the subnets a serverless VPC
/// configuration may name — one answer to "how many subnets may a create name", sized well
/// above the handful of Availability Zones a cluster is really spread over.
pub const MAX_PLACEMENT_SUBNETS: usize = super::MAX_SERVERLESS_VPC_ENTRIES;

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` for an id no subnet carries, `InvalidSubnetID.Malformed` for
/// a value that is not a subnet id at all. Both say the same thing about a placement.
const SUBNET_ID_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// What Amazon MSK answers a create whose client subnets it could not resolve against the
/// account's network.
///
/// A CONSTANT of the service, not built from the request (the apostrophe is U+2019, as the
/// service writes it): the ids the request named are not echoed back, nor is the count of
/// them, nor anything of the answer EC2 gave the resolving call — one sentence for every
/// create that places a resource in a client VPC it cannot be placed in, whichever member
/// named the subnets. That member is reported in the refusal's `invalidParameter` by the
/// handler that knows it.
pub const CLIENT_SUBNETS_ABSENT_MESSAGE: &str =
    "One or more of the specified client subnets don\u{2019}t exist.";

/// Why the subnets `subnet_ids` names cannot be placed in, or `None` when they can — which
/// includes the case where this emulator has no EC2 to ask, because a refusal has to be
/// grounded in what EC2 answered rather than assumed from silence.
///
/// Two questions, in this order: whether the list is within [`MAX_PLACEMENT_SUBNETS`],
/// which is answered from the request alone and so bounds everything below it, and then
/// whether EC2 resolves every id it carries.
///
/// Amazon MSK creates the elastic network interfaces a cluster (or a VPC connection, or a
/// replicator's client) is reached through INSIDE the subnets a request names, so it
/// resolves them before it provisions anything. The subnets of the account are EC2's, so
/// EC2 is asked: it answers `InvalidSubnetID.NotFound` for an id no subnet of the account
/// carries, and an id it reports no subnet for says the same thing. Either way the caller
/// is answered MSK's own [`CLIENT_SUBNETS_ABSENT_MESSAGE`] — EC2's answer decides WHETHER
/// the create is refused, not what the refusal says.
pub async fn placement_subnet_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if subnet_ids.is_empty() {
        return None;
    }
    // The caller-sized list is clamped here, before it is cloned into the call below or
    // walked against what EC2 answered, so neither is sized by the request.
    if subnet_ids.len() > MAX_PLACEMENT_SUBNETS {
        return Some(format!(
            "A create may name at most {MAX_PLACEMENT_SUBNETS} subnets, not {}.",
            subnet_ids.len()
        ));
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an unreachable EC2
    // gets, which grounds no refusal.
    let endpoint = peer_endpoint("ec2", region)?;
    let answer = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            // EC2 reports one subnet per id it resolved, so an id it reported nothing for
            // names no subnet of the account — the same thing the refusal below says.
            let reported: Vec<&str> = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::subnet_id)
                .collect();
            if !subnet_ids.iter().all(|id| reported.contains(&id.as_str())) {
                return Some(CLIENT_SUBNETS_ABSENT_MESSAGE.to_string());
            }
            return None;
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    // Any other answer is this emulator failing to ask rather than EC2 reporting a subnet
    // the account does not have, so it grounds no refusal — and it is worth a warning,
    // because a deployment with no EC2 to ask is one where this gate does not run.
    if !code.starts_with(SUBNET_ID_REFUSAL_PREFIX) {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the subnets a request named could not be resolved against EC2"
        );
        return None;
    }
    Some(CLIENT_SUBNETS_ABSENT_MESSAGE.to_string())
}

/// The endpoint the in-process callback reaches `service` in `region` at, or `None` when
/// `region` is not a Region LABEL.
///
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends up in an
/// endpoint URL's authority rather than in a response, so it is read as a Region label
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// instead of being interpolated as given: a value carrying a host or path delimiter would
/// name an authority the caller chose rather than the peer service this emulator asks.
pub(crate) fn peer_endpoint(service: &str, region: &str) -> Option<String> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!(
            peer = %service,
            "the request names no Region label, so {service} cannot be asked"
        );
        return None;
    };
    Some(format!("https://{service}.{region}.amazonaws.com"))
}

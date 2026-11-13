// SPDX-License-Identifier: CC0-1.0

//! The account's subnets, a fact this service RESOLVES rather than holds.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. "The subnets and
//! security groups for the cluster must already exist before you use this API action"
//! (`CreateCluster`'s own documentation), and `NetworkingRequest$subnetIds` is "the list of
//! subnet IDs where PCS creates an Elastic Network Interface (ENI) to enable communication
//! between managed controllers and PCS resources" — so a create that places something in a
//! caller-named subnet resolves what it was given BEFORE it mints anything, and EC2 is the
//! one answer to "does this account hold that subnet".
//!
//! EC2 is reached the way any emulated service reaches another, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process: the callback IS the transport, so asking
//! EC2 opens no socket and resolves no host name — the endpoint URL below only tells the SDK
//! which service and Region the question is for, and the Region is read as a Region LABEL
//! first, so a caller-supplied value can never re-point that authority. Nothing is cached and
//! nothing is stored here: a subnet created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.
//!
//! Every refusal built here is grounded in what EC2 ANSWERED. A hop that could not be made
//! at all — an emulator with no EC2 mounted, an answer this emulator could not read — is
//! this service failing to ask rather than EC2 reporting a subnet the account does not have,
//! so it refuses nothing: this gate keeps nothing of the subnet it resolves, so a refusal
//! assumed from silence would cost a create that EC2 would have resolved.
//!
//! What the CALLER is told is PCS's own fixed sentence ([`SUBNETS_ABSENT_MESSAGE`]): it
//! names no subnet id of the request and carries nothing of EC2's own answer, because PCS
//! answers for the request it was given rather than reporting the call it made on the
//! caller's behalf.

use super::{service_validation, ValidationException};

/// What PCS answers a create whose subnets it could not resolve against the account's
/// network.
///
/// A CONSTANT of the service: the whole sentence is fixed text, naming neither the ids the
/// request carried nor how many of them failed to resolve — "1 or more" is the service's own
/// wording, not a count.
pub const SUBNETS_ABSENT_MESSAGE: &str =
    "AWS PCS couldn't find 1 or more of the subnets you provided.";

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` for an id no subnet carries, `InvalidSubnetID.Malformed` for a
/// value that is not a subnet id at all. Both say the same thing about a placement.
const SUBNET_ID_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 client this service asks about the subnets a request named, addressed to the
/// Region the request was signed for, or `None` when there is no Region label to ask in.
///
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends up in an
/// endpoint URL's authority rather than in a response, so it is read as a Region label
/// (`service_common::arn::region_label`, the tree's one reading of that grammar) instead of
/// being interpolated as given: a value carrying a host or path delimiter would name an
/// authority the caller chose rather than the peer this emulator asks.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — and each of those would need a sleep implementation this client is not
/// given. The same goes for caching the identity: the credentials the callback hands over
/// are used directly.
fn ec2_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let label = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(label.to_string()))
        .endpoint_url(format!("https://ec2.{label}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// The modelled `ValidationException` (HTTP 400) a request naming `subnet_ids` is refused
/// with, or `None` when the placement resolves and the call may proceed.
///
/// `Some` only where EC2 ANSWERED that an id names no subnet of this account in this Region:
/// it reports `InvalidSubnetID.*` for such an id, and an id it reported no subnet for says
/// the same thing. `None` for an empty list — there is nothing to resolve — and for every
/// answer that is not such a report, which is this emulator failing to ask (see the module
/// docs).
///
/// The caller must have bounded `subnet_ids` already: it is cloned into the call below, so an
/// unbounded list would size this work from the request. Every handler that reaches here has
/// put it through [`super::stored_list_violation`] first, because it is a list the create
/// would also PERSIST — and that ceiling is well inside the largest page EC2 answers a
/// `DescribeSubnets` that names its ids with, so the reported set below is never a partial
/// page.
pub async fn violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<ValidationException> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if subnet_ids.is_empty() {
        return None;
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an unreachable EC2
    // gives, which grounds no refusal.
    let client = ec2_client(ctx, region)?;
    let answer = client
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
            if subnet_ids.iter().all(|id| reported.contains(&id.as_str())) {
                return None;
            }
            return Some(service_validation(SUBNETS_ABSENT_MESSAGE.to_string()));
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default();
    if !code.starts_with(SUBNET_ID_REFUSAL_PREFIX) {
        // At `warn`: a deployment where this hop fails is one where this gate does not run
        // at all, which is worth seeing, and it IS why a placement was not resolved.
        ::tracing::warn!(
            %region,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the subnets a PCS create named could not be resolved against EC2"
        );
        return None;
    }
    Some(service_validation(SUBNETS_ABSENT_MESSAGE.to_string()))
}

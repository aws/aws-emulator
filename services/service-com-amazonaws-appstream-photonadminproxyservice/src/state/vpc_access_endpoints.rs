// SPDX-License-Identifier: CC0-1.0

//! THE HOP: asking the service that OWNS interface VPC endpoints what an
//! `AccessEndpoints` entry really names.
//!
//! A stack's `AccessEndpoints` entry does not describe an endpoint — it REFERENCES one, by
//! the `VpceId` of an interface VPC endpoint in the caller's account. That endpoint is an
//! Amazon EC2 resource, so the only answer to whether the account has it, and which
//! endpoint service it is an endpoint TO, is EC2's: asked here over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), the way any emulated service
//! reaches another, so this crate names no other service's crate and nothing leaves the
//! process.
//!
//! The reference is resolved wherever it is WRITTEN — `CreateStack` and the `UpdateStack`
//! that replaces it — because what the two write is the same reference: a stack pins
//! streaming to one interface endpoint, so an id the account has no endpoint for, or one
//! whose endpoint service is not the AppStream streaming service, pins it to nothing.
//! Either is refused with [`crate::state::INVALID_PARAMETER_VALUE_CODE`] and the sentence
//! for that case, before any record is written.
//!
//! What this module does NOT look at is the endpoint's own lifecycle state: an endpoint
//! still coming up is an endpoint the account has, and a stack may reference it.
//!
//! Nothing is refused where EC2 could not be asked at all (no Region label on the request,
//! or no owner of VPC endpoints mounted beside this service): "the account does not have
//! this endpoint" and "the owner of endpoints could not be asked" are two different
//! answers, and only the first is a refusal.

use crate::ops::types::AccessEndpoint;

use super::WireRefusal;

/// The endpoint-service label of AppStream's STREAMING endpoint service, the one an
/// `AccessEndpoints` entry references — the label an endpoint service name carries after
/// the Region (`appstream.streaming` in `com.amazonaws.us-west-2.appstream.streaming`).
///
/// AppStream publishes two endpoint services, the API one and the streaming one, and an
/// access endpoint is what a user's streaming session is carried over; an endpoint to any
/// other service, this service's API endpoint included, is not one for AppStream streaming.
const STREAMING_ENDPOINT_SERVICE_LABEL: &str = "appstream.streaming";

/// The endpoint service name an `AccessEndpoints` entry must name an endpoint to, in
/// `region`: `com.amazonaws.<region>.appstream.streaming`.
fn streaming_endpoint_service_name(region: &str) -> String {
    format!("com.amazonaws.{region}.{STREAMING_ENDPOINT_SERVICE_LABEL}")
}

/// The sentence AppStream refuses a `VpceId` the account has no VPC endpoint for with. The
/// fixed text is everything but the id, which is the `VpceId` the request supplied.
pub fn vpc_endpoint_not_valid_message(vpce_id: &str) -> String {
    format!(
        "The VPC endpoint {vpce_id} is not valid. Specify a valid VPC endpoint that exists \
         in your AWS account, and try again."
    )
}

/// The sentence AppStream refuses a `VpceId` that names an endpoint to some OTHER endpoint
/// service with. Entirely fixed text — it names neither the id nor the service the endpoint
/// really points at — and the two spaces after the first sentence are AWS's own.
pub const VPC_ENDPOINT_NOT_FOR_APPSTREAM_MESSAGE: &str =
    "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the \
     endpoint you specify corresponds to the AppStream 2.0 service, and try again.";

/// The `DescribeVpcEndpoints` filter that selects endpoints BY ID (`vpc-endpoint-id`,
/// listed by that operation's own `Filters` documentation).
///
/// A filter rather than `VpcEndpointIds`: an identifier the account has no endpoint for is
/// then an absent row instead of a failure of the whole call, so every identifier the
/// request named is still resolved against what EC2 does hold.
const VPC_ENDPOINT_ID_FILTER: &str = "vpc-endpoint-id";

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The endpoint URL is only the address the callback's transport routes
/// on IN PROCESS: no socket is opened, no name is resolved and nothing leaves this process.
/// The credentials are the callback's, which sign this in-process hop and never appear in a
/// response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// instead of being interpolated as given: `None` for a value that would name an authority
/// the caller chose rather than the peer service this emulator asks.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// Why `access_endpoints` may not be written to a stack — the first entry that references
/// an interface VPC endpoint the account does not have, or one that is not an endpoint to
/// AppStream's streaming service — or `None` when every entry references one it does have.
///
/// Called by a write BEFORE it takes this service's store, so no guard is held across the
/// hop. ONE hop, whose question is bounded by the model: `AccessEndpointList` is
/// `@length min 1 max 4` (`smithy_query(query="shape", shape="AccessEndpointList")`), and a
/// repeated id costs no second value in the filter. An entry carrying no `VpceId` references
/// no endpoint and so resolves to nothing to refuse.
pub async fn access_endpoints_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    access_endpoints: &[AccessEndpoint],
) -> Option<WireRefusal> {
    let mut referenced: Vec<&str> = Vec::with_capacity(access_endpoints.len());
    for endpoint in access_endpoints {
        let vpce_id = endpoint.vpce_id.as_deref().unwrap_or_default();
        if !vpce_id.is_empty() && !referenced.contains(&vpce_id) {
            referenced.push(vpce_id);
        }
    }
    if referenced.is_empty() {
        return None;
    }

    let client = ec2(ctx, region)?;
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ENDPOINT_ID_FILTER)
        .set_values(Some(referenced.iter().map(|id| (*id).to_string()).collect()))
        .build();
    let described = match client.describe_vpc_endpoints().filters(by_id).send().await {
        Ok(described) => described,
        Err(error) => {
            // At `warn`: a stack whose access endpoints were never resolved against the
            // owner of VPC endpoints is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the interface VPC endpoints a stack references could not be resolved \
                 against EC2"
            );
            return None;
        }
    };

    // What the account holds, as EC2 reports it: the endpoint service each referenced id is
    // an endpoint TO. An id absent from this list is one the account has no endpoint for —
    // which is a different refusal from an endpoint to the wrong service.
    let required = streaming_endpoint_service_name(region);
    for vpce_id in referenced {
        let Some(endpoint) = described
            .vpc_endpoints()
            .iter()
            .find(|endpoint| endpoint.vpc_endpoint_id() == Some(vpce_id))
        else {
            return Some(WireRefusal::invalid_parameter_value(
                vpc_endpoint_not_valid_message(vpce_id),
            ));
        };
        if endpoint.service_name() != Some(required.as_str()) {
            return Some(WireRefusal::invalid_parameter_value(
                VPC_ENDPOINT_NOT_FOR_APPSTREAM_MESSAGE,
            ));
        }
    }
    None
}

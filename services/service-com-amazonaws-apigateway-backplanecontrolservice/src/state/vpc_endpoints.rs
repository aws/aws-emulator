// SPDX-License-Identifier: CC0-1.0

//! The VPC endpoints the ACCOUNT has, as the service that OWNS them reports them, for the
//! one member of this service that names one: the `accessAssociationSource` of a domain
//! name access association ("The identifier of the domain name access association source.
//! For a VPCE, the value is the VPC endpoint ID",
//! `CreateDomainNameAccessAssociation$accessAssociationSource`).
//!
//! A VPC endpoint is not a resource of this service: it is Amazon EC2's. A client builds
//! one with `ec2:CreateVpcEndpoint` and then names it when it binds that endpoint to a
//! private custom domain name, so EC2 is the ONE answer to "does the account have an
//! endpoint under this id", and an identifier a request names is resolved there and
//! nowhere else — reached the way any emulated service reaches another, through the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so this
//! crate names no other service's crate, no socket is opened and nothing leaves the
//! process.
//!
//! This service keeps NOTHING of an endpoint — a `DomainNameAccessAssociation` reports its
//! source identifier and nothing else about it — so this only GATES on what EC2 answered: a
//! hop that could not be made at all is evidence of nothing, so it refuses nothing and says
//! so at `warn`, and the refusal that IS served is the one EC2's answer grounds. Nothing is
//! cached: an endpoint created a moment ago resolves at once, and one that was deleted stops
//! resolving at once.
//!
//! The per-crate client below is this tree's convention for that hop — the sibling
//! `service-com-amazonaws-apigatewayv2-apigatewayv2` builds its own to resolve a VPC link's
//! subnets, as do `fsx`, `kafka`, `eks` and `opensearch` — because each service asks a
//! different question and takes its own reading of silence. What is shared is already
//! shared: the Region-label grammar ([`::service_common::arn::region_label`]).

/// The `DescribeVpcEndpoints` filter that selects endpoints BY ID (`vpc-endpoint-id`,
/// listed by that operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `VpcEndpointIds`: an identifier the
/// account has no endpoint for is then an absent row instead of a failure of the whole
/// call, so the answer distinguishes "no such endpoint" from "EC2 could not be asked".
const VPC_ENDPOINT_ID_FILTER: &str = "vpc-endpoint-id";

/// An AWS SDK client for Amazon EC2 over the in-process callback into this same emulator.
///
/// This crate's ONE client for that hop: the access association source below is resolved
/// through it, and so is the VPC endpoint a private integration's own connection reaches
/// ([`crate::service::invoke`]), so both ask EC2 the same way.
///
/// `region` is the Region the caller signed for, so the question is asked in the Region the
/// request addressed. The endpoint URL is only the address the callback's transport routes
/// on IN PROCESS. The credentials are the callback's, which sign this in-process hop and
/// never appear in a response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it would end up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// ([`::service_common::arn::region_label`], the one place this tree states that grammar)
/// instead of being interpolated as given: `None` for a value that would name an authority
/// the caller chose rather than the peer service this emulator asks.
pub(crate) fn ec2(
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
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// What the owner of VPC endpoints answered about the identifier one request named.
enum Answer {
    /// EC2 could not be asked at all — evidence of nothing.
    NotAsked,
    /// Whether EC2 holds an endpoint of the account under the identifier asked about.
    Holds(bool),
}

/// Ask EC2 whether the account has a VPC endpoint under `vpc_endpoint_id` in `region`.
async fn account_holds_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_endpoint_id: &str,
) -> Answer {
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return Answer::NotAsked;
    };
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ENDPOINT_ID_FILTER)
        .values(vpc_endpoint_id)
        .build();
    match client.describe_vpc_endpoints().filters(by_id).send().await {
        Ok(output) => Answer::Holds(
            output
                .vpc_endpoints()
                .iter()
                .any(|endpoint| endpoint.vpc_endpoint_id() == Some(vpc_endpoint_id)),
        ),
        Err(error) => {
            // At `warn`: this gate refuses nothing on silence, so an emulator where the hop
            // cannot be made admits an association whose source names no endpoint, and that
            // is worth seeing.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the access association source could not be resolved against EC2"
            );
            Answer::NotAsked
        }
    }
}

/// Resolve the `accessAssociationSource` of a `VPCE` domain name access association against
/// the VPC endpoints the account has in `region`.
///
/// `Ok(())` for an identifier the account HAS an endpoint under — and for one EC2 could not
/// be asked about at all, which is evidence of nothing (see this module's documentation).
/// `Err` carries the message for the operation's `BadRequestException`
/// ([`super::invalid_access_association_source`]), so an identifier naming no endpoint of
/// the account is refused before anything is stored.
pub async fn resolve_access_association_source(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    access_association_source: &str,
) -> Result<(), String> {
    match account_holds_endpoint(ctx, region, access_association_source).await {
        Answer::NotAsked | Answer::Holds(true) => Ok(()),
        Answer::Holds(false) => Err(super::invalid_access_association_source(
            access_association_source,
        )),
    }
}

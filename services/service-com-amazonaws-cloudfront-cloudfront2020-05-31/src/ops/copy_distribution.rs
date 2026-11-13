// SPDX-License-Identifier: CC0-1.0

//! @op CopyDistribution
//!
//! Creates a STAGING distribution from the stored configuration of the primary
//! distribution the path names. Everything the new distribution runs under is read from
//! the primary's persisted record; the request supplies only the three members the model
//! lets it supply — the new distribution's `CallerReference`, whether it is `Enabled`,
//! and that the copy is a staging distribution — so nothing else about the copy is
//! derived from the request that created it.
//!
//! The create itself is the SHARED create path every distribution create in this service
//! runs ([`crate::state::ComAmazonawsCloudfrontCloudfront20200531State::create_distribution`]),
//! so the copy gets its own id, domain name and version token, starts `InProgress` with
//! the propagation the clock owes it, and takes part in the caller-reference rule and the
//! store bound exactly as `CreateDistribution` does.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::service_state`].
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CopyDistribution`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json")
//! ```
//!
//! Creates a staging distribution using the configuration of the provided primary
//! distribution. A staging distribution is a copy of an existing distribution (called the
//! primary distribution) that you can use in a continuous deployment workflow. After you
//! create a staging distribution, you can use UpdateDistribution to modify the staging
//! distribution's configuration. Then you can use CreateContinuousDeploymentPolicy to
//! incrementally move traffic to the staging distribution. This API operation requires the
//! following IAM permissions: GetDistribution CreateDistribution CopyDistribution
//!
//! ## Input
//!
//! * `CallerReference` (string) **required** — A value that uniquely identifies a request to
//!   create a resource. This helps to prevent CloudFront from creating a duplicate resource
//!   if you accidentally resubmit an identical request.
//! * `Enabled` (boolean) — A Boolean flag to specify the state of the staging distribution
//!   when it's created. When you set this value to True, the staging distribution is enabled.
//!   When you set this value to False, the staging distribution is disabled. If you omit this
//!   field, the default value is True.
//! * `IfMatch` (string) — The version identifier of the primary distribution whose
//!   configuration you are copying. This is the ETag value returned in the response to
//!   GetDistribution and GetDistributionConfig.
//! * `PrimaryDistributionId` (string) **required** — The identifier of the primary
//!   distribution whose configuration you are copying. To get a distribution ID, use
//!   ListDistributions.
//! * `Staging` (boolean) — The type of distribution that your primary distribution will be
//!   copied to. The only valid value is True, indicating that you are copying to a staging
//!   distribution.
//!
//! ## Output
//!
//! * `Distribution` (Distribution) — A distribution tells CloudFront where you want content
//!   to be delivered from, and the details about how to track and manage content delivery.
//! * `ETag` (string) — The version identifier for the current version of the staging
//!   distribution.
//! * `Location` (string) — The URL of the staging distribution.
//!
//! ## Errors
//!
//! * `AccessDenied` (HTTP 403) — Access denied.
//! * `CNAMEAlreadyExists` (HTTP 409) — The CNAME specified is already defined for CloudFront.
//! * `DistributionAlreadyExists` (HTTP 409) — The caller reference you attempted to create
//!   the distribution with is associated with another distribution.
//! * `IllegalFieldLevelEncryptionConfigAssociationWithCacheBehavior` (HTTP 400) — The
//!   specified configuration for field-level encryption can't be associated with the
//!   specified cache behavior.
//! * `InconsistentQuantities` (HTTP 400) — The value of Quantity and the size of Items don't
//!   match.
//! * `InvalidArgument` (HTTP 400) — An argument is invalid.
//! * `InvalidDefaultRootObject` (HTTP 400) — The default root object file name is too big or
//!   contains an invalid character.
//! * `InvalidErrorCode` (HTTP 400) — An invalid error code was specified.
//! * `InvalidForwardCookies` (HTTP 400) — Your request contains forward cookies option which
//!   doesn't match with the expectation for the whitelisted list of cookie names. Either list
//!   of cookie names has been specified when not allowed or list of cookie names is missing
//!   when expected.
//! * `InvalidFunctionAssociation` (HTTP 400) — A CloudFront function association is invalid.
//! * `InvalidGeoRestrictionParameter` (HTTP 400) — The specified geo restriction parameter is
//!   not valid.
//! * `InvalidHeadersForS3Origin` (HTTP 400) — The headers specified are not valid for an
//!   Amazon S3 origin.
//! * `InvalidIfMatchVersion` (HTTP 400) — The If-Match version is missing or not valid.
//! * `InvalidLambdaFunctionAssociation` (HTTP 400) — The specified Lambda@Edge function
//!   association is invalid.
//! * `InvalidLocationCode` (HTTP 400) — The location code specified is not valid.
//! * `InvalidMinimumProtocolVersion` (HTTP 400) — The minimum protocol version specified is
//!   not valid.
//! * `InvalidOrigin` (HTTP 400) — The Amazon S3 origin server specified does not refer to a
//!   valid Amazon S3 bucket.
//! * `InvalidOriginAccessControl` (HTTP 400) — The origin access control is not valid.
//! * `InvalidOriginAccessIdentity` (HTTP 400) — The origin access identity is not valid or
//!   doesn't exist.
//! * `InvalidOriginKeepaliveTimeout` (HTTP 400) — The keep alive timeout specified for the
//!   origin is not valid.
//! * `InvalidOriginReadTimeout` (HTTP 400) — The read timeout specified for the origin is not
//!   valid.
//! * `InvalidProtocolSettings` (HTTP 400) — You cannot specify SSLv3 as the minimum protocol
//!   version if you only want to support only clients that support Server Name Indication
//!   (SNI).
//! * `InvalidQueryStringParameters` (HTTP 400) — The query string parameters specified are
//!   not valid.
//! * `InvalidRelativePath` (HTTP 400) — The relative path is too big, is not URL-encoded, or
//!   does not begin with a slash (/).
//! * `InvalidRequiredProtocol` (HTTP 400) — This operation requires the HTTPS protocol.
//!   Ensure that you specify the HTTPS protocol in your request, or omit the
//!   RequiredProtocols element from your distribution configuration.
//! * `InvalidResponseCode` (HTTP 400) — A response code is not valid.
//! * `InvalidTTLOrder` (HTTP 400) — The TTL order specified is not valid.
//! * `InvalidViewerCertificate` (HTTP 400) — A viewer certificate specified is not valid.
//! * `InvalidWebACLId` (HTTP 400) — A web ACL ID specified is not valid. To specify a web ACL
//!   created using the latest version of WAF, use the ACL ARN, for example
//!   arn:aws:wafv2:us-east-1:123456789012:global/webacl/ExampleWebACL/473e64fd-f30b-4765-81a0-62ad96dd167a.
//!   To specify a web ACL created using WAF Classic, use the ACL ID, for example
//!   473e64fd-f30b-4765-81a0-62ad96dd167a.
//! * `MissingBody` (HTTP 400) — This operation requires a body. Ensure that the body is
//!   present and the Content-Type header is set.
//! * `NoSuchCachePolicy` (HTTP 404) — The cache policy does not exist.
//! * `NoSuchDistribution` (HTTP 404) — The specified distribution does not exist.
//! * `NoSuchFieldLevelEncryptionConfig` (HTTP 404) — The specified configuration for
//!   field-level encryption doesn't exist.
//! * `NoSuchOrigin` (HTTP 404) — No origin exists with the specified Origin Id.
//! * `NoSuchOriginRequestPolicy` (HTTP 404) — The origin request policy does not exist.
//! * `NoSuchRealtimeLogConfig` (HTTP 404) — The real-time log configuration does not exist.
//! * `NoSuchResponseHeadersPolicy` (HTTP 404) — The response headers policy does not exist.
//! * `PreconditionFailed` (HTTP 412) — The precondition in one or more of the request fields
//!   evaluated to false.
//! * `RealtimeLogConfigOwnerMismatch` (HTTP 401) — The specified real-time log configuration
//!   belongs to a different Amazon Web Services account.
//! * `TooManyCacheBehaviors` (HTTP 400) — You cannot create more cache behaviors for the
//!   distribution.
//! * `TooManyCertificates` (HTTP 400) — You cannot create anymore custom SSL/TLS
//!   certificates.
//! * `TooManyCookieNamesInWhiteList` (HTTP 400) — Your request contains more cookie names in
//!   the whitelist than are allowed per cache behavior.
//! * `TooManyDistributionCNAMEs` (HTTP 400) — Your request contains more CNAMEs than are
//!   allowed per distribution.
//! * `TooManyDistributions` (HTTP 400) — Processing your request would cause you to exceed
//!   the maximum number of distributions allowed.
//! * `TooManyDistributionsAssociatedToCachePolicy` (HTTP 400) — The maximum number of
//!   distributions have been associated with the specified cache policy. For more
//!   information, see Quotas (formerly known as limits) in the Amazon CloudFront Developer
//!   Guide.
//! * `TooManyDistributionsAssociatedToFieldLevelEncryptionConfig` (HTTP 400) — The maximum
//!   number of distributions have been associated with the specified configuration for
//!   field-level encryption.
//! * `TooManyDistributionsAssociatedToKeyGroup` (HTTP 400) — The number of distributions that
//!   reference this key group is more than the maximum allowed. For more information, see
//!   Quotas (formerly known as limits) in the Amazon CloudFront Developer Guide.
//! * `TooManyDistributionsAssociatedToOriginAccessControl` (HTTP 400) — The maximum number of
//!   distributions have been associated with the specified origin access control. For more
//!   information, see Quotas (formerly known as limits) in the Amazon CloudFront Developer
//!   Guide.
//! * `TooManyDistributionsAssociatedToOriginRequestPolicy` (HTTP 400) — The maximum number of
//!   distributions have been associated with the specified origin request policy. For more
//!   information, see Quotas (formerly known as limits) in the Amazon CloudFront Developer
//!   Guide.
//! * `TooManyDistributionsAssociatedToResponseHeadersPolicy` (HTTP 400) — The maximum number
//!   of distributions have been associated with the specified response headers policy. For
//!   more information, see Quotas (formerly known as limits) in the Amazon CloudFront
//!   Developer Guide.
//! * `TooManyDistributionsWithFunctionAssociations` (HTTP 400) — You have reached the maximum
//!   number of distributions that are associated with a CloudFront function. For more
//!   information, see Quotas (formerly known as limits) in the Amazon CloudFront Developer
//!   Guide.
//! * `TooManyDistributionsWithLambdaAssociations` (HTTP 400) — Processing your request would
//!   cause the maximum number of distributions with Lambda@Edge function associations per
//!   owner to be exceeded.
//! * `TooManyDistributionsWithSingleFunctionARN` (HTTP 400) — The maximum number of
//!   distributions have been associated with the specified Lambda@Edge function.
//! * `TooManyFunctionAssociations` (HTTP 400) — You have reached the maximum number of
//!   CloudFront function associations for this distribution. For more information, see Quotas
//!   (formerly known as limits) in the Amazon CloudFront Developer Guide.
//! * `TooManyHeadersInForwardedValues` (HTTP 400) — Your request contains too many headers in
//!   forwarded values.
//! * `TooManyKeyGroupsAssociatedToDistribution` (HTTP 400) — The number of key groups
//!   referenced by this distribution is more than the maximum allowed. For more information,
//!   see Quotas (formerly known as limits) in the Amazon CloudFront Developer Guide.
//! * `TooManyLambdaFunctionAssociations` (HTTP 400) — Your request contains more Lambda@Edge
//!   function associations than are allowed per distribution.
//! * `TooManyOriginCustomHeaders` (HTTP 400) — Your request contains too many origin custom
//!   headers.
//! * `TooManyOriginGroupsPerDistribution` (HTTP 400) — Processing your request would cause
//!   you to exceed the maximum number of origin groups allowed.
//! * `TooManyOrigins` (HTTP 400) — You cannot create more origins for the distribution.
//! * `TooManyQueryStringParameters` (HTTP 400) — Your request contains too many query string
//!   parameters.
//! * `TooManyTrustedSigners` (HTTP 400) — Your request contains more trusted signers than are
//!   allowed per distribution.
//! * `TrustedKeyGroupDoesNotExist` (HTTP 400) — The specified key group does not exist.
//! * `TrustedSignerDoesNotExist` (HTTP 400) — One or more of your trusted signers don't
//!   exist.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::IfMatch;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CopyDistributionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CopyDistributionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudfront#CopyDistribution")]
#[derive(Debug)]
pub enum CopyDistributionErrorModeled {
    AccessDenied(AccessDenied),
    CNAMEAlreadyExists(CNAMEAlreadyExists),
    DistributionAlreadyExists(DistributionAlreadyExists),
    IllegalFieldLevelEncryptionConfigAssociationWithCacheBehavior(IllegalFieldLevelEncryptionConfigAssociationWithCacheBehavior),
    InconsistentQuantities(InconsistentQuantities),
    InvalidArgument(InvalidArgument),
    InvalidDefaultRootObject(InvalidDefaultRootObject),
    InvalidErrorCode(InvalidErrorCode),
    InvalidForwardCookies(InvalidForwardCookies),
    InvalidFunctionAssociation(InvalidFunctionAssociation),
    InvalidGeoRestrictionParameter(InvalidGeoRestrictionParameter),
    InvalidHeadersForS3Origin(InvalidHeadersForS3Origin),
    InvalidIfMatchVersion(InvalidIfMatchVersion),
    InvalidLambdaFunctionAssociation(InvalidLambdaFunctionAssociation),
    InvalidLocationCode(InvalidLocationCode),
    InvalidMinimumProtocolVersion(InvalidMinimumProtocolVersion),
    InvalidOrigin(InvalidOrigin),
    InvalidOriginAccessControl(InvalidOriginAccessControl),
    InvalidOriginAccessIdentity(InvalidOriginAccessIdentity),
    InvalidOriginKeepaliveTimeout(InvalidOriginKeepaliveTimeout),
    InvalidOriginReadTimeout(InvalidOriginReadTimeout),
    InvalidProtocolSettings(InvalidProtocolSettings),
    InvalidQueryStringParameters(InvalidQueryStringParameters),
    InvalidRelativePath(InvalidRelativePath),
    InvalidRequiredProtocol(InvalidRequiredProtocol),
    InvalidResponseCode(InvalidResponseCode),
    InvalidTTLOrder(InvalidTTLOrder),
    InvalidViewerCertificate(InvalidViewerCertificate),
    InvalidWebACLId(InvalidWebACLId),
    MissingBody(MissingBody),
    NoSuchCachePolicy(NoSuchCachePolicy),
    NoSuchDistribution(NoSuchDistribution),
    NoSuchFieldLevelEncryptionConfig(NoSuchFieldLevelEncryptionConfig),
    NoSuchOrigin(NoSuchOrigin),
    NoSuchOriginRequestPolicy(NoSuchOriginRequestPolicy),
    NoSuchRealtimeLogConfig(NoSuchRealtimeLogConfig),
    NoSuchResponseHeadersPolicy(NoSuchResponseHeadersPolicy),
    PreconditionFailed(PreconditionFailed),
    RealtimeLogConfigOwnerMismatch(RealtimeLogConfigOwnerMismatch),
    TooManyCacheBehaviors(TooManyCacheBehaviors),
    TooManyCertificates(TooManyCertificates),
    TooManyCookieNamesInWhiteList(TooManyCookieNamesInWhiteList),
    TooManyDistributionCNAMEs(TooManyDistributionCNAMEs),
    TooManyDistributions(TooManyDistributions),
    TooManyDistributionsAssociatedToCachePolicy(TooManyDistributionsAssociatedToCachePolicy),
    TooManyDistributionsAssociatedToFieldLevelEncryptionConfig(TooManyDistributionsAssociatedToFieldLevelEncryptionConfig),
    TooManyDistributionsAssociatedToKeyGroup(TooManyDistributionsAssociatedToKeyGroup),
    TooManyDistributionsAssociatedToOriginAccessControl(TooManyDistributionsAssociatedToOriginAccessControl),
    TooManyDistributionsAssociatedToOriginRequestPolicy(TooManyDistributionsAssociatedToOriginRequestPolicy),
    TooManyDistributionsAssociatedToResponseHeadersPolicy(TooManyDistributionsAssociatedToResponseHeadersPolicy),
    TooManyDistributionsWithFunctionAssociations(TooManyDistributionsWithFunctionAssociations),
    TooManyDistributionsWithLambdaAssociations(TooManyDistributionsWithLambdaAssociations),
    TooManyDistributionsWithSingleFunctionARN(TooManyDistributionsWithSingleFunctionARN),
    TooManyFunctionAssociations(TooManyFunctionAssociations),
    TooManyHeadersInForwardedValues(TooManyHeadersInForwardedValues),
    TooManyKeyGroupsAssociatedToDistribution(TooManyKeyGroupsAssociatedToDistribution),
    TooManyLambdaFunctionAssociations(TooManyLambdaFunctionAssociations),
    TooManyOriginCustomHeaders(TooManyOriginCustomHeaders),
    TooManyOriginGroupsPerDistribution(TooManyOriginGroupsPerDistribution),
    TooManyOrigins(TooManyOrigins),
    TooManyQueryStringParameters(TooManyQueryStringParameters),
    TooManyTrustedSigners(TooManyTrustedSigners),
    TrustedKeyGroupDoesNotExist(TrustedKeyGroupDoesNotExist),
    TrustedSignerDoesNotExist(TrustedSignerDoesNotExist),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::copy_distribution::CopyDistributionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CopyDistributionError = crate::scaffolded::Scaffolded<CopyDistributionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CopyDistributionErrorModeled> for CopyDistributionError {
    fn from(modeled: CopyDistributionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `Enabled` on the staging distribution when the request leaves the member out: "If you
/// omit this field, the default value is `True`" (`CopyDistributionRequest$Enabled`).
const DEFAULT_ENABLED: bool = true;

/// Why a `Staging` value other than `true` is refused: `CopyDistributionRequest$Staging`
/// is "the type of distribution that your primary distribution will be copied to. The
/// only valid value is `True`, indicating that you are copying to a staging
/// distribution." The member is a plain `boolean` in the model, so the one value outside
/// that documented domain reaches this handler and is refused here as the invalid
/// argument it is.
const STAGING_MUST_BE_TRUE: &str =
    "The only valid value for Staging is true: CopyDistribution copies a primary \
     distribution to a staging distribution.";

/// Report a refusal from the shared create path as this operation's modelled error.
///
/// The copy is created through the same path `CreateDistribution` uses, so it is refused
/// for the same reasons; the model declares every one of them for this operation too.
fn create_refusal(refusal: crate::state::CreateRefusal) -> CopyDistributionError {
    match refusal {
        crate::state::CreateRefusal::InconsistentQuantities(message) => {
            CopyDistributionErrorModeled::InconsistentQuantities(InconsistentQuantities {
                message: Some(message),
            })
        }
        crate::state::CreateRefusal::ConfigLimit(crate::state::ConfigLimit::CacheBehaviors(
            message,
        )) => CopyDistributionErrorModeled::TooManyCacheBehaviors(TooManyCacheBehaviors {
            message: Some(message),
        }),
        crate::state::CreateRefusal::ConfigLimit(crate::state::ConfigLimit::TrustedSigners(
            message,
        )) => CopyDistributionErrorModeled::TooManyTrustedSigners(TooManyTrustedSigners {
            message: Some(message),
        }),
        crate::state::CreateRefusal::ConfigLimit(crate::state::ConfigLimit::TrustedKeyGroups(
            message,
        )) => CopyDistributionErrorModeled::TooManyKeyGroupsAssociatedToDistribution(
            TooManyKeyGroupsAssociatedToDistribution {
                message: Some(message),
            },
        ),
        crate::state::CreateRefusal::DistributionAlreadyExists(message) => {
            CopyDistributionErrorModeled::DistributionAlreadyExists(DistributionAlreadyExists {
                message: Some(message),
            })
        }
        crate::state::CreateRefusal::TooManyDistributions(message) => {
            CopyDistributionErrorModeled::TooManyDistributions(TooManyDistributions {
                message: Some(message),
            })
        }
        crate::state::CreateRefusal::InvalidViewerCertificate(message) => {
            CopyDistributionErrorModeled::InvalidViewerCertificate(InvalidViewerCertificate {
                message: Some(message),
            })
        }
        crate::state::CreateRefusal::InvalidOriginAccessControl(message) => {
            CopyDistributionErrorModeled::InvalidOriginAccessControl(InvalidOriginAccessControl {
                message: Some(message),
            })
        }
        crate::state::CreateRefusal::InvalidLambdaFunctionAssociation(message) => {
            CopyDistributionErrorModeled::InvalidLambdaFunctionAssociation(
                InvalidLambdaFunctionAssociation {
                    message: Some(message),
                },
            )
        }
        // A copy takes the configuration of a STORED distribution, and the create and the
        // update refuse an origin that names both an origin access control and an origin
        // access identity, so no stored configuration carries that combination and this
        // arm cannot be reached from a copy. The model declares no mutual-exclusion error
        // for this operation, so were it ever reached the refusal is reported as the
        // invalid argument it is, under the same sentence and the same 400.
        crate::state::CreateRefusal::IllegalOriginAccessConfiguration(message) => {
            CopyDistributionErrorModeled::InvalidArgument(InvalidArgument {
                message: Some(message),
            })
        }
        // Likewise unreachable from a copy, and for the same reason: the create and the
        // update refuse a function association naming an origin-facing event type, so no
        // stored configuration carries one.
        crate::state::CreateRefusal::InvalidArgument(message) => {
            CopyDistributionErrorModeled::InvalidArgument(InvalidArgument {
                message: Some(message),
            })
        }
    }
    .into()
}

/// The `CopyDistribution` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CopyDistribution<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CopyDistribution<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CopyDistribution<Eng>
{
    type Input = CopyDistributionInput;
    type Output = CopyDistributionOutput;
    type Error = CopyDistributionError;

    const NAME: &'static str = "CopyDistribution";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::COPY_DISTRIBUTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::COPY_DISTRIBUTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(crate::state::store_unavailable());
        };
        let now = SystemTime::now();
        // The listener the copy's edge is served on, opened BEFORE the store lock is taken:
        // binding a socket is asynchronous and a lock held across an await would make this
        // future non-`Send`. The handle is a local this function owns, so every path out of
        // here that does not store a distribution — each refusal below, and the create's own
        // — DROPS it, and dropping it closes the port ([`crate::state::EdgeEndpoint`]); the
        // bound on how many distributions this emulator holds is read first, so a create it
        // refuses for capacity opens nothing at all. A copy is always a STANDARD
        // distribution — it is created `Staging`, a member only a standard distribution has
        // — so it always has a domain name of its own to publish.
        if state.lock().at_distribution_capacity() {
            return Err(create_refusal(crate::state::too_many_distributions()));
        }
        let edge_endpoint =
            crate::state::open_edge_endpoint(&*self.engine, ctx.callback().clone()).await;
        let mut store = state.lock();
        store.settle(now);

        // The configuration being copied is the PRIMARY's stored one — "the
        // configuration of the provided primary distribution" — so a primary the store
        // does not hold is the modelled 404 and nothing is minted for it.
        let Some(primary) = store
            .distribution(&input.primary_distribution_id)
            .map(|stored| (stored.etag.clone(), stored.config.clone()))
        else {
            return Err(CopyDistributionErrorModeled::NoSuchDistribution(
                crate::state::no_such_distribution(),
            )
            .into());
        };
        let (primary_etag, primary_config) = primary;

        // `If-Match` is "the version identifier of the primary distribution whose
        // configuration you are copying", and the model leaves the header OPTIONAL — this
        // call modifies nothing about the primary, so a request that states no version
        // states no precondition and copies the configuration as it now stands. A header
        // that IS present is checked against the stored version, so a copy taken under a
        // version is taken only from the configuration its caller actually read.
        if input.if_match.is_some() {
            match crate::state::check_if_match(&primary_etag, input.if_match.as_deref()) {
                IfMatch::Current => {}
                IfMatch::Invalid => {
                    // The refusal wording is `crate::state`'s: the sentence is a property
                    // of the header the request carried, so every operation that
                    // evaluates an `If-Match` answers with the same service text rather
                    // than wording of its own.
                    return Err(CopyDistributionErrorModeled::InvalidIfMatchVersion(
                        crate::state::invalid_if_match_version(),
                    )
                    .into());
                }
                IfMatch::Stale => {
                    return Err(CopyDistributionErrorModeled::PreconditionFailed(
                        crate::state::precondition_failed(),
                    )
                    .into());
                }
            }
        }

        // What this operation creates is a STAGING distribution, and the request's
        // `Staging` member says so: its only documented value is `true`, so any other one
        // is refused rather than silently creating something else.
        if input.staging == Some(false) {
            return Err(CopyDistributionErrorModeled::InvalidArgument(InvalidArgument {
                message: Some(STAGING_MUST_BE_TRUE.to_string()),
            })
            .into());
        }

        // The copy's configuration: the primary's, with exactly the three members this
        // request speaks about applied to it. Every other member — the origins, the cache
        // behaviours, the aliases, the certificate, the restrictions — is the primary's,
        // because the copy is "a copy of an existing distribution" rather than a
        // configuration the request describes.
        let mut config = primary_config;
        // "A value that uniquely identifies a request to create a resource": the copy is a
        // new resource, so it is created under the reference THIS request names rather
        // than under the primary's.
        config.caller_reference = input.caller_reference;
        // "A Boolean flag to specify the state of the staging distribution when it's
        // created."
        config.enabled = input.enabled.unwrap_or(DEFAULT_ENABLED);
        // "A Boolean that indicates whether this is a staging distribution. When this
        // value is `true`, this is a staging distribution."
        config.staging = Some(true);

        // The shared create path: the copy is a distribution like any other, so it is
        // created, bounded and indexed by caller reference exactly as `CreateDistribution`
        // creates one — and, like that operation, this request carries no tag set, so the
        // copy starts with no tags.
        let created = store
            .create_distribution(config, crate::state::TagSet::new(), now, edge_endpoint)
            .map_err(create_refusal)?;

        Ok(CopyDistributionOutput {
            distribution: Some(created.distribution),
            // "The URL of the staging distribution."
            location: Some(crate::state::location_uri(
                ctx,
                &crate::state::distribution_path(&created.id),
            )),
            // "The version identifier for the current version of the staging
            // distribution."
            e_tag: Some(created.etag),
        })
    }
}

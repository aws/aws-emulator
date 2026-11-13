// SPDX-License-Identifier: CC0-1.0

//! `AWS::CloudFront::Distribution` and `AWS::CloudFront::OriginAccessControl`: the
//! distribution a stack declares, and the origin access control its origins reach a private
//! bucket with, created in Amazon CloudFront itself.
//!
//! This is the per-type half of provisioning for two resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::iot_authorizer`] for the reason that module records —
//! the catalogue file is far past the length a change to this repository may touch — and is
//! reached through the same seam ([`super::Owner`]).
//!
//! ## Why the distribution is really created
//!
//! A distribution's identity is the `E…` id CloudFront MINTS for it: `GetDistribution`,
//! `UpdateDistribution`, `CreateInvalidation`, `AssociateDistributionWebACL` and a
//! distribution tenant's `DistributionId` all address one by that id, and "Ref returns the
//! distribution ID". So a stack that reported a distribution created while nothing created
//! it publishes an id that resolves to nothing — the first `GetDistribution` on a stack
//! output answers `NoSuchDistribution`, blaming the client for an id the deployment itself
//! produced, and nothing downstream (the origin access control's wiring, a tenant of the
//! distribution, an invalidation) can run at all. Creating it where it belongs is what makes
//! the id the stack publishes one CloudFront really holds.
//!
//! ## The properties, and the two spellings of one configuration
//!
//! `AWS::CloudFront::Distribution` publishes `DistributionConfig` and `Tags`, and
//! `AWS::CloudFront::OriginAccessControl` publishes `OriginAccessControlConfig` — each the
//! very structure its create takes as the request's payload. What differs between the
//! template's spelling of a configuration and the API's is mechanical, and
//! [`distribution_config_members`] is where that correspondence lives:
//!
//! * where CloudFront's model wraps a list in a `{Quantity, Items}` container (`Aliases`,
//!   `Origins`, `CacheBehaviors`, `Headers`, … — [`WRAPPED_LISTS`]), the template declares
//!   the LIST alone, so each declared list is re-wrapped into that container with the element
//!   name the model's own list member carries (`AliasList`'s member is `CNAME`,
//!   `OriginList`'s is `Origin`), and `Quantity` is the length of the list the template
//!   wrote;
//! * four members the two sides spell differently are renamed ([`RENAMED_MEMBERS`]);
//! * `CachedMethods` is a sibling of `AllowedMethods` in a template and a MEMBER of it in
//!   the API, so it is nested where the API takes it;
//! * a template's `Logging` carries no `Enabled` member at all — declaring it IS enabling
//!   logging — so the `Enabled` the API requires is written from whether the declaration
//!   names a bucket;
//! * `CallerReference` is required by the API and published by neither type, so
//!   CloudFormation supplies one ([`caller_reference`]).
//!
//! Everything else is passed through under the name the template wrote, which is the name
//! the API takes. WHICH names are judged here is exactly two levels: a property the TYPE does
//! not publish ([`DISTRIBUTION_PROPERTIES`], [`ORIGIN_ACCESS_CONTROL_PROPERTIES`]) and a
//! member the CONFIGURATION itself does not publish ([`DISTRIBUTION_CONFIG_MEMBERS`],
//! [`ORIGIN_ACCESS_CONTROL_CONFIG_MEMBERS`]) are REFUSED rather than dropped, because those
//! are the two levels where a template and the API part company at all — a distribution
//! reported created with less than its declaration asked for is the shallow success this
//! module exists to avoid. Below them the two vocabularies are the same, so a member is
//! rendered as the element it names and the OWNING service's own deserialization is what
//! judges it; what is checked here for every level is that the name is a name the API could
//! have ([`refuse_unrenderable_members`]).
//!
//! ## Deleting one
//!
//! "You must disable the distribution before you can delete it", and the disabled
//! configuration has to have reached the edge locations first. So the delete reads the
//! configuration CloudFront holds now, submits it back with `Enabled` false, and asks for
//! the deletion; while that service still reports the distribution `InProgress` the deletion
//! is not finished, which is what [`removal`] waits out — the pattern
//! [`super::super::provisioner::Provisioner::removal`] documents for a delete its owning
//! service completes in its own time.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    call_rest_xml, percent_encoded, xml_leaf, xml_scalar_children, Created, Readiness, Refusal,
    Removal,
};
use crate::state::provisioner::tags_of;
use crate::state::{scalar_text, StoredResource};

/// The two resource types this module provisions.
pub(crate) const CLOUDFRONT_DISTRIBUTION: &str = "AWS::CloudFront::Distribution";
pub(crate) const CLOUDFRONT_ORIGIN_ACCESS_CONTROL: &str = "AWS::CloudFront::OriginAccessControl";

/// Which of them a resource is. Resolved once, so every call below is about that same
/// resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `AWS::CloudFront::Distribution`.
    Distribution,
    /// `AWS::CloudFront::OriginAccessControl`.
    OriginAccessControl,
}

/// The kind of CloudFront resource `resource_type` is, or `None` for a type this module does
/// not own.
pub(crate) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        CLOUDFRONT_DISTRIBUTION => Some(Kind::Distribution),
        CLOUDFRONT_ORIGIN_ACCESS_CONTROL => Some(Kind::OriginAccessControl),
        _ => None,
    }
}

/// The SigV4 signing name of the service that owns both types, which is what routes the
/// in-process call to CloudFront's own handlers (that model's `sigv4Name`/`endpointPrefix`,
/// `smithy_query(query="service_info")`). It speaks `restXml`.
const CLOUDFRONT_SIGNING_NAME: &str = "cloudfront";

/// The API-version prefix every CloudFront operation's URI carries, from the HTTP bindings
/// its model declares (`POST /2020-05-31/distribution`).
const CLOUDFRONT_URI_PREFIX: &str = "/2020-05-31";

/// The namespace a CloudFront request document carries: that model's own
/// `xmlNamespace.uri`.
const CLOUDFRONT_XML_NAMESPACE: &str = "http://cloudfront.amazonaws.com/doc/2020-05-31/";

/// The header CloudFront reports a resource's version in, and the one its update and its
/// delete take that version back as.
const ETAG_HEADER: &str = "etag";
const IF_MATCH_HEADER: &str = "if-match";

/// The error codes CloudFront answers for a resource it does not hold, which is what says a
/// delete has nothing left to remove and a read has nothing to report.
const NO_SUCH_DISTRIBUTION: &str = "NoSuchDistribution";
const NO_SUCH_ORIGIN_ACCESS_CONTROL: &str = "NoSuchOriginAccessControl";

/// The code CloudFront answers a delete of a distribution whose disabled configuration has
/// not reached the edge locations yet: "The specified CloudFront distribution is not
/// disabled. You must disable the distribution before you can delete it."
const DISTRIBUTION_NOT_DISABLED: &str = "DistributionNotDisabled";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// The status CloudFront reports a distribution in while a configuration is still
/// propagating to the edge locations (`Distribution$Status`), which is the state a deletion
/// of one is still in progress in.
const STATUS_IN_PROGRESS: &str = "InProgress";

/// How deeply a declared configuration may nest before it is refused.
///
/// CHOSEN by this emulator rather than read off a published quota: the rendering below walks
/// a declaration RECURSIVELY, and a template is caller-supplied, so the depth it can drive is
/// bounded before anything is rendered ([`refuse_unrenderable_members`]). It is far deeper
/// than any configuration CloudFront's own model admits — that model's deepest path
/// (a distribution's cache behaviour, its forwarded values, their cookies, the cookie names,
/// one name) is five.
const MAX_DECLARED_DEPTH: usize = 32;

/// Every property `AWS::CloudFront::Distribution` publishes.
///
/// Source: AWS's CloudFormation resource reference for the type. A declared property this
/// list does not carry is one no call here applies.
const DISTRIBUTION_PROPERTIES: &[&str] = &["DistributionConfig", "Tags"];

/// Every property `AWS::CloudFront::OriginAccessControl` publishes.
const ORIGIN_ACCESS_CONTROL_PROPERTIES: &[&str] = &["OriginAccessControlConfig"];

/// The members `DistributionConfig` publishes, in the order that shape declares them
/// (`smithy_query(query="shape",shape="DistributionConfig")`), less `CallerReference`, which
/// CloudFormation supplies.
///
/// This is both the ORDER the request document writes them in and the set of members a
/// declaration may state: the three the type publishes that are no members of the API's
/// configuration at all — the legacy `CNAMEs`, `CustomOrigin` and `S3Origin`, each
/// documented as deprecated in favour of a member above — are deliberately absent, so a
/// declaration carrying one is refused rather than reported as a distribution configured
/// without it.
const DISTRIBUTION_CONFIG_MEMBERS: &[&str] = &[
    "Aliases",
    "DefaultRootObject",
    "Origins",
    "OriginGroups",
    "DefaultCacheBehavior",
    "CacheBehaviors",
    "CustomErrorResponses",
    "Comment",
    "Logging",
    "PriceClass",
    "Enabled",
    "ViewerCertificate",
    "Restrictions",
    "WebACLId",
    "HttpVersion",
    "IPV6Enabled",
    "ContinuousDeploymentPolicyId",
    "Staging",
    "AnycastIpListId",
    "TenantConfig",
    "ConnectionMode",
    "ViewerMtlsConfig",
    "ConnectionFunctionAssociation",
    "CacheTagConfig",
];

/// The members `OriginAccessControlConfig` publishes, in the order that shape declares them.
/// The template spells each one identically.
const ORIGIN_ACCESS_CONTROL_CONFIG_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "SigningProtocol",
    "SigningBehavior",
    "OriginAccessControlOriginType",
];

/// The four members a template and the API spell differently, as
/// `(template property, API member)`.
///
/// Source: the CloudFormation resource reference for `AWS::CloudFront::Distribution` against
/// the shapes those properties belong to — `DistributionConfig$IsIPV6Enabled`,
/// `Origin$CustomHeaders`, `CustomOriginConfig$OriginSslProtocols` and `ViewerCertificate`'s
/// three initialisms (`ACMCertificateArn`, `IAMCertificateId`, `SSLSupportMethod`).
const RENAMED_MEMBERS: &[(&str, &str)] = &[
    ("IPV6Enabled", "IsIPV6Enabled"),
    ("OriginCustomHeaders", "CustomHeaders"),
    ("OriginSSLProtocols", "OriginSslProtocols"),
    ("AcmCertificateArn", "ACMCertificateArn"),
    ("IamCertificateId", "IAMCertificateId"),
    ("SslSupportMethod", "SSLSupportMethod"),
];

/// How a declared LIST reaches the API: the container the model wraps it in, and the element
/// each of its items carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListShape {
    /// `{Quantity, Items}` — the container most of CloudFront's lists are wrapped in.
    QuantityAndItems,
    /// `{Enabled, Quantity, Items}` — `TrustedSigners` and `TrustedKeyGroups`, whose
    /// `Enabled` "is `true` if any of the … in the list are configured", so it follows from
    /// the list the declaration wrote.
    EnabledQuantityAndItems,
    /// `{Items}` alone — `Tags`, whose container publishes no `Quantity`.
    ItemsOnly,
    /// `{Quantity, Items}` written into the PARENT element rather than a container of its
    /// own: a `GeoRestriction` carries `RestrictionType`, `Quantity` and `Items`, and the
    /// template spells its items as the member `Locations`.
    IntoTheParent,
    /// A list the model does not wrap at all: `TenantConfig$ParameterDefinitions`, whose
    /// items carry the default element name a restXml list member has.
    Unwrapped,
}

/// Every list a declared `DistributionConfig` can carry, as
/// `(template property, API member, item element, shape)`.
///
/// Source: the shapes of CloudFront's own model
/// (`smithy_query(query="shape",shape="Aliases,AliasList,…")`): the item element is the
/// `xmlName` the wrapped list's member carries, and the shape is which of `Quantity`,
/// `Items` and `Enabled` that container declares. A property that is already spelled
/// `{Quantity, Items}` in a template — an `OriginGroups`, an origin group's `Members` and
/// its failover `StatusCodes` — is here too: its own `Items` is resolved through the entry
/// of the member HOLDING it, so the `Quantity` the template wrote is the one that travels.
const WRAPPED_LISTS: &[(&str, &str, &str, ListShape)] = &[
    ("Aliases", "Aliases", "CNAME", ListShape::QuantityAndItems),
    ("Origins", "Origins", "Origin", ListShape::QuantityAndItems),
    (
        "CacheBehaviors",
        "CacheBehaviors",
        "CacheBehavior",
        ListShape::QuantityAndItems,
    ),
    (
        "CustomErrorResponses",
        "CustomErrorResponses",
        "CustomErrorResponse",
        ListShape::QuantityAndItems,
    ),
    (
        "OriginGroups",
        "OriginGroups",
        "OriginGroup",
        ListShape::QuantityAndItems,
    ),
    (
        "Members",
        "Members",
        "OriginGroupMember",
        ListShape::QuantityAndItems,
    ),
    (
        "StatusCodes",
        "StatusCodes",
        "StatusCode",
        ListShape::QuantityAndItems,
    ),
    (
        "AllowedMethods",
        "AllowedMethods",
        "Method",
        ListShape::QuantityAndItems,
    ),
    (
        "CachedMethods",
        "CachedMethods",
        "Method",
        ListShape::QuantityAndItems,
    ),
    (
        "TrustedSigners",
        "TrustedSigners",
        "AwsAccountNumber",
        ListShape::EnabledQuantityAndItems,
    ),
    (
        "TrustedKeyGroups",
        "TrustedKeyGroups",
        "KeyGroup",
        ListShape::EnabledQuantityAndItems,
    ),
    ("Headers", "Headers", "Name", ListShape::QuantityAndItems),
    (
        "WhitelistedNames",
        "WhitelistedNames",
        "Name",
        ListShape::QuantityAndItems,
    ),
    (
        "QueryStringCacheKeys",
        "QueryStringCacheKeys",
        "Name",
        ListShape::QuantityAndItems,
    ),
    (
        "LambdaFunctionAssociations",
        "LambdaFunctionAssociations",
        "LambdaFunctionAssociation",
        ListShape::QuantityAndItems,
    ),
    (
        "FunctionAssociations",
        "FunctionAssociations",
        "FunctionAssociation",
        ListShape::QuantityAndItems,
    ),
    (
        "OriginCustomHeaders",
        "CustomHeaders",
        "OriginCustomHeader",
        ListShape::QuantityAndItems,
    ),
    (
        "OriginSSLProtocols",
        "OriginSslProtocols",
        "SslProtocol",
        ListShape::QuantityAndItems,
    ),
    ("Locations", "", "Location", ListShape::IntoTheParent),
    ("Tags", "Tags", "Tag", ListShape::ItemsOnly),
    (
        "ParameterDefinitions",
        "ParameterDefinitions",
        "member",
        ListShape::Unwrapped,
    ),
];

/// Create the resource one of these declarations describes, in Amazon CloudFront itself — so
/// a client that deployed the stack can pass `!Ref <distribution>` straight to
/// `GetDistribution`, and `!Ref <control>` to an origin's `OriginAccessControlId`.
///
/// `Err` is CloudFront's own refusal of the create, which is what the resource fails with: a
/// configuration it will not take, an origin access control the distribution names that does
/// not exist, a log bucket that cannot receive the distribution's access logs.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::Distribution => create_distribution(ctx, region, resource, propagated).await,
        // `AWS::CloudFront::OriginAccessControl` publishes no `Tags` property and
        // `CreateOriginAccessControl` takes none: the type is not taggable, so no tags are
        // propagated to it.
        Kind::OriginAccessControl => create_origin_access_control(ctx, region, resource).await,
    }
}

/// Whether CloudFront reports a created resource ready.
///
/// Nothing is asked, and nothing waits. An origin access control EXISTS from the moment its
/// create answers. A distribution exists from that moment too — `GetDistribution` resolves
/// it, an invalidation can be created on it and a tenant can be bound to it — while
/// CloudFront propagates the configuration to its edge locations and reports it
/// `InProgress`; that is CloudFront's own transition rather than a wait on the creation,
/// exactly as the DB instance an `AWS::RDS::DBInstance` creates is reported created while
/// Amazon RDS still has it `creating`.
pub(super) fn ready() -> Readiness {
    Readiness::Ready
}

/// Apply a CHANGED declaration to the resource the stack already has, through CloudFront's
/// own update API — the call that takes the resource's id, so it keeps its identity and
/// nothing is replaced.
///
/// Both updates take the whole configuration and the version the caller read, so each is the
/// same document the create sent plus the `If-Match` that version answered.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<Option<Created>, Refusal> {
    // The DECLARED document, not the record's: by the time this is reached the resource
    // carries the properties it was CREATED with, so a property the new declaration
    // introduces is only in `declared`.
    let id = &resource.physical_resource_id;
    match kind {
        Kind::Distribution => {
            refuse_unapplied_properties(declared, &resource.resource_type, DISTRIBUTION_PROPERTIES)?;
            let config = distribution_config_document(declared, resource, propagated)?;
            let (_, version) = read_version(ctx, region, &distribution_config_path(id)).await?;
            let answered = call_cloudfront(
                ctx,
                region,
                "PUT",
                &distribution_config_path(id),
                Some(&config),
                version.as_deref(),
            )
            .await?;
            Ok(Some(Created {
                physical_resource_id: None,
                attributes: distribution_attributes(&answered),
            }))
        }
        Kind::OriginAccessControl => {
            refuse_unapplied_properties(
                declared,
                &resource.resource_type,
                ORIGIN_ACCESS_CONTROL_PROPERTIES,
            )?;
            let config = origin_access_control_document(declared, resource)?;
            let (_, version) =
                read_version(ctx, region, &origin_access_control_config_path(id)).await?;
            // `UpdateOriginAccessControl` is bound to the control's CONFIG path
            // (`PUT /2020-05-31/origin-access-control/{Id}/config`), exactly as the
            // distribution's update is bound to its own.
            call_cloudfront(
                ctx,
                region,
                "PUT",
                &origin_access_control_config_path(id),
                Some(&config),
                version.as_deref(),
            )
            .await?;
            Ok(Some(Created::default()))
        }
    }
}

/// Delete the resource a stack created, addressed by the id CloudFront minted for it.
///
/// A resource CloudFront no longer holds is already in the state the delete asks for, judged
/// on the CODE and never on a message. A distribution's deletion is not over when the call
/// answers — it has to be disabled and the disabled configuration has to reach the edge
/// locations first — which is what [`removal`] waits out.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let id = &resource.physical_resource_id;
    match kind {
        Kind::Distribution => remove_distribution(ctx, region, id).await.map(|_| ()),
        Kind::OriginAccessControl => {
            let version = match read_version(ctx, region, &origin_access_control_config_path(id))
                .await
            {
                Ok((_, version)) => version,
                Err(refusal) if refusal.code == NO_SUCH_ORIGIN_ACCESS_CONTROL => return Ok(()),
                Err(refusal) => return Err(refusal),
            };
            match call_cloudfront(
                ctx,
                region,
                "DELETE",
                &origin_access_control_path(id),
                None,
                version.as_deref(),
            )
            .await
            {
                Ok(_) => Ok(()),
                Err(refusal) if refusal.code == NO_SUCH_ORIGIN_ACCESS_CONTROL => Ok(()),
                Err(refusal) => Err(refusal),
            }
        }
    }
}

/// Whether CloudFront has finished removing a deleted resource.
///
/// An origin access control is gone as its delete answers. A distribution is not: "you must
/// disable the distribution before you can delete it", and the disabled configuration has to
/// have reached the edge locations, so the deletion is asked for again until that service no
/// longer holds the distribution ([`remove_distribution`]).
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match kind {
        Kind::Distribution => remove_distribution(ctx, region, physical_resource_id).await,
        Kind::OriginAccessControl => Ok(Removal::Gone),
    }
}

/// Create the distribution an `AWS::CloudFront::Distribution` declares.
///
/// The tags the declaration asks for and the ones CloudFormation gives the resource travel
/// on the create itself: `CreateDistributionWithTags` takes the configuration and the tag
/// list as one payload, which is the one call that makes a tagged distribution in a single
/// step.
async fn create_distribution(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        &resource.resource_type,
        DISTRIBUTION_PROPERTIES,
    )?;
    let config = distribution_config_members(&resource.properties, resource, propagated)?;
    let tags = tags_of(&resource.properties, propagated);
    let mut items = String::new();
    for (key, value) in &tags {
        items.push_str(&format!(
            "<Tag>{}{}</Tag>",
            xml_leaf("Key", key),
            xml_leaf("Value", value)
        ));
    }
    let body = format!(
        "<DistributionConfigWithTags xmlns=\"{CLOUDFRONT_XML_NAMESPACE}\">\
         <DistributionConfig>{config}</DistributionConfig>\
         <Tags><Items>{items}</Items></Tags></DistributionConfigWithTags>"
    );
    let answered = call_cloudfront(
        ctx,
        region,
        "POST",
        "/distribution?WithTags",
        Some(&body),
        None,
    )
    .await?;
    let id = answered_identifier(&answered, "Distribution", "CreateDistributionWithTags")?;
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: distribution_attributes(&answered),
    })
}

/// Create the origin access control an `AWS::CloudFront::OriginAccessControl` declares.
async fn create_origin_access_control(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        &resource.resource_type,
        ORIGIN_ACCESS_CONTROL_PROPERTIES,
    )?;
    let body = origin_access_control_document(&resource.properties, resource)?;
    let answered = call_cloudfront(
        ctx,
        region,
        "POST",
        "/origin-access-control",
        Some(&body),
        None,
    )
    .await?;
    let id = answered_identifier(&answered, "OriginAccessControl", "CreateOriginAccessControl")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// Ask CloudFront to take the distribution away, and report whether it still holds it.
///
/// One function for the delete and for the wait that follows it, because the work is the
/// same at both: a distribution CloudFront still has ENABLED is disabled with the
/// configuration it carries now, and the deletion is then asked for. The refusal that says
/// the disabled configuration has not reached the edge locations yet
/// ([`DISTRIBUTION_NOT_DISABLED`]) is not a failure of the deletion — it is the deletion
/// still being in progress.
async fn remove_distribution(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    id: &str,
) -> Result<Removal, Refusal> {
    let path = distribution_config_path(id);
    let (config, version) = match read_version(ctx, region, &path).await {
        Ok(read) => read,
        Err(refusal) if refusal.code == NO_SUCH_DISTRIBUTION => return Ok(Removal::Gone),
        Err(refusal) => return Err(refusal),
    };
    let mut version = version;
    // "The specified CloudFront distribution is not disabled. You must disable the
    // distribution before you can delete it": the configuration CloudFront holds NOW is
    // submitted back with `Enabled` false, so nothing else about the distribution is
    // changed on the way out.
    if config_enabled(&config) == Some(true) {
        if let Some(disabled) = with_root_member_text(&config, "Enabled", "false") {
            let (_, answered) = call_cloudfront_versioned(
                ctx,
                region,
                "PUT",
                &path,
                Some(&disabled),
                version.as_deref(),
            )
            .await?;
            version = answered.get(ETAG_HEADER).cloned();
        }
    }
    match call_cloudfront(
        ctx,
        region,
        "DELETE",
        &distribution_path(id),
        None,
        version.as_deref(),
    )
    .await
    {
        Ok(_) => Ok(Removal::Gone),
        Err(refusal) if refusal.code == NO_SUCH_DISTRIBUTION => Ok(Removal::Gone),
        Err(refusal) if refusal.code == DISTRIBUTION_NOT_DISABLED => {
            Ok(Removal::Pending(STATUS_IN_PROGRESS.to_string()))
        }
        Err(refusal) => Err(refusal),
    }
}

/// The `DistributionConfig` document a create or an update sends: the declared configuration
/// under the root element the operation's payload is bound to.
fn distribution_config_document(
    properties: &Value,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<String, Refusal> {
    let members = distribution_config_members(properties, resource, propagated)?;
    Ok(format!(
        "<DistributionConfig xmlns=\"{CLOUDFRONT_XML_NAMESPACE}\">{members}</DistributionConfig>"
    ))
}

/// The members of the `DistributionConfig` a declaration describes, rendered in the order
/// that shape declares them.
///
/// `CallerReference` comes first, as the shape declares it, and is CloudFormation's to
/// supply ([`caller_reference`]). `Comment` is REQUIRED by the API and optional in a
/// template, so a declaration that states none sends the empty comment rather than leaving
/// the member out of a document the service would refuse.
fn distribution_config_members(
    properties: &Value,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<String, Refusal> {
    let declared = properties
        .get("DistributionConfig")
        .and_then(Value::as_object)
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "Property DistributionConfig cannot be empty for resource of type {}",
                resource.resource_type
            ),
        })?;
    refuse_unapplied_members(declared, "DistributionConfig", DISTRIBUTION_CONFIG_MEMBERS)?;
    refuse_unrenderable_members(properties, "DistributionConfig", 0)?;
    let mut rendered = xml_leaf("CallerReference", &caller_reference(resource, propagated));
    for member in DISTRIBUTION_CONFIG_MEMBERS {
        let Some(value) = declared.get(*member).filter(|value| !value.is_null()) else {
            if *member == "Comment" {
                rendered.push_str(&xml_leaf("Comment", ""));
            }
            continue;
        };
        rendered.push_str(&rendered_member(member, value, declared, "DistributionConfig"));
    }
    Ok(rendered)
}

/// The `OriginAccessControlConfig` document a create or an update of an origin access
/// control sends.
///
/// `Name` is required by the API and optional in a template, so a declaration that names
/// none is created under the identifier CloudFormation computed for the resource — the name
/// is not this resource's identity (CloudFront mints that), it is what the control is called.
fn origin_access_control_document(
    properties: &Value,
    resource: &StoredResource,
) -> Result<String, Refusal> {
    let declared = properties
        .get("OriginAccessControlConfig")
        .and_then(Value::as_object)
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "Property OriginAccessControlConfig cannot be empty for resource of type {}",
                resource.resource_type
            ),
        })?;
    refuse_unapplied_members(
        declared,
        "OriginAccessControlConfig",
        ORIGIN_ACCESS_CONTROL_CONFIG_MEMBERS,
    )?;
    refuse_unrenderable_members(properties, "OriginAccessControlConfig", 0)?;
    let mut rendered = String::new();
    for member in ORIGIN_ACCESS_CONTROL_CONFIG_MEMBERS {
        match declared.get(*member).filter(|value| !value.is_null()) {
            Some(value) => rendered.push_str(&rendered_member(
                member,
                value,
                declared,
                "OriginAccessControlConfig",
            )),
            None if *member == "Name" => {
                rendered.push_str(&xml_leaf("Name", &resource.physical_resource_id));
            }
            None => {}
        }
    }
    Ok(format!(
        "<OriginAccessControlConfig xmlns=\"{CLOUDFRONT_XML_NAMESPACE}\">{rendered}\
         </OriginAccessControlConfig>"
    ))
}

/// One declared member, as the request document carries it.
///
/// `siblings` is the structure the member belongs to and `owner` is that structure's own
/// member name, which the two members whose API home is not where a template writes them
/// need: `CachedMethods` is written INSIDE `AllowedMethods`, and the `Items` of a container a
/// template already spells as `{Quantity, Items}` carries the element name the OWNER's entry
/// names.
fn rendered_member(member: &str, value: &Value, siblings: &Map<String, Value>, owner: &str) -> String {
    // `CachedMethods` is a member of `AllowedMethods` in the API: it is rendered there, so
    // reaching it as a sibling writes nothing.
    if member == "CachedMethods" && siblings.contains_key("AllowedMethods") {
        return String::new();
    }
    let name = api_member(member);
    match value {
        Value::Array(items) => rendered_list(member, name, items, siblings, owner),
        Value::Object(members) => {
            format!("<{name}>{}</{name}>", rendered_structure(member, members))
        }
        scalar => scalar_text(scalar)
            .map(|text| xml_leaf(name, &text))
            .unwrap_or_default(),
    }
}

/// The members of one declared STRUCTURE, in the order the declaration wrote them. `owner` is
/// the member name that structure is written under.
///
/// Two structures carry a member the API does not take from a template, and both are
/// written here:
///
/// * a template's `Logging` has no `Enabled` member — "if you don't want to enable logging …
///   specify `false` for `Enabled`, and specify empty `Bucket` and `Prefix` elements"
///   (`LoggingConfig$Enabled`), so declaring a bucket to log into IS enabling logging and
///   the member the API requires is written from that;
/// * a `GeoRestriction`'s declared `Locations` is the API's `Quantity` and `Items`, written
///   into the restriction itself ([`ListShape::IntoTheParent`]).
fn rendered_structure(owner: &str, members: &Map<String, Value>) -> String {
    let mut rendered = String::new();
    if owner == "Logging" && !members.contains_key("Enabled") {
        let enabled = members
            .get("Bucket")
            .and_then(Value::as_str)
            .is_some_and(|bucket| !bucket.trim().is_empty());
        rendered.push_str(&xml_leaf("Enabled", &enabled.to_string()));
    }
    for (name, value) in members {
        if value.is_null() {
            continue;
        }
        rendered.push_str(&rendered_member(name, value, members, owner));
    }
    rendered
}

/// One declared LIST, wrapped the way the model wraps it ([`WRAPPED_LISTS`]).
///
/// A list this module's catalogue does not carry is written as the element name the
/// declaration used holding one element per item, which is what an unwrapped restXml list
/// is: nothing is dropped, and nothing is invented.
fn rendered_list(
    member: &str,
    name: &str,
    items: &[Value],
    siblings: &Map<String, Value>,
    owner: &str,
) -> String {
    // `Items` is the list a container the TEMPLATE already spells as `{Quantity, Items}`
    // holds — an `OriginGroups`, an origin group's `Members`, its failover `StatusCodes` — so
    // the element its items carry is the one the OWNER's entry names, and the `Quantity`
    // beside it is the one the declaration wrote (rendered as the scalar it is).
    if member == "Items" {
        let item = wrapped_list(owner)
            .map(|(_, _, item, _)| item)
            .unwrap_or(UNWRAPPED_ITEM_ELEMENT);
        return format!("<Items>{}</Items>", rendered_items(item, items, owner));
    }
    let Some((_, _, item, shape)) = wrapped_list(member) else {
        return items
            .iter()
            .map(|value| rendered_member(member, value, siblings, owner))
            .collect();
    };
    let rendered = rendered_items(item, items, member);
    let quantity = xml_leaf("Quantity", &items.len().to_string());
    // The one member that is a MEMBER of another in the API rather than a sibling.
    let nested = if member == "AllowedMethods" {
        siblings
            .get("CachedMethods")
            .and_then(Value::as_array)
            .map(|cached| {
                format!(
                    "<CachedMethods>{}<Items>{}</Items></CachedMethods>",
                    xml_leaf("Quantity", &cached.len().to_string()),
                    rendered_items("Method", cached, "CachedMethods")
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    match shape {
        ListShape::QuantityAndItems => {
            format!("<{name}>{quantity}<Items>{rendered}</Items>{nested}</{name}>")
        }
        ListShape::EnabledQuantityAndItems => {
            let enabled = xml_leaf("Enabled", &(!items.is_empty()).to_string());
            format!("<{name}>{enabled}{quantity}<Items>{rendered}</Items></{name}>")
        }
        ListShape::ItemsOnly => format!("<{name}><Items>{rendered}</Items></{name}>"),
        ListShape::IntoTheParent => format!("{quantity}<Items>{rendered}</Items>"),
        ListShape::Unwrapped => format!("<{name}>{rendered}</{name}>"),
    }
}

/// The items of one list, each under the element `item` the model's own list member carries.
fn rendered_items(item: &str, items: &[Value], owner: &str) -> String {
    items
        .iter()
        .map(|value| match value {
            Value::Object(members) => {
                format!("<{item}>{}</{item}>", rendered_structure(item, members))
            }
            Value::Array(nested) => {
                let empty = Map::new();
                rendered_list(item, item, nested, &empty, owner)
            }
            scalar => scalar_text(scalar)
                .map(|text| xml_leaf(item, &text))
                .unwrap_or_default(),
        })
        .collect()
}

/// The element name an UNWRAPPED restXml list's items carry where its model names none,
/// which is the default that protocol gives a list member.
const UNWRAPPED_ITEM_ELEMENT: &str = "member";

/// The catalogue entry for a declared list member, or `None` for a member that is no wrapped
/// list.
fn wrapped_list(member: &str) -> Option<(&'static str, &'static str, &'static str, ListShape)> {
    WRAPPED_LISTS
        .iter()
        .find(|(property, _, _, _)| *property == member)
        .copied()
}

/// The API member a declared property name reaches: the name the template wrote, unless the
/// two sides spell it differently ([`RENAMED_MEMBERS`]).
fn api_member(member: &str) -> &str {
    RENAMED_MEMBERS
        .iter()
        .find(|(property, _)| *property == member)
        .map(|(_, api)| *api)
        .unwrap_or(member)
}

/// The attributes a `Distribution` answer reports for the resource: the id CloudFront minted
/// and the domain name it serves the distribution on, which are the two attributes
/// `AWS::CloudFront::Distribution` publishes.
///
/// A MULTI-TENANT distribution serves no traffic of its own and reports no domain name, so
/// such a resource carries no `DomainName` attribute rather than an empty one.
fn distribution_attributes(answered: &str) -> BTreeMap<String, String> {
    let reported = xml_scalar_children(answered, "Distribution");
    let mut attributes = BTreeMap::new();
    for (attribute, member) in [("Id", "Id"), ("DomainName", "DomainName")] {
        if let Some(value) = reported
            .get(member)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    attributes
}

/// The identifier the create's answer reports for the resource it made, or the refusal for
/// an answer that identifies nothing: a resource whose id nothing reported is not one a stack
/// can publish.
fn answered_identifier(answered: &str, element: &str, operation: &str) -> Result<String, Refusal> {
    xml_scalar_children(answered, element)
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Refusal {
            code: "UnidentifiedFailure".to_string(),
            message: format!("{operation} answered no {element}.Id"),
        })
}

/// The `CallerReference` CloudFormation sends with a create that requires one.
///
/// `DistributionConfig$CallerReference` is REQUIRED — "a unique value (for example, a
/// date-time stamp) that ensures that the request can't be replayed" — and the type publishes
/// no property for it, so it is CloudFormation's to supply. It is derived from the stack this
/// resource belongs to and its place in it (the two tags CloudFormation records on every
/// resource it provisions), so it is unique per stack and logical id and the SAME string if
/// this deployment is attempted again — which is exactly the retry that member exists for,
/// and what lets an update of the distribution submit the reference it was created under
/// ("a distribution's caller reference identifies it for its whole life").
fn caller_reference(resource: &StoredResource, propagated: &[(String, String)]) -> String {
    let stack = propagated
        .iter()
        .find(|(key, _)| key == super::STACK_ID_TAG)
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    crate::state::template::content_digest(
        CALLER_REFERENCE_SALT,
        &format!("{stack}/{}", resource.logical_id),
        32,
    )
}

/// The salt a distribution's caller reference is derived under.
const CALLER_REFERENCE_SALT: &str = "cloudfront-distribution";

/// The URI each per-resource operation is bound to, with the id written as ONE path segment
/// so an id outside the modelled form is refused by that operation's validation instead of
/// addressing a different one.
fn distribution_path(id: &str) -> String {
    format!("/distribution/{}", percent_encoded(id))
}

fn distribution_config_path(id: &str) -> String {
    format!("/distribution/{}/config", percent_encoded(id))
}

fn origin_access_control_path(id: &str) -> String {
    format!("/origin-access-control/{}", percent_encoded(id))
}

fn origin_access_control_config_path(id: &str) -> String {
    format!("/origin-access-control/{}/config", percent_encoded(id))
}

/// Read a resource's configuration and the VERSION CloudFront reports it under.
///
/// Every mutation of a CloudFront resource is conditional on that version (`If-Match`), and
/// the version travels in a response HEADER rather than in the answer's body — so a provider
/// that is about to change or remove one reads it here first.
async fn read_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    path: &str,
) -> Result<(String, Option<String>), Refusal> {
    let (document, headers) =
        call_cloudfront_versioned(ctx, region, "GET", path, None, None).await?;
    let version = headers.get(ETAG_HEADER).cloned();
    Ok((document, version))
}

/// Call one CloudFront operation in process, on the URI its model binds that operation to,
/// and read the XML document it answers with.
async fn call_cloudfront(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    path: &str,
    body: Option<&str>,
    if_match: Option<&str>,
) -> Result<String, Refusal> {
    call_cloudfront_versioned(ctx, region, method, path, body, if_match)
        .await
        .map(|(document, _headers)| document)
}

/// [`call_cloudfront`], reporting the `ETag` the answer carried as well.
async fn call_cloudfront_versioned(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    path: &str,
    body: Option<&str>,
    if_match: Option<&str>,
) -> Result<(String, BTreeMap<String, String>), Refusal> {
    let sent: Vec<(&'static str, String)> = if_match
        .map(|version| vec![(IF_MATCH_HEADER, version.to_string())])
        .unwrap_or_default();
    call_rest_xml(
        ctx,
        CLOUDFRONT_SIGNING_NAME,
        region,
        method,
        &format!("{CLOUDFRONT_URI_PREFIX}{path}"),
        body,
        &sent,
        &[ETAG_HEADER],
    )
    .await
}

/// Whether the `DistributionConfig` document `config` has the distribution ENABLED.
///
/// Read from the configuration's OWN `Enabled` member — the depth-1 one — because the
/// document carries several (a cache behaviour's trusted signers, its gRPC configuration, the
/// logging configuration each have one of their own), and only the configuration's own says
/// whether the distribution is enabled.
fn config_enabled(config: &str) -> Option<bool> {
    xml_scalar_children(config, "DistributionConfig")
        .get("Enabled")
        .and_then(Value::as_str)
        .map(|enabled| enabled.trim().eq_ignore_ascii_case("true"))
}

/// `document` with the text of its root's own `member` element replaced by `value`, or `None`
/// where the document carries no such element.
///
/// The document is re-emitted by the same maintained XML implementation that read it, so
/// everything but that one element's text travels back exactly as the answering service wrote
/// it — which is what lets a disable submit the configuration CloudFront holds rather than one
/// rebuilt from the declaration.
fn with_root_member_text(document: &str, member: &str, value: &str) -> Option<String> {
    use ::quick_xml::events::{BytesText, Event};

    let mut reader = ::quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut writer = ::quick_xml::Writer::new(Vec::new());
    let mut depth = 0usize;
    let mut replacing = false;
    let mut replaced = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                depth += 1;
                let hit = depth == 2 && start.local_name().as_ref() == member.as_bytes();
                writer.write_event(Event::Start(start)).ok()?;
                if hit {
                    writer
                        .write_event(Event::Text(BytesText::new(value)))
                        .ok()?;
                    replacing = true;
                    replaced = true;
                }
            }
            Ok(Event::End(end)) => {
                depth = depth.saturating_sub(1);
                replacing = false;
                writer.write_event(Event::End(end)).ok()?;
            }
            Ok(other) => {
                if !replacing {
                    writer.write_event(other).ok()?;
                }
            }
            Err(_) => return None,
        }
    }
    replaced
        .then(|| String::from_utf8(writer.into_inner()).ok())
        .flatten()
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps a distribution from being reported as created — or as updated
/// — with less than its declaration asked for.
fn refuse_unapplied_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    refuse_unapplied_members(properties, resource_type, published)
}

/// [`refuse_unapplied_properties`] for the members of one declared STRUCTURE, which is where
/// a configuration's own members are judged.
fn refuse_unapplied_members(
    members: &Map<String, Value>,
    owner: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    for member in members.keys() {
        if published.contains(&member.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!("this emulator does not apply the {member} property of {owner}"),
        });
    }
    Ok(())
}

/// Refuse a declaration the rendering below could not carry: one nested deeper than
/// [`MAX_DECLARED_DEPTH`], or carrying a member name that is no API member name at all.
///
/// Both are properties of the whole declared document, so both are decided HERE, once, before
/// anything is rendered — which is what keeps the rendering itself a total function over a
/// document it has already been told is renderable.
///
/// Every member of every shape in CloudFront's model is spelled in ASCII letters and digits,
/// and below the configuration's own members ([`DISTRIBUTION_CONFIG_MEMBERS`]) a declared
/// name travels INTO the request document as the element it names. So a name outside that
/// alphabet names no member of the API and is refused, naming it — rather than reaching the
/// wire as text that is not an element name.
fn refuse_unrenderable_members(value: &Value, owner: &str, depth: usize) -> Result<(), Refusal> {
    if depth > MAX_DECLARED_DEPTH {
        return Err(Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "{owner} nests deeper than the {MAX_DECLARED_DEPTH} levels this emulator \
                 carries"
            ),
        });
    }
    match value {
        Value::Object(members) => {
            for (member, held) in members {
                if member.is_empty() || !member.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(Refusal {
                        code: MISSING_PROPERTY.to_string(),
                        message: format!("{owner} declares no member {member:?}"),
                    });
                }
                refuse_unrenderable_members(held, member, depth + 1)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                refuse_unrenderable_members(item, owner, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;

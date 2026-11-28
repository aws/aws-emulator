// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ram-amazonresourcesharing service.
//!
//! The service's primary resource is the **resource share**. Every share is stored
//! exactly once, keyed by its ARN (`arn:aws:ram:<region>:<account>:resource-share/<id>`),
//! and every association between a share and an entity — a resource, a principal or
//! a source — is stored exactly once beside it, keyed by
//! `(share arn, association type, associated entity)`. Each operation reads and
//! writes those two maps, so what a create or an update stored is exactly what a
//! later read reports.
//!
//! Both maps are `BTreeMap`s: the key order is total and stable, so a paginated read
//! can name the next entry in an opaque `nextToken` and a page boundary survives
//! later writes.
//!
//! Sharing with a principal outside the owning account's organization also stores an
//! **invitation**, keyed by its own ARN, beside the share: the invited account joins
//! the share only once it answers, so what is pending for it is a record of its own.
//!
//! Managed permissions are kept in one map too, and a fresh store already holds the
//! AWS managed permissions RAM ships ([`aws_managed_permissions`]): those exist before
//! any caller creates anything, so the permissions one account created and the ones
//! Amazon Web Services maintains are read back through the same lookup.

mod aws_managed_permissions;
mod permission_associations;
mod permission_attachments;
mod permission_deletion;
mod permission_deprecation;
mod permission_replacement;
mod permission_version_deletion;
mod policy_actions;

pub use aws_managed_permissions::*;
pub use permission_associations::*;
pub use permission_attachments::*;
pub use permission_deletion::*;
pub use permission_deprecation::*;
pub use permission_replacement::*;
pub use permission_version_deletion::*;
pub use policy_actions::*;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Resource, ResourceShare, ResourceShareAssociation, ResourceShareConfiguration,
    ResourceShareInvitation, ResourceSharePermissionDetail, ResourceSharePermissionSummary, Tag,
    TagFilter,
};

/// This service's ARN namespace and sigv4 signing name (`awsService.arnNamespace`:
/// `smithy_query(query="service_info")` reports `ram`).
pub const ARN_SERVICE: &str = "ram";

/// The resource portion a resource-share ARN carries before its id.
pub const SHARE_ARN_RESOURCE_PREFIX: &str = "resource-share/";

/// The resource portion a resource-share-invitation ARN carries before its id.
pub const INVITATION_ARN_RESOURCE_PREFIX: &str = "resource-share-invitation/";

/// The ARN service field of AWS Organizations, the service whose ARNs name a part of
/// an organization's hierarchy (`arn:aws:organizations::<account>:organization/o-…`,
/// `…:ou/o-…/ou-…`, `…:root/o-…/r-…`, `…:account/o-…/<id>`).
pub const ORGANIZATIONS_ARN_SERVICE: &str = "organizations";

/// The ARN service field of AWS Identity and Access Management, the service whose
/// ARNs name a user or a role — the principal forms a RAM invitation can be
/// addressed to (`ResourceShareInvitation$receiverArn`).
pub const IAM_ARN_SERVICE: &str = "iam";

/// The number of digits in an Amazon Web Services account id.
pub const ACCOUNT_ID_LEN: usize = 12;

/// The id prefixes AWS Organizations gives the parts of its hierarchy, which a RAM
/// principal may name without an ARN: an organization (`o-`), an organizational unit
/// (`ou-`) and a root (`r-`). Longest prefix first, so `ou-` is never read as `o-`.
pub const ORGANIZATIONS_ID_PREFIXES: [&str; 3] = ["ou-", "o-", "r-"];

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `allowExternalPrincipals` when `CreateResourceShare` omits it.
///
/// From that member's own documentation: "The default value is true."
pub const DEFAULT_ALLOW_EXTERNAL_PRINCIPALS: bool = true;

/// `ResourceShareConfiguration$retainSharingOnAccountLeaveOrganization` when the
/// create request names no configuration, or names one that leaves the member out.
///
/// That member "specifies whether the consumer account retains access to the
/// resource share after leaving the organization" (its own documentation); a share
/// gets no such retention unless it is asked for, so the service default is false.
/// A share's configuration is part of its record either way: RAM reports the
/// structure on every `ResourceShare` it returns, with the defaults for a share
/// created without an explicit one.
pub const DEFAULT_RETAIN_SHARING_ON_ACCOUNT_LEAVE_ORGANIZATION: bool = false;

/// The `featureSet` of a share created through the RAM API.
///
/// From the modelled `ResourceShare$featureSet` documentation: `STANDARD` is "a
/// resource share that supports all functionality … might have been created by RAM";
/// the other two values (`CREATED_FROM_POLICY`, `PROMOTING_TO_STANDARD`) describe a
/// share the customer created by attaching a resource-based policy, which no
/// operation in this crate can produce.
pub const FEATURE_SET_STANDARD: &str = "STANDARD";

/// `ResourceShareStatus` values this slice stores, from
/// `smithy_query(query="shape",shape="ResourceShareStatus")`.
pub const SHARE_STATUS_ACTIVE: &str = "ACTIVE";
/// A share `DeleteResourceShare` has removed: it keeps its record as a tombstone so
/// the ARN can never be reused and so a read that asks for `DELETED` can see it.
pub const SHARE_STATUS_DELETED: &str = "DELETED";

/// `ResourceShareAssociationStatus` values this slice stores, from
/// `smithy_query(query="shape",shape="ResourceShareAssociationStatus")`.
pub const ASSOCIATION_STATUS_ASSOCIATING: &str = "ASSOCIATING";
/// The status an association reaches once the move RAM owes it falls due.
pub const ASSOCIATION_STATUS_ASSOCIATED: &str = "ASSOCIATED";
/// The status an association that is being torn down carries while the teardown is
/// still owed.
pub const ASSOCIATION_STATUS_DISASSOCIATING: &str = "DISASSOCIATING";
/// The terminal status of an association RAM has torn down. The record survives in
/// it, so a read still reports the association and a caller polling for the teardown
/// to finish sees it complete.
pub const ASSOCIATION_STATUS_DISASSOCIATED: &str = "DISASSOCIATED";

/// The message RAM reports on a principal association that has not settled yet.
///
/// RAM invites the principal to the share, and the principal accepts or rejects the
/// invitation, so what a pending principal association is waiting for is that
/// answer. The member that reports it is the modelled
/// `ResourceShareAssociation$statusMessage` ("A message about the status of the
/// association"); AWS publishes no wording, so this sentence is this emulator's and
/// the RULE — a principal association that has not completed says what it is waiting
/// for, and a settled one reports nothing — is what is pinned.
pub const ASSOCIATING_PRINCIPAL_STATUS_MESSAGE: &str =
    "Waiting for the principal to either accept or reject the resource share invitation.";

/// `ResourceShareAssociationType` values, from
/// `smithy_query(query="shape",shape="ResourceShareAssociationType")`.
pub const ASSOCIATION_TYPE_RESOURCE: &str = "RESOURCE";
/// The association type of an entry that came from `principals`.
pub const ASSOCIATION_TYPE_PRINCIPAL: &str = "PRINCIPAL";
/// The association type of an entry that came from `sources`.
pub const ASSOCIATION_TYPE_SOURCE: &str = "SOURCE";

/// `ResourceShareInvitationStatus` values this slice stores, from
/// `smithy_query(query="shape",shape="ResourceShareInvitationStatus")`. An invitation
/// starts `PENDING` — the invited account has neither accepted nor rejected it and it
/// has not expired — and the other three are the answers it can reach.
pub const INVITATION_STATUS_PENDING: &str = "PENDING";
/// The status of an invitation the invited account accepted.
pub const INVITATION_STATUS_ACCEPTED: &str = "ACCEPTED";
/// The status of an invitation the invited account rejected.
pub const INVITATION_STATUS_REJECTED: &str = "REJECTED";
/// The status of an invitation nobody answered before it expired.
pub const INVITATION_STATUS_EXPIRED: &str = "EXPIRED";

/// `ResourceStatus` values this slice reports for a resource association, from
/// `smithy_query(query="shape",shape="ResourceStatus")`.
pub const RESOURCE_STATUS_AVAILABLE: &str = "AVAILABLE";
/// The status of a resource whose association with the share has not settled yet.
pub const RESOURCE_STATUS_PENDING: &str = "PENDING";
/// The status of a resource the share no longer shares.
pub const RESOURCE_STATUS_UNAVAILABLE: &str = "UNAVAILABLE";

/// `ResourceRegionScope` values, from
/// `smithy_query(query="shape",shape="ResourceRegionScope")`: a resource reachable
/// "from any Amazon Web Services Region" is `GLOBAL`, and one reachable "only by
/// using requests that target the Amazon Web Services Region in which the resource
/// exists" is `REGIONAL` (`Resource$resourceRegionScope`).
pub const RESOURCE_REGION_SCOPE_GLOBAL: &str = "GLOBAL";
/// The scope of a resource that exists in one Region.
pub const RESOURCE_REGION_SCOPE_REGIONAL: &str = "REGIONAL";
/// The `ResourceRegionScopeFilter` value that filters nothing out, and the value a
/// read uses when the request names no scope: "The default value is ALL"
/// (`ListPendingInvitationResources$resourceRegionScope`).
pub const RESOURCE_REGION_SCOPE_FILTER_ALL: &str = "ALL";

/// `ResourceOwner` values, from `smithy_query(query="shape",shape="ResourceOwner")`.
pub const RESOURCE_OWNER_SELF: &str = "SELF";
/// Shares another account shares with this one — none, in this emulator, because
/// every share it holds was created locally by the account that owns it.
pub const RESOURCE_OWNER_OTHER_ACCOUNTS: &str = "OTHER-ACCOUNTS";

/// How long after the call that creates an association RAM takes to move it from
/// `ASSOCIATING` to `ASSOCIATED`.
///
/// RAM answers `AssociateResourceShare` with the new associations in the
/// non-terminal `ASSOCIATING` status (the status the modelled
/// `ResourceShareAssociationStatus` enum carries for an association still being
/// made), so reaching `ASSOCIATED` is a transition the clock owes rather than
/// something a caller asks for. AWS publishes no duration for it, so this span is
/// this emulator's own choice; it is deliberately not zero, so a caller that reads
/// straight back still observes `ASSOCIATING`. Nothing sleeps for it: the due
/// instant is stored and the move is materialised by the first call at or after it.
pub const ASSOCIATION_READY_AFTER_MILLIS: i64 = 500;

/// How long a resource share invitation stays answerable before it expires.
///
/// RAM gives the invited account SEVEN DAYS to accept or reject an invitation to a
/// resource share, and the invitation expires at the end of them — which is what the
/// modelled `EXPIRED` invitation status and the
/// `ResourceShareInvitationExpiredException` ("the specified invitation is past its
/// expiration date and time") report. The expiry is a move the clock owes: the due
/// instant is stored on the record and nothing sleeps for it.
pub const INVITATION_EXPIRES_AFTER_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How long after the call that tears an association down RAM takes to move it from
/// `DISASSOCIATING` to `DISASSOCIATED`.
///
/// The mirror of [`ASSOCIATION_READY_AFTER_MILLIS`], and owed on the same terms: a
/// teardown that has something to revoke is answered in the non-terminal
/// `DISASSOCIATING` status, so reaching `DISASSOCIATED` is a move the clock owes
/// rather than something a caller asks for. AWS publishes no duration for it either,
/// so this span is this emulator's own choice; it is deliberately not zero, and
/// nothing sleeps for it.
pub const DISASSOCIATION_READY_AFTER_MILLIS: i64 = 500;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteResourceShare`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. A cap is RAM's own published limit
// where it publishes one; where it publishes none the constant says so and the
// value is this emulator's resource guard, set well above any realistic use.
// ---------------------------------------------------------------------------

/// The most resource shares this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. A create that would exceed it is
/// refused with the modelled `ResourceShareLimitExceededException`.
pub const MAX_RESOURCE_SHARES: usize = 500;

/// The most entities one call may associate.
///
/// From the `ResourceShareLimitExceededException` documentation: "You can associate
/// up to 100 resources per call."
pub const MAX_ENTITIES_PER_CALL: usize = 100;

/// The most associations one share may hold, across all three association types.
///
/// Emulator resource guard, NOT an AWS quota.
pub const MAX_ASSOCIATIONS_PER_SHARE: usize = 1000;

/// The most invitations one share keeps, across every account it has invited and
/// every answer those invitations reached.
///
/// Emulator resource guard, NOT an AWS quota. An answered invitation is kept beside
/// its share so a read can still report what became of it; at the cap the oldest
/// ANSWERED one is forgotten, and a share whose invitations are all unanswered
/// invites nobody further rather than growing past it.
pub const MAX_INVITATIONS_PER_SHARE: usize = 1000;

/// The most tags one resource share may carry.
///
/// RAM follows the Amazon Web Services tagging limit of 50 tags per resource; a
/// create that would exceed it is refused with the modelled
/// `TagLimitExceededException`.
pub const MAX_TAGS_PER_SHARE: usize = 50;

/// The longest `name`, `clientToken` or tag key/value this emulator stores. The
/// model publishes no length for any of them, so this is the emulator's guard; a
/// longer value is refused rather than stored.
pub const MAX_NAME_LEN: usize = 256;

/// The longest ARN or principal/source identifier this emulator stores: the AWS ARN
/// length limit.
pub const MAX_ARN_LEN: usize = 2048;

/// The most `(operation, clientToken)` pairs this emulator remembers for
/// idempotency. Emulator resource guard: the oldest entry is forgotten when a new
/// one arrives at the cap, so a long-running emulator cannot grow without bound.
pub const MAX_CLIENT_TOKENS: usize = 1024;

/// The message every `IdempotentParameterMismatchException` this service answers
/// carries: RAM words this refusal with one fixed sentence, identical for every
/// tokened operation and interpolating nothing from the request, so every handler
/// that compares a [`ClientTokenRecord`]'s fingerprint reports it from here.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "The request has the same client token as a previous request, but the requests are not the \
     same.";

/// The largest page any paginated read here builds, and the size it uses when the
/// request omits `maxResults`: the maximum of the modelled `MaxResults` range
/// (1..500).
pub const MAX_PAGE_SIZE: usize = 500;

/// The tag-key prefix AWS reserves for its own tags; a caller may not set one.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The IAM policy language version an instantiated resource policy is written in:
/// the current one, which the policies RAM writes today carry.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The `Effect` an instantiated resource policy's statement carries.
///
/// The statement RAM instantiates is a managed permission's template, whose
/// "Effect: must be set to ALLOW" (`CreatePermission$policyTemplate`'s own
/// documentation), so a share's statement only ever allows.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";

/// The action part of an instantiated statement's `Action`: every action of the
/// service namespace the statement names. The statement names a namespace rather
/// than a list of actions because this service holds no managed permission to read
/// an action list from; `instantiated_policy` says so where it builds the document.
pub const POLICY_ACTION_WILDCARD: &str = "*";

/// The resource portion a managed-permission ARN carries before its name
/// (`arn:aws:ram:<region>:<account>:permission/<name>`).
pub const PERMISSION_ARN_RESOURCE_PREFIX: &str = "permission/";

/// The `PermissionType` of a permission `CreatePermission` minted.
///
/// From `ResourceSharePermissionSummary$permissionType`: `CUSTOMER_MANAGED` is the
/// one "you, or another principal in your account created"; `AWS_MANAGED`
/// ([`PERMISSION_TYPE_AWS_MANAGED`]) is one "Amazon Web Services created and
/// manages", which no operation in this crate can produce — those are the catalog
/// entries the store is seeded with.
pub const PERMISSION_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The `PermissionTypeFilter` value that keeps every kind of managed permission, and
/// the one a read that names no filter is answered under: "ALL – returns both Amazon
/// Web Services managed permissions and customer managed permissions. If you don't
/// specify this parameter, the default is All" (`ListPermissions$permissionType`).
pub const PERMISSION_TYPE_FILTER_ALL: &str = "ALL";

/// The `PermissionFeatureSet` of a permission created through the RAM API: the one
/// "that supports all functionality". The other two values describe a permission RAM
/// wrote from an attached resource-based policy
/// (`ResourceSharePermissionSummary$featureSet`).
pub const PERMISSION_FEATURE_SET_STANDARD: &str = "STANDARD";

/// The `PermissionStatus` a freshly created permission version carries: "this
/// permission or version can be associated with resource shares"
/// (`ResourceSharePermissionDetail$status`).
pub const PERMISSION_STATUS_ATTACHABLE: &str = "ATTACHABLE";

/// The most versions one managed permission may carry.
///
/// From `CreatePermissionVersion`'s own documentation: "If the specified customer
/// managed permission already has the maximum of 5 versions, then you must delete one
/// of the existing versions before you can create a new one." A create past it is
/// refused with the modelled `PermissionVersionsLimitExceededException`.
pub const MAX_PERMISSION_VERSIONS: usize = 5;

/// The version number a permission's first version carries, which is also its
/// default version: versions are numbered from one upward
/// (`GetPermission$permissionVersion`, `ListPermissionVersions`).
pub const FIRST_PERMISSION_VERSION: i32 = 1;

/// The `Effect` a managed permission's policy template must carry: "Effect: must be
/// set to ALLOW" (`CreatePermission$policyTemplate`'s own documentation).
pub const POLICY_TEMPLATE_EFFECT_ALLOW: &str = "ALLOW";

/// The `Effect` element of a policy template.
pub const POLICY_TEMPLATE_EFFECT_ELEMENT: &str = "Effect";

/// The `Action` element of a policy template.
pub const POLICY_TEMPLATE_ACTION_ELEMENT: &str = "Action";

/// The optional `Condition` element of a policy template.
pub const POLICY_TEMPLATE_CONDITION_ELEMENT: &str = "Condition";

/// The most managed permissions one account may CREATE in this emulator's store.
/// Emulator resource guard, worded as RAM's own refusal: a create past the cap answers
/// `PermissionLimitExceededException`, which RAM raises when a create "would exceed
/// the maximum number of permissions you can create in each Amazon Web Services
/// Region". It counts the customer managed permissions only — the AWS managed catalog
/// the store is seeded with is not something a caller created.
pub const MAX_PERMISSIONS: usize = 500;

/// The longest policy template this emulator reads and stores. Emulator resource
/// guard: the modelled `Policy` shape carries no length constraint, so this bounds
/// both what one call can store and what the JSON reader is handed.
pub const MAX_POLICY_TEMPLATE_LEN: usize = 10_240;

/// The message every refusal of a policy document that cannot be read as a policy
/// statement carries.
///
/// RAM words this one refusal class with a single fixed sentence that interpolates
/// nothing from the request: the error class's own code (`InvalidPolicyException`'s
/// `awsQueryError` code is `InvalidPolicy`) ahead of the reason the document was not
/// read. Every operation that takes a `policyTemplate` reports it from here.
pub const INVALID_POLICY_FORMAT_MESSAGE: &str = "InvalidPolicy: Invalid policy format.";

/// One entry of this service's resource-type catalog.
#[derive(Debug, Clone, Copy)]
pub struct ResourceTypeEntry {
    /// The display name `ListResourceTypes` reports:
    /// `<service-code>:<resource-code>`.
    pub resource_type: &'static str,
    /// Whether a CUSTOMER MANAGED permission can be created for this resource type.
    /// A resource type that cannot still shares, through the managed permissions
    /// Amazon Web Services maintains for it.
    pub customer_managed_permissions: bool,
}

/// The resource types this service can share, with the display name
/// `ListResourceTypes` reports for each and whether a customer managed permission can
/// be created for it.
///
/// Written out here because `CreatePermission$resourceType` is checked against it
/// ("To see the list of valid values for this parameter, query the ListResourceTypes
/// operation. This value must match the display name of the resource (available in
/// ListResourceTypes)"), and the model publishes no shape enumerating them. Source:
/// the resource types AWS Resource Access Manager documents as shareable, one entry
/// per `(service, resource)` pair it names, with the support for customer managed
/// permissions that documentation records for each — the Amazon EC2, Amazon VPC,
/// Amazon API Gateway and Amazon Web Services Private CA resources there are shared
/// only through the permissions Amazon Web Services manages for them.
///
/// ONE ENTRY PER TYPE. `Resource$type` is `<service-code>:<resource-code>` where the
/// service code is the one the owning service's own ARNs carry, so a type is published
/// once, under that code: a second spelling under a service code no ARN names would
/// publish a type nothing can be shared as — no resource ARN classifies to it
/// ([`is_shareable_resource_arn`]) and no managed permission is published for it
/// ([`default_aws_managed_permission`]). EC2 Image Builder's resources, for one, are
/// `imagebuilder:*`, the code its ARNs and its managed permissions carry. Where the
/// owning service publishes a DIFFERENT resource code for a related resource — a later
/// version of an earlier type, or a resource whose own ARN carries a plural code — that
/// is a type of its own and takes its own entry, because the managed permissions
/// published for it carry that spelling.
///
/// A custom billing view is one of the types: the Billing service declares no
/// operation that attaches a resource-based policy to a view, and the document its
/// `GetResourcePolicy` reports is the one a share of the view instantiates on it
/// ([`ComAmazonawsRamAmazonresourcesharingState::resource_policies`]). Its entry
/// records support for a customer managed permission, as every type this service
/// publishes without the one documented exception above does \u2014 a custom billing
/// view is not one of the Amazon EC2, Amazon VPC, Amazon API Gateway or Amazon Web
/// Services Private CA resources that are shared only through the permissions
/// Amazon Web Services manages.
pub const RESOURCE_TYPES: &[ResourceTypeEntry] = &[
    resource_type_entry("acm-pca:CertificateAuthority", false),
    resource_type_entry("agent-registry:Registry", true),
    resource_type_entry("apigateway:Domainnames", false),
    resource_type_entry("apigateway:Portalproducts", false),
    resource_type_entry("appmesh:Mesh", true),
    resource_type_entry("appmesh-preview:Mesh", true),
    resource_type_entry("appsync:Apis", true),
    resource_type_entry("backup:BackupVault", true),
    resource_type_entry("bedrock:CustomModel", true),
    resource_type_entry("billing:BillingView", true),
    resource_type_entry("cloudhsm:Backup", true),
    resource_type_entry("cloudtrail:EventDataStore", true),
    resource_type_entry("codebuild:Project", true),
    resource_type_entry("codebuild:ReportGroup", true),
    resource_type_entry("codeconnections:Connection", true),
    resource_type_entry("datazone:Domain", true),
    resource_type_entry("ec2:CapacityReservation", false),
    resource_type_entry("ec2:CoipPool", false),
    resource_type_entry("ec2:DedicatedHost", false),
    resource_type_entry("ec2:Ipam", false),
    resource_type_entry("ec2:IpamPool", false),
    resource_type_entry("ec2:IpamResourceDiscovery", false),
    resource_type_entry("ec2:LocalGatewayRouteTable", false),
    resource_type_entry("ec2:NetworkInsightsAccessScope", false),
    resource_type_entry("ec2:PlacementGroup", false),
    resource_type_entry("ec2:PrefixList", false),
    resource_type_entry("ec2:SecurityGroup", false),
    resource_type_entry("ec2:Subnet", false),
    resource_type_entry("ec2:TrafficMirrorTarget", false),
    resource_type_entry("ec2:TransitGateway", false),
    resource_type_entry("ec2:TransitGatewayMulticastDomain", false),
    resource_type_entry("ec2:VerifiedAccessGroup", false),
    resource_type_entry("ec2:VerifiedAccessInstance", false),
    resource_type_entry("ec2:Volume", false),
    resource_type_entry("elasticloadbalancing:TrustStore", true),
    resource_type_entry("events:EventBus", true),
    resource_type_entry("events:EventBusv2", true),
    resource_type_entry("fsx:Volume", true),
    resource_type_entry("glue:Catalog", true),
    resource_type_entry("glue:Database", true),
    resource_type_entry("glue:Table", true),
    resource_type_entry("imagebuilder:Component", true),
    resource_type_entry("imagebuilder:ContainerRecipe", true),
    resource_type_entry("imagebuilder:Image", true),
    resource_type_entry("imagebuilder:ImageRecipe", true),
    resource_type_entry("license-manager:LicenseConfiguration", true),
    resource_type_entry("network-firewall:ContainerAssociation", true),
    resource_type_entry("network-firewall:Firewall", true),
    resource_type_entry("network-firewall:FirewallPolicy", true),
    resource_type_entry("network-firewall:StatefulRulegroup", true),
    resource_type_entry("network-firewall:StatelessRulegroup", true),
    resource_type_entry("network-firewall:TLSInspectionConfiguration", true),
    resource_type_entry("networkmanager:CoreNetwork", false),
    resource_type_entry("networkmanager:GlobalNetwork", false),
    resource_type_entry("odb:CloudExadataInfrastructure", true),
    resource_type_entry("odb:ExascaleDbStorageVault", true),
    resource_type_entry("odb:OdbNetwork", true),
    resource_type_entry("outposts:Outpost", true),
    resource_type_entry("outposts:Site", true),
    resource_type_entry("rds:Cluster", true),
    resource_type_entry("refactor-spaces:Environment", true),
    resource_type_entry("resource-explorer-2:View", true),
    resource_type_entry("resource-groups:Group", true),
    resource_type_entry("route53profiles:Profile", true),
    resource_type_entry("route53resolver:FirewallRuleGroup", true),
    resource_type_entry("route53resolver:ResolverQueryLogConfig", true),
    resource_type_entry("route53resolver:ResolverRule", true),
    resource_type_entry("s3:AccessGrants", true),
    resource_type_entry("s3:AccessGrantsInstance", true),
    resource_type_entry("s3-outposts:AccessPoint", true),
    resource_type_entry("s3-outposts:Bucket", true),
    resource_type_entry("s3-outposts:Endpoint", true),
    resource_type_entry("s3-outposts:Outpost", true),
    resource_type_entry("sagemaker:FeatureGroup", true),
    resource_type_entry("sagemaker:Hub", true),
    resource_type_entry("sagemaker:LineageGroup", true),
    resource_type_entry("sagemaker:MlflowApp", true),
    resource_type_entry("sagemaker:MlflowTrackingServer", true),
    resource_type_entry("sagemaker:ModelCard", true),
    resource_type_entry("sagemaker:ModelPackageGroup", true),
    resource_type_entry("sagemaker:PartnerApp", true),
    resource_type_entry("sagemaker:Pipeline", true),
    resource_type_entry("sagemaker:SagemakerCatalog", true),
    resource_type_entry("servicecatalog:Application", true),
    resource_type_entry("servicecatalog:Applications", true),
    resource_type_entry("servicecatalog:AttributeGroup", true),
    resource_type_entry("servicecatalog:AttributeGroups", true),
    resource_type_entry("servicediscovery:Namespace", true),
    resource_type_entry("sms-voice:OptOutList", true),
    resource_type_entry("sms-voice:PhoneNumber", true),
    resource_type_entry("sms-voice:Pool", true),
    resource_type_entry("sms-voice:RcsAgent", true),
    resource_type_entry("sms-voice:SenderId", true),
    resource_type_entry("ssm:Document", true),
    resource_type_entry("ssm:ManagedInstance", true),
    resource_type_entry("ssm:Parameter", true),
    resource_type_entry("ssm-contacts:Contact", true),
    resource_type_entry("ssm-contacts:Rotation", true),
    resource_type_entry("ssm-incidents:ResponsePlan", true),
    resource_type_entry("vpc-lattice:ResourceConfiguration", true),
    resource_type_entry("vpc-lattice:Service", true),
    resource_type_entry("vpc-lattice:ServiceNetwork", true),
];

/// One catalog entry, so [`RESOURCE_TYPES`] reads as the table it is.
const fn resource_type_entry(
    resource_type: &'static str,
    customer_managed_permissions: bool,
) -> ResourceTypeEntry {
    ResourceTypeEntry {
        resource_type,
        customer_managed_permissions,
    }
}

/// The resource types of [`RESOURCE_TYPES`] whose resources are reachable from any
/// Amazon Web Services Region.
///
/// `ResourceRegionScope` tells the two apart by where a resource can be reached from —
/// `GLOBAL` "from any Amazon Web Services Region", `REGIONAL` "only by using requests
/// that target the Amazon Web Services Region in which the resource exists" — and a
/// resource reachable from anywhere names no Region in its own ARN, which is the same
/// test [`resource_region_scope`] applies to a resource ARN. Source: the ARN format each
/// owning service documents for the type. The entries below are the catalog's types
/// whose ARN leaves the Region field empty — a global network and a core network
/// (`arn:<partition>:networkmanager::<account>:core-network/<id>`), an IP Address
/// Manager, one of its pools and one of its resource discoveries
/// (`arn:<partition>:ec2::<account>:ipam/<id>`), and a billing view
/// (`arn:<partition>:billing::<account>:billingview/<id>`, the form the Billing
/// model's own `BillingViewArn` pattern declares); every other type in the catalog
/// names its resources inside one Region.
const GLOBAL_SCOPE_RESOURCE_TYPES: &[&str] = &[
    "billing:BillingView",
    "ec2:Ipam",
    "ec2:IpamPool",
    "ec2:IpamResourceDiscovery",
    "networkmanager:CoreNetwork",
    "networkmanager:GlobalNetwork",
];

/// The `ResourceRegionScope` of the resources of `resource_type`.
///
/// A property of the type rather than of any one resource of it, so it is read off the
/// catalog ([`GLOBAL_SCOPE_RESOURCE_TYPES`]) the way `ListResourceTypes` reports it
/// beside the type, and matched the way every other spelling of a resource type is
/// ([`resource_type_matches`]).
pub fn resource_type_region_scope(resource_type: &str) -> &'static str {
    let global = GLOBAL_SCOPE_RESOURCE_TYPES
        .iter()
        .any(|global| resource_type_matches(global, resource_type));
    if global {
        RESOURCE_REGION_SCOPE_GLOBAL
    } else {
        RESOURCE_REGION_SCOPE_REGIONAL
    }
}

/// Separates the fields of a composite page cursor or fingerprint input. A control
/// character, so it cannot occur inside an ARN, a name or a principal id.
const FIELD_SEPARATOR: char = '\u{1}';

/// How an association is keyed: `(resource share ARN, association type, associated
/// entity)`. One entity is associated with one share under one type at most once, so
/// this key is the association's identity as well as its page order.
pub type AssociationKey = (String, String, String);

/// One stored resource share: every member `ResourceShare` reports, plus the managed
/// permissions attached to it.
#[derive(Debug, Clone, Default)]
pub struct ShareRecord {
    /// The share's ARN: this record's store key.
    pub arn: String,
    pub name: String,
    pub owning_account_id: String,
    pub allow_external_principals: bool,
    pub status: String,
    pub status_message: Option<String>,
    /// Tags keyed by tag key, so a key is carried at most once and `tags` reports a
    /// stable order.
    pub tags: BTreeMap<String, String>,
    /// Epoch milliseconds; rendered as the modelled `DateTime` timestamps.
    pub creation_time: i64,
    pub last_updated_time: i64,
    pub feature_set: String,
    /// The managed permissions attached to this share BY NAME, keyed by permission
    /// ARN: the ones the create request named plus the ones
    /// `AssociateResourceSharePermission` attached afterwards, less the ones an attach
    /// replaced. `GetResourceShares`' `permissionArn` filter reads these, and
    /// [`ComAmazonawsRamAmazonresourcesharingState::share_permissions`] projects them
    /// beside the default permission of every resource type the share shares that none
    /// of them applies to.
    pub attached_permissions: BTreeMap<String, SharePermissionRecord>,
    /// The ARNs of the managed permissions `DisassociateResourceSharePermission` has
    /// REMOVED from this share, so the share is no longer governed by one of them even
    /// when it is the published default of a resource type the share shares and would
    /// otherwise be derived back onto it
    /// ([`ComAmazonawsRamAmazonresourcesharingState::share_permissions`]). An attach of
    /// the permission, and a resource of its type newly shared — which is when RAM
    /// attaches the type's default permission — put it back.
    pub removed_permissions: BTreeSet<String>,
    /// The share's configuration, always stored: a create that names none stores
    /// the service defaults, so every read reports the structure.
    pub resource_share_configuration: ResourceShareConfiguration,
}

impl ShareRecord {
    /// This record as the modelled `ResourceShare` wire shape.
    pub fn view(&self) -> ResourceShare {
        ResourceShare {
            resource_share_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            owning_account_id: Some(self.owning_account_id.clone()),
            allow_external_principals: Some(self.allow_external_principals),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            // `tags` reports the share's tag set, which is a list that may be
            // empty: a share carrying no tags reports `[]`, so a caller can tell
            // "no tags" from "tags not reported".
            tags: Some(tag_list(&self.tags)),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
            feature_set: Some(self.feature_set.clone()),
            resource_share_configuration: Some(self.resource_share_configuration.clone()),
        }
    }

    /// Whether `DeleteResourceShare` has already removed this share.
    pub fn is_deleted(&self) -> bool {
        self.status == SHARE_STATUS_DELETED
    }

    /// Attach `tags` to this share, replacing the value of a key it already
    /// carries, and answer whether the stored tag set changed.
    ///
    /// Call it only after [`tags_rejection`] has passed, which is what guarantees
    /// every pair has a key.
    pub fn merge_tags(&mut self, tags: &[Tag]) -> bool {
        let mut changed = false;
        for (key, value) in tag_map(tags) {
            if self.tags.insert(key, value.clone()).as_ref() != Some(&value) {
                changed = true;
            }
        }
        changed
    }

    /// Detach every key in `keys` from this share, and answer the keys that were
    /// actually carried and so were removed, in key order. A key the share does not
    /// carry is not an error: removing it leaves the share exactly as it was, and it
    /// is not reported as removed — which is what lets a caller tell an untag that
    /// was a no-op from one that detached a tag.
    pub fn remove_tags(&mut self, keys: &[String]) -> Vec<String> {
        // Every key is removed, not just up to the first that was present, so this
        // collects the removals rather than short-circuiting on one. A key named
        // twice is removed once and reported once.
        keys.iter()
            .filter(|key| self.tags.remove(*key).is_some())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// A status move the clock owes a record: the status it reaches, and the instant it
/// falls due.
///
/// RAM answers the call that starts the move with the record in a NON-TERMINAL status
/// (`ASSOCIATING` while an association is being made, `DISASSOCIATING` while one is
/// being torn down), so reaching the terminal status is something the clock owes and
/// not something a caller asks for. The due instant is stored and nothing sleeps for
/// it: [`ComAmazonawsRamAmazonresourcesharingState::settle_due_associations`]
/// materialises the move on the first call at or after it.
#[derive(Debug, Clone)]
pub struct OwedMove {
    /// The status the record reaches when the move falls due.
    pub status: String,
    /// When it falls due, in epoch milliseconds.
    pub due_at: i64,
}

/// One stored association between a resource share and an entity.
#[derive(Debug, Clone, Default)]
pub struct AssociationRecord {
    pub resource_share_arn: String,
    pub resource_share_name: String,
    pub associated_entity: String,
    pub association_type: String,
    pub status: String,
    pub status_message: Option<String>,
    /// Epoch milliseconds, as on [`ShareRecord`].
    pub creation_time: i64,
    pub last_updated_time: i64,
    pub external: bool,
    /// The move the clock still owes this record, or `None` when its status is
    /// terminal and nothing further is owed.
    pub owed_move: Option<OwedMove>,
}

impl AssociationRecord {
    /// This record as the modelled `ResourceShareAssociation` wire shape.
    pub fn view(&self) -> ResourceShareAssociation {
        ResourceShareAssociation {
            resource_share_arn: Some(self.resource_share_arn.clone()),
            resource_share_name: Some(self.resource_share_name.clone()),
            associated_entity: Some(self.associated_entity.clone()),
            association_type: Some(self.association_type.clone()),
            status: Some(self.status.clone()),
            // A message about the status only belongs on a record whose status is
            // still moving: a terminal association is waiting for nothing, so
            // whatever it was waiting for no longer applies and the member is
            // dropped. The store clears the message when it makes the move, and this
            // is the same rule at the serializer, so no read can report a stale one.
            status_message: self.status_message.clone().filter(|_| !self.is_terminal()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
            external: Some(self.external),
        }
    }

    /// This record as the modelled `ResourceShareAssociation` wire shape, reported by
    /// the operation that REMOVES the association from the share.
    ///
    /// `DisassociateResourceShare` "removes the specified principals, resources, or
    /// source constraints from participating in the specified resource share" (the
    /// operation's own documentation) and answers with the records it removed. On those
    /// records the organization-membership flag is false whatever the entity is: the
    /// call leaves no principal participating in the share, so there is none to place
    /// in the owning account's organization. Every operation that reports an
    /// association still participating in the share — `AssociateResourceShare`,
    /// `GetResourceShareAssociations`, and the list a `ResourceShareInvitation` carries
    /// — reports [`Self::view`], with the flag derived from the entity and the share's
    /// owning account, so the stored derivation is untouched by a teardown.
    pub fn removal_view(&self) -> ResourceShareAssociation {
        ResourceShareAssociation {
            external: Some(false),
            ..self.view()
        }
    }

    /// Whether this record's status is one no further move is owed from.
    pub fn is_terminal(&self) -> bool {
        association_status_is_terminal(&self.status)
    }
}

/// One stored invitation for an account to join a resource share.
///
/// RAM shares with an account OUTSIDE the owner's organization by invitation: the
/// invited account joins the share only once it accepts
/// (`AcceptResourceShareInvitation`/`RejectResourceShareInvitation`), so the
/// invitation is a resource of its own, keyed by its ARN, and the record is what
/// says whether it is still unanswered.
#[derive(Debug, Clone, Default)]
pub struct InvitationRecord {
    /// The invitation's ARN: this record's store key.
    pub arn: String,
    /// The share the invitation is to.
    pub resource_share_arn: String,
    pub resource_share_name: String,
    /// The account that owns the share, which is the one that sent the invitation.
    pub sender_account_id: String,
    /// The account that was invited.
    pub receiver_account_id: String,
    /// The IAM principal that was invited, when the association named one rather
    /// than a whole account.
    pub receiver_arn: Option<String>,
    /// Epoch milliseconds, as on [`ShareRecord`].
    pub invitation_timestamp: i64,
    pub status: String,
    /// When a still-`PENDING` invitation expires, in epoch milliseconds:
    /// [`INVITATION_EXPIRES_AFTER_MILLIS`] after it was sent. The move to `EXPIRED`
    /// is one the clock owes, so the instant is stored and
    /// [`ComAmazonawsRamAmazonresourcesharingState::settle_due_invitations`]
    /// materialises it on the first call at or after it.
    pub expires_at: i64,
}

impl InvitationRecord {
    /// This record as the modelled `ResourceShareInvitation` wire shape.
    pub fn view(&self) -> ResourceShareInvitation {
        ResourceShareInvitation {
            resource_share_invitation_arn: Some(self.arn.clone()),
            resource_share_name: Some(self.resource_share_name.clone()),
            resource_share_arn: Some(self.resource_share_arn.clone()),
            sender_account_id: Some(self.sender_account_id.clone()),
            receiver_account_id: Some(self.receiver_account_id.clone()),
            invitation_timestamp: Some(to_date_time(self.invitation_timestamp)),
            status: Some(self.status.clone()),
            // The member is deprecated in favour of `ListPendingInvitationResources`
            // ("To view the resources associated with a pending resource share
            // invitation, use ListPendingInvitationResources"), so an invitation
            // reports no associations and a caller is pointed at that operation.
            resource_share_associations: None,
            receiver_arn: self.receiver_arn.clone(),
        }
    }

    /// Whether this invitation is still unanswered and unexpired \u2014 the state the
    /// resources of its share are \"pending\" in.
    pub fn is_pending(&self) -> bool {
        self.status == INVITATION_STATUS_PENDING
    }
}

/// One version of a stored managed permission: the policy template it carries and
/// when it was written.
///
/// A permission's versions are its own records because every read of a permission
/// names one — `GetPermission` "retrieves the default version" unless the request
/// asks for another, and each version carries its own policy document and status.
#[derive(Debug, Clone, Default)]
pub struct PermissionVersionRecord {
    /// This version's number, counting from [`FIRST_PERMISSION_VERSION`]; the key
    /// this record is stored under.
    pub version: i32,
    /// The policy template this version was created with, stored exactly as the
    /// request wrote it so a read reports the document that was stored.
    pub policy_template: String,
    /// A `PermissionStatus`: whether this version can be associated with shares.
    pub status: String,
    /// Epoch milliseconds; rendered as the modelled `DateTime` timestamps.
    pub creation_time: i64,
    pub last_updated_time: i64,
    /// Where the delete of THIS version has got to, or `None` for a version nobody has
    /// deleted ([`permission_version_deletion`]).
    pub deletion: Option<PermissionDeletion>,
}

/// One stored managed permission, keyed by its ARN, with every version written for
/// it.
#[derive(Debug, Clone, Default)]
pub struct PermissionRecord {
    /// The permission's ARN: this record's store key.
    pub arn: String,
    pub name: String,
    /// The resource type this permission applies to,
    /// `<service-code>:<resource-code>`.
    pub resource_type: String,
    /// A `PermissionType`: who created and maintains this permission.
    pub permission_type: String,
    /// A `PermissionFeatureSet`.
    pub feature_set: String,
    /// Tags keyed by tag key, so a key is carried at most once and `tags` reports a
    /// stable order.
    pub tags: BTreeMap<String, String>,
    /// Epoch milliseconds; rendered as the modelled `DateTime` timestamps.
    pub creation_time: i64,
    pub last_updated_time: i64,
    /// The version a read that names none reports.
    pub default_version: i32,
    /// Whether this permission is "the default managed permission for all resources
    /// of this resource type" (`ResourceSharePermissionSummary$isResourceTypeDefault`),
    /// which is a property of the permission rather than of one of its versions.
    pub is_resource_type_default: bool,
    /// Whether `UpdatePermission` has deprecated this permission, so every version of
    /// it reports [`PERMISSION_STATUS_DEPRECATED`] and no new resource share
    /// association may be made with it ([`permission_deprecation`]).
    pub deprecated: bool,
    /// Every version written for this permission, keyed by version number: the key
    /// order is total and stable, so a paginated read over versions has a page order.
    pub versions: BTreeMap<i32, PermissionVersionRecord>,
    /// Where the delete of this whole permission has got to, or `None` for a
    /// permission nobody has deleted ([`permission_deletion`]).
    pub deletion: Option<PermissionDeletion>,
}

impl PermissionRecord {
    /// The version a request naming `version` is about: the one it asked for, or
    /// this permission's default version when it asked for none. `None` when no such
    /// version was ever written.
    pub fn version(&self, version: Option<i32>) -> Option<&PermissionVersionRecord> {
        self.versions
            .get(&version.unwrap_or(self.default_version))
    }

    /// One version of this permission as the modelled `ResourceSharePermissionSummary`
    /// wire shape.
    pub fn summary(&self, version: &PermissionVersionRecord) -> ResourceSharePermissionSummary {
        ResourceSharePermissionSummary {
            arn: Some(self.arn.clone()),
            // The version is reported as a string, which is what the member's target
            // shape is, even though a request names it as an integer.
            version: Some(version.version.to_string()),
            default_version: Some(version.version == self.default_version),
            name: Some(self.name.clone()),
            resource_type: Some(self.resource_type.clone()),
            status: Some(self.version_status(version)),
            // The times are the version's own: what a summary describes is one
            // version of the permission.
            creation_time: Some(to_date_time(version.creation_time)),
            last_updated_time: Some(to_date_time(version.last_updated_time)),
            is_resource_type_default: Some(self.is_resource_type_default),
            permission_type: Some(self.permission_type.clone()),
            feature_set: Some(self.feature_set.clone()),
            // A permission carrying no tags reports `[]`, so a caller can tell "no
            // tags" from "tags not reported".
            tags: Some(tag_list(&self.tags)),
            // Members the model declares and this slice holds no state behind: a
            // permission here declares no template parameters, and the owner of a
            // permission this account can read is reported by the reads that know it.
            permission_template_parameters: None,
            resource_owner: None,
        }
    }

    /// One version of this permission as the modelled `ResourceSharePermissionDetail`
    /// wire shape: the summary's members plus the version's policy document.
    pub fn detail(&self, version: &PermissionVersionRecord) -> ResourceSharePermissionDetail {
        ResourceSharePermissionDetail {
            arn: Some(self.arn.clone()),
            version: Some(version.version.to_string()),
            default_version: Some(version.version == self.default_version),
            name: Some(self.name.clone()),
            resource_type: Some(self.resource_type.clone()),
            permission: Some(version.policy_template.clone()),
            creation_time: Some(to_date_time(version.creation_time)),
            last_updated_time: Some(to_date_time(version.last_updated_time)),
            is_resource_type_default: Some(self.is_resource_type_default),
            permission_type: Some(self.permission_type.clone()),
            feature_set: Some(self.feature_set.clone()),
            status: Some(self.version_status(version)),
            tags: Some(tag_list(&self.tags)),
            // Members the model declares and this slice holds no state behind: a
            // permission here declares no template parameters, and the owner of a
            // permission this account can read is reported by the reads that know it.
            permission_template_parameters: None,
            resource_owner: None,
        }
    }
}

/// What one `(operation, clientToken)` pair already did, so repeating the request
/// repeats the answer instead of acting twice.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// A digest of every other parameter of the first call that used this token.
    /// A later call with the same token and a different digest is the
    /// `IdempotentParameterMismatchException` case.
    pub fingerprint: String,
    /// What the first call acted on, so the repeat can answer from the store rather
    /// than from a cached body: the ARN of a resource share or a managed permission,
    /// or — where the call started one — the id of the background task it recorded
    /// ([`ReplaceWorkRecord`]).
    pub resource_arn: String,
}

/// Per-service state for com-amazonaws-ram-amazonresourcesharing. A handler reaches
/// it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRamAmazonresourcesharingState {
    /// Every resource share, live or tombstoned, keyed by its ARN.
    pub resource_shares: BTreeMap<String, ShareRecord>,
    /// Every association, keyed by [`AssociationKey`].
    pub associations: BTreeMap<AssociationKey, AssociationRecord>,
    /// Every invitation this service has sent, keyed by its ARN. An invitation is
    /// created only beside a principal association, dropped with its share, and
    /// capped per share by [`MAX_INVITATIONS_PER_SHARE`].
    pub invitations: BTreeMap<String, InvitationRecord>,
    /// Every managed permission this service holds, keyed by its ARN: the ones this
    /// account created and the AWS managed catalog RAM ships, which a fresh store
    /// already carries ([`aws_managed_permissions::catalog_records`]). A permission's
    /// name is unique within a Region and its ARN carries that name, so one name in
    /// one Region is one entry here.
    pub permissions: BTreeMap<String, PermissionRecord>,
    /// Every `ReplacePermissionAssociations` background task this service has started,
    /// keyed by the id the call that started it handed back ([`ReplaceWorkRecord`]). A
    /// task is kept after it has run so a read can still report it, and the map is
    /// capped by [`MAX_REPLACE_PERMISSION_ASSOCIATIONS_WORKS`].
    pub replace_permission_associations_works: BTreeMap<String, ReplaceWorkRecord>,
    /// `(operation, clientToken)` -> what that token already did.
    client_tokens: HashMap<(String, String), ClientTokenRecord>,
    /// The order tokens were remembered in, so the oldest can be forgotten at the
    /// cap.
    client_token_order: VecDeque<(String, String)>,
    /// Mixed into each generated id so two calls in the same nanosecond differ.
    id_counter: u64,
    /// The last instant handed out by [`Self::tick`].
    clock: i64,
}

impl Default for ComAmazonawsRamAmazonresourcesharingState {
    /// A fresh store holds no share, association or invitation — and the whole AWS
    /// managed permission catalog, because those permissions are RAM's own and are
    /// there to be read and attached before any caller has created anything.
    fn default() -> Self {
        Self {
            resource_shares: BTreeMap::new(),
            associations: BTreeMap::new(),
            invitations: BTreeMap::new(),
            permissions: aws_managed_permissions::catalog_records(now_millis()),
            replace_permission_associations_works: BTreeMap::new(),
            client_tokens: HashMap::new(),
            client_token_order: VecDeque::new(),
            id_counter: 0,
            clock: 0,
        }
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// This request's instant, in epoch milliseconds.
    ///
    /// The wall clock, nudged forward by a millisecond when two calls land inside
    /// the same one, so the `lastUpdatedTime` an update stores is always strictly
    /// after the one it replaced and the order of two writes is observable.
    pub fn tick(&mut self) -> i64 {
        self.clock = now_millis().max(self.clock + 1);
        self.clock
    }

    /// Make every association move the clock owed at or before `now`.
    ///
    /// This is the only write a reporting operation performs, and it only
    /// materialises what was already due: an association RAM accepted in
    /// `ASSOCIATING` reaches `ASSOCIATED` [`ASSOCIATION_READY_AFTER_MILLIS`] after
    /// the call that created it, and one it accepted in `DISASSOCIATING` reaches
    /// `DISASSOCIATED` [`DISASSOCIATION_READY_AFTER_MILLIS`] after the call that
    /// started the teardown — whether or not anybody asks.
    pub fn settle_due_associations(&mut self, now: i64) {
        for record in self.associations.values_mut() {
            let Some(owed) = record.owed_move.clone() else {
                continue;
            };
            if owed.due_at > now {
                continue;
            }
            record.status = owed.status;
            // A record that has reached its terminal status is waiting for nothing,
            // so it reports no status message.
            record.status_message = None;
            // The move happened when it fell due, not when somebody noticed.
            record.last_updated_time = owed.due_at;
            record.owed_move = None;
        }
    }

    /// Expire every invitation whose answering window the clock has already closed.
    ///
    /// Like [`Self::settle_due_associations`], this only materialises what was
    /// already due: an invitation nobody answered within
    /// [`INVITATION_EXPIRES_AFTER_MILLIS`] of being sent is `EXPIRED` from that
    /// instant on, whether or not anybody asks.
    pub fn settle_due_invitations(&mut self, now: i64) {
        for record in self.invitations.values_mut() {
            if record.is_pending() && record.expires_at <= now {
                record.status = INVITATION_STATUS_EXPIRED.to_string();
            }
        }
    }

    /// The invitation `arn` names, whatever its status.
    pub fn invitation(&self, arn: &str) -> Option<&InvitationRecord> {
        self.invitations.get(arn)
    }

    /// The share `arn` names, whether live or tombstoned.
    pub fn share(&self, arn: &str) -> Option<&ShareRecord> {
        self.resource_shares.get(arn)
    }

    /// The share `arn` names, for an operation the spec says acts on it. A share
    /// `DeleteResourceShare` has removed is gone as far as those operations are
    /// concerned, so this answers `None` for one — the `UnknownResourceException`
    /// case.
    pub fn live_share(&self, arn: &str) -> Option<&ShareRecord> {
        self.resource_shares
            .get(arn)
            .filter(|share| !share.is_deleted())
    }

    /// [`Self::live_share`] for a caller that needs to change the record.
    pub fn live_share_mut(&mut self, arn: &str) -> Option<&mut ShareRecord> {
        self.resource_shares
            .get_mut(arn)
            .filter(|share| !share.is_deleted())
    }

    /// Store a newly created share.
    pub fn insert_share(&mut self, share: ShareRecord) {
        self.resource_shares.insert(share.arn.clone(), share);
    }

    /// The managed permission `arn` names.
    pub fn permission(&self, arn: &str) -> Option<&PermissionRecord> {
        self.permissions.get(arn)
    }

    /// Store a newly created managed permission.
    pub fn insert_permission(&mut self, permission: PermissionRecord) {
        self.permissions
            .insert(permission.arn.clone(), permission);
    }

    /// Write a new version of the managed permission `arn` carrying `policy_template`,
    /// and answer it — or `None` when this store holds no permission under that ARN.
    ///
    /// The new version is numbered one above the highest the permission carries, and it
    /// "is automatically set as the default version of the customer managed permission"
    /// (`CreatePermissionVersion`'s own documentation), so from here on a read that
    /// names no version reports it and a share created naming the permission attaches
    /// it. The versions already written are left exactly as they are — "existing
    /// resource shares continue to use their original permission versions" — so a share
    /// attached to an earlier one keeps reporting that one.
    pub fn add_permission_version(
        &mut self,
        arn: &str,
        policy_template: String,
        now: i64,
    ) -> Option<PermissionVersionRecord> {
        let permission = self.permissions.get_mut(arn)?;
        // One above the highest version ever written for this permission, so a number
        // is never reused even when a version in the middle was deleted.
        let version = permission
            .versions
            .keys()
            .next_back()
            .copied()
            .unwrap_or(FIRST_PERMISSION_VERSION - 1)
            + 1;
        let record = PermissionVersionRecord {
            version,
            policy_template,
            status: PERMISSION_STATUS_ATTACHABLE.to_string(),
            creation_time: now,
            last_updated_time: now,
            // Nothing has deleted a version this call is writing.
            deletion: None,
        };
        permission.versions.insert(version, record.clone());
        permission.default_version = version;
        permission.last_updated_time = now;
        Some(record)
    }

    /// Every resource type the share `arn` names shares, each of them once.
    ///
    /// A resource association SURVIVES being torn down, in a terminal status, and the
    /// managed permission RAM attaches when a resource type is first shared stays
    /// attached to the share until the permission itself is disassociated from it — so
    /// what counts here is that the share has an association of that type at all, not
    /// where that association has got to. A resource whose ARN names no type
    /// ([`resource_type_of_arn`]) contributes none.
    pub fn shared_resource_types(&self, arn: &str) -> Vec<String> {
        let mut types: Vec<String> = self
            .associations
            .values()
            .filter(|record| {
                record.resource_share_arn == arn
                    && record.association_type == ASSOCIATION_TYPE_RESOURCE
            })
            .filter_map(|record| resource_type_of_arn(&record.associated_entity))
            .collect();
        types.sort_unstable();
        types.dedup();
        types
    }

    /// How many managed permissions this account has CREATED, which is what the
    /// create cap counts: the AWS managed catalog the store is seeded with was created
    /// by Amazon Web Services, not by a caller, so it does not count against what a
    /// caller may create.
    ///
    /// A permission a delete has removed still counts: its record survives the delete
    /// so a caller can read the deletion back, and nothing reclaims it, so it is a
    /// record this account's creates put in the store like any other. That is what
    /// keeps the store finite under a create-and-delete loop.
    pub fn customer_managed_permission_count(&self) -> usize {
        self.permissions
            .values()
            .filter(|permission| permission.permission_type == PERMISSION_TYPE_CUSTOMER_MANAGED)
            .count()
    }

    /// How many associations the share `arn` holds, which the per-share cap counts.
    pub fn association_count(&self, arn: &str) -> usize {
        self.associations
            .keys()
            .filter(|(share_arn, _, _)| share_arn == arn)
            .count()
    }

    /// Whether the share `share_arn` currently associates `entity` under
    /// `association_type`: the association exists and is on the attach path
    /// ([`association_is_attaching`]).
    pub fn associates(&self, share_arn: &str, association_type: &str, entity: &str) -> bool {
        self.associations
            .get(&(
                share_arn.to_string(),
                association_type.to_string(),
                entity.to_string(),
            ))
            .is_some_and(|record| association_is_attaching(&record.status))
    }

    /// Every live share that currently shares the resource `resource_arn`, in share
    /// ARN order.
    ///
    /// A share shares a resource while its resource association is on the attach
    /// path ([`association_is_attaching`]). An association that was torn down shares
    /// nothing, and neither does a share `DeleteResourceShare` has removed, so
    /// neither is reported.
    pub fn shares_sharing_resource(&self, resource_arn: &str) -> Vec<&ShareRecord> {
        self.associations
            .values()
            .filter(|record| {
                record.association_type == ASSOCIATION_TYPE_RESOURCE
                    && record.associated_entity == resource_arn
                    && association_is_attaching(&record.status)
            })
            .filter_map(|record| self.live_share(&record.resource_share_arn))
            .collect()
    }

    /// The resource-based policy documents to report for the shared resource
    /// `resource_arn`, narrowed to the shares that share it with `principal` when a
    /// read names one — or `None` when no live share shares that resource at all,
    /// which is the case a read refuses: policies are reported only for the
    /// resources the caller "own[s] and ha[s] shared".
    ///
    /// RAM keeps no policy document of its own. One document per share that shares
    /// the resource, in share ARN order: RAM instantiates the policy on each shared
    /// resource from the share that shares it ([`Self::instantiated_policy`]), so a
    /// resource two shares share carries one document from each, and a share that
    /// grants no identity outside the owner's organization contributes none.
    pub fn resource_policies(
        &self,
        resource_arn: &str,
        principal: Option<&str>,
    ) -> Option<Vec<String>> {
        let shares = self.shares_sharing_resource(resource_arn);
        if shares.is_empty() {
            return None;
        }
        Some(
            shares
                .into_iter()
                .filter(|share| {
                    // A read that names a principal asks for the policy that
                    // principal has on the resource, so only a share that shares the
                    // resource WITH it can contribute one.
                    principal.is_none_or(|principal| {
                        self.associates(&share.arn, ASSOCIATION_TYPE_PRINCIPAL, principal)
                    })
                })
                .filter_map(|share| self.instantiated_policy(share, resource_arn))
                .collect(),
        )
    }

    /// The document the managed permission attached to `share` instantiates on the
    /// resource `resource_arn`, or `None` when the share names no identity that a
    /// resource policy has to grant.
    ///
    /// RAM keeps no policy document of its own: the policy on a shared resource is
    /// the INSTANTIATION of the managed permission attached to the share, and RAM
    /// fills in the two elements the permission's template may not carry —
    /// `Resource` "comes from the ARN of the specific resource that you are sharing"
    /// and `Principal` "comes from the list of identities added to the resource
    /// share" (`CreatePermission$policyTemplate`'s own documentation), the template
    /// supplying the allow `Effect` and the actions.
    ///
    /// Only an identity OUTSIDE the owning account's organization needs such a
    /// grant: one inside it reaches the resource through Organizations, so a share
    /// that adds none leaves its resources carrying no policy at all — which is what
    /// a share without an external principal reports.
    ///
    /// The statement's actions are the ones the managed permission allows. This
    /// service stores the ARNs of a share's permissions and nothing about the
    /// permissions themselves, so the statement names the service namespace those
    /// actions are drawn from — the shared resource's own, read off its ARN — rather
    /// than a list of actions nothing told it about.
    fn instantiated_policy(&self, share: &ShareRecord, resource_arn: &str) -> Option<String> {
        let principals = self.external_principals(&share.arn);
        if principals.is_empty() {
            return None;
        }
        let service = arn_service(resource_arn)?;
        Some(
            ::serde_json::json!({
                "Version": POLICY_LANGUAGE_VERSION,
                "Statement": [{
                    "Effect": POLICY_EFFECT_ALLOW,
                    "Principal": {"AWS": principals},
                    "Action": format!("{service}:{POLICY_ACTION_WILDCARD}"),
                    "Resource": resource_arn,
                }],
            })
            .to_string(),
        )
    }

    /// The identities outside the owning account's organization that the share
    /// `share_arn` is shared with, in association order and each of them once.
    ///
    /// `ResourceShareAssociation$external` "indicates whether the principal belongs
    /// to the same organization in Organizations as the Amazon Web Services account
    /// that owns the resource share", and [`entity_is_external`] is what decided it
    /// when the association was stored, so this reads the stored flag rather than
    /// deciding it again.
    pub fn external_principals(&self, share_arn: &str) -> Vec<String> {
        self.associations
            .values()
            .filter(|record| {
                record.resource_share_arn == share_arn
                    && record.association_type == ASSOCIATION_TYPE_PRINCIPAL
                    && record.external
                    && association_is_attaching(&record.status)
            })
            .map(|record| record.associated_entity.clone())
            .collect()
    }

    /// Associate `entity` with the share under `association_type`, returning the
    /// record as it now stands.
    ///
    /// A new association starts in the non-terminal `ASSOCIATING` status, and a
    /// principal association says in its `statusMessage` that it is waiting for the
    /// invitation to be answered ([`associating_status_message`]). The move to
    /// `ASSOCIATED` is one the clock owes, [`ASSOCIATION_READY_AFTER_MILLIS`] after
    /// this call; [`Self::settle_due_associations`] materialises it and drops the
    /// message with it.
    ///
    /// An entity already on the attach path — `ASSOCIATING` or `ASSOCIATED` — is left
    /// exactly as it is: re-associating it changes nothing, so neither its status nor
    /// its timestamps move, except for the share's name, which an update may have
    /// changed since. An entity whose association was torn down (or whose teardown is
    /// still in flight) is associated AGAIN: the record restarts the attach, so the
    /// status leaves `DISASSOCIATED`/`DISASSOCIATING`, the owed teardown is dropped
    /// and the record is dated from this call.
    pub fn associate(
        &mut self,
        share: &ShareRecord,
        association_type: &str,
        entity: &str,
        now: i64,
    ) -> AssociationRecord {
        let key = (
            share.arn.clone(),
            association_type.to_string(),
            entity.to_string(),
        );
        let attaching = |record: &AssociationRecord| association_is_attaching(&record.status);
        // Whether THIS call is what starts the attach, which is what decides whether
        // a principal is being invited now or was already on its way in.
        let started = self
            .associations
            .get(&key)
            .is_none_or(|record| !attaching(record));
        let record = self
            .associations
            .entry(key)
            .or_insert_with(|| AssociationRecord {
                resource_share_arn: share.arn.clone(),
                resource_share_name: share.name.clone(),
                associated_entity: entity.to_string(),
                association_type: association_type.to_string(),
                status: String::new(),
                status_message: None,
                creation_time: now,
                last_updated_time: now,
                external: entity_is_external(&share.owning_account_id, entity),
                owed_move: None,
            });
        if !attaching(record) {
            // Either the record was just created, or it had left the attach path; in
            // both cases this call is what associates the entity, so the attach
            // starts here.
            record.status = ASSOCIATION_STATUS_ASSOCIATING.to_string();
            record.status_message = associating_status_message(association_type).map(str::to_string);
            record.creation_time = now;
            record.last_updated_time = now;
            record.external = entity_is_external(&share.owning_account_id, entity);
            record.owed_move = Some(OwedMove {
                status: ASSOCIATION_STATUS_ASSOCIATED.to_string(),
                due_at: now + ASSOCIATION_READY_AFTER_MILLIS,
            });
        }
        record.resource_share_name = share.name.clone();
        let view = record.clone();
        // Sharing with a principal OUTSIDE the owner's organization invites the
        // account that principal belongs to: the account joins the share only once
        // it accepts. A principal inside the organization, and any entity that is
        // not a principal, is associated directly and invites nobody.
        if started && association_type == ASSOCIATION_TYPE_PRINCIPAL && view.external {
            self.invite(share, entity, now);
        }
        // RAM governs the resources of a type a share shares by that type's default
        // managed permission when the share holds none of its own for the type, so a
        // share that starts sharing a resource of a type whose default it had removed is
        // governed by that default again from this call on.
        if started && association_type == ASSOCIATION_TYPE_RESOURCE {
            self.restore_removed_permissions(&share.arn, entity);
        }
        view
    }

    /// Invite the account `entity` belongs to to join `share`, and answer the
    /// invitation — or `None` when `entity` names no account to invite (a service
    /// principal, say), because there is then nobody to answer one.
    ///
    /// An account that already has an unanswered invitation to this share is not
    /// invited twice: the pending invitation it is already holding is the one it
    /// answers. An account whose previous invitation was answered or expired is
    /// invited again, since this call shares with it afresh.
    fn invite(&mut self, share: &ShareRecord, entity: &str, now: i64) -> Option<String> {
        let receiver = invitation_receiver_account(entity)?.to_string();
        let pending = self.invitations.values().find(|invitation| {
            invitation.resource_share_arn == share.arn
                && invitation.receiver_account_id == receiver
                && invitation.is_pending()
        });
        if let Some(invitation) = pending {
            return Some(invitation.arn.clone());
        }
        // The store is finite: at the cap the share forgets the invitation whose
        // answer is oldest, and one whose invitations are all still unanswered
        // invites nobody further rather than growing past it.
        if !self.make_room_for_invitation(&share.arn) {
            return None;
        }
        // The invitation lives in the same partition, Region and account as the
        // share it invites to: it is a resource of the sending account.
        let region = arn_region(&share.arn)
            .filter(|region| !region.is_empty())
            .unwrap_or(DEFAULT_REGION)
            .to_string();
        let arn = self.fresh_invitation_arn(&region);
        self.invitations.insert(
            arn.clone(),
            InvitationRecord {
                arn: arn.clone(),
                resource_share_arn: share.arn.clone(),
                resource_share_name: share.name.clone(),
                sender_account_id: share.owning_account_id.clone(),
                receiver_account_id: receiver,
                // Only an IAM principal ARN names the user or role that received
                // the invitation; a bare account id names the account alone.
                receiver_arn: invitation_receiver_arn(entity).map(str::to_string),
                invitation_timestamp: now,
                status: INVITATION_STATUS_PENDING.to_string(),
                expires_at: now + INVITATION_EXPIRES_AFTER_MILLIS,
            },
        );
        Some(arn)
    }

    /// Make room for one more invitation to the share `arn` names, and answer whether
    /// there is any: the oldest ANSWERED invitation to that share is forgotten when it
    /// already holds [`MAX_INVITATIONS_PER_SHARE`], and a share holding that many
    /// unanswered ones has no room at all.
    fn make_room_for_invitation(&mut self, arn: &str) -> bool {
        let held: Vec<(i64, String, bool)> = self
            .invitations
            .values()
            .filter(|invitation| invitation.resource_share_arn == arn)
            .map(|invitation| {
                (
                    invitation.invitation_timestamp,
                    invitation.arn.clone(),
                    invitation.is_pending(),
                )
            })
            .collect();
        if held.len() < MAX_INVITATIONS_PER_SHARE {
            return true;
        }
        let oldest_answered = held
            .into_iter()
            .filter(|(_, _, pending)| !pending)
            .min()
            .map(|(_, arn, _)| arn);
        match oldest_answered {
            Some(forgotten) => {
                self.invitations.remove(&forgotten);
                true
            }
            None => false,
        }
    }

    /// Tear the association between the share `arn` and `entity` under
    /// `association_type` down, returning the record as it now stands, or `None` when
    /// the share holds no such association.
    ///
    /// The record SURVIVES the teardown in a terminal status, so a read still reports
    /// the association and a caller polling for the teardown sees it complete. Which
    /// status the call answers with follows from what there was to revoke:
    ///
    /// * an association that had reached `ASSOCIATED` was granting access, so
    ///   revoking it is work the service has to do: the record goes to the
    ///   non-terminal `DISASSOCIATING` and the clock owes `DISASSOCIATED`
    ///   [`DISASSOCIATION_READY_AFTER_MILLIS`] later;
    /// * an association still on the way in (`ASSOCIATING` — a principal that never
    ///   answered its invitation, say) was granting nothing, so there is nothing to
    ///   revoke and the record is terminal at once, in `DISASSOCIATED`;
    /// * a record already torn down, or already tearing down, is left exactly as it
    ///   is, so repeating the call neither restarts the move nor moves the stamps.
    ///
    /// A terminal record reports no `statusMessage`: whatever it was waiting for no
    /// longer applies.
    pub fn disassociate(
        &mut self,
        arn: &str,
        association_type: &str,
        entity: &str,
        now: i64,
    ) -> Option<AssociationRecord> {
        let key = (
            arn.to_string(),
            association_type.to_string(),
            entity.to_string(),
        );
        let record = self.associations.get_mut(&key)?;
        if record.status == ASSOCIATION_STATUS_DISASSOCIATING
            || record.status == ASSOCIATION_STATUS_DISASSOCIATED
        {
            return Some(record.clone());
        }
        let granted = record.status == ASSOCIATION_STATUS_ASSOCIATED;
        record.status_message = None;
        record.last_updated_time = now;
        if granted {
            record.status = ASSOCIATION_STATUS_DISASSOCIATING.to_string();
            record.owed_move = Some(OwedMove {
                status: ASSOCIATION_STATUS_DISASSOCIATED.to_string(),
                due_at: now + DISASSOCIATION_READY_AFTER_MILLIS,
            });
        } else {
            record.status = ASSOCIATION_STATUS_DISASSOCIATED.to_string();
            record.owed_move = None;
        }
        Some(record.clone())
    }

    /// Carry a share's new name onto its associations, which each report
    /// `resourceShareName`.
    pub fn rename_associations(&mut self, arn: &str, name: &str) {
        for (key, record) in &mut self.associations {
            if key.0 == arn {
                record.resource_share_name = name.to_string();
            }
        }
    }

    /// Tombstone the share `arn` names and drop its associations: the share stops
    /// sharing anything, and no later read reports an association of a share that
    /// no longer exists.
    pub fn delete_share(&mut self, arn: &str, now: i64) {
        if let Some(share) = self.resource_shares.get_mut(arn) {
            share.status = SHARE_STATUS_DELETED.to_string();
            share.status_message = None;
            share.last_updated_time = now;
        }
        self.associations.retain(|key, _| key.0 != arn);
        // An invitation to a share that no longer shares anything invites nobody to
        // anything, so no later read reports one.
        self.invitations
            .retain(|_, invitation| invitation.resource_share_arn != arn);
    }

    /// What this `(operation, clientToken)` pair already did, if anything.
    pub fn client_token_record(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<&ClientTokenRecord> {
        self.client_tokens
            .get(&(operation.to_string(), token.to_string()))
    }

    /// Remember what this `(operation, clientToken)` pair did, forgetting the
    /// oldest remembered pair once the cap is reached.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        token: &str,
        fingerprint: String,
        resource_arn: String,
    ) {
        let key = (operation.to_string(), token.to_string());
        if self.client_tokens.contains_key(&key) {
            return;
        }
        while self.client_token_order.len() >= MAX_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
        self.client_token_order.push_back(key.clone());
        self.client_tokens.insert(
            key,
            ClientTokenRecord {
                fingerprint,
                resource_arn,
            },
        );
    }

    /// A resource-share-invitation ARN in `region` that no stored invitation is
    /// using. Built the way [`Self::fresh_share_arn`] builds a share's.
    pub fn fresh_invitation_arn(&mut self, region: &str) -> String {
        for _ in 0..1024 {
            let candidate = invitation_arn(region, &self.fresh_uuid());
            if !self.invitations.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, as on [`Self::fresh_share_arn`]: there are always more of
        // these than there are stored invitations.
        (0u64..=self.invitations.len() as u64)
            .map(|n| invitation_arn(region, &format!("00000000-0000-4000-8000-{n:012x}")))
            .find(|arn| !self.invitations.contains_key(arn))
            .unwrap_or_else(|| invitation_arn(region, "00000000-0000-4000-8000-ffffffffffff"))
    }

    /// A resource-share ARN in `region` that no stored share is using.
    pub fn fresh_share_arn(&mut self, region: &str) -> String {
        for _ in 0..1024 {
            let candidate = share_arn(region, &self.fresh_uuid());
            if !self.resource_shares.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: there are
        // always more of these than there are stored shares.
        (0u64..=self.resource_shares.len() as u64)
            .map(|n| share_arn(region, &format!("00000000-0000-4000-8000-{n:012x}")))
            .find(|arn| !self.resource_shares.contains_key(arn))
            .unwrap_or_else(|| share_arn(region, "00000000-0000-4000-8000-ffffffffffff"))
    }

    /// A fresh UUID-shaped identifier, used for a share id and for the
    /// `clientToken` RAM generates when the request omits one.
    pub fn fresh_uuid(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.id_counter.rotate_left(17);
        let digits: String = (0..4)
            .map(|_| format!("{:08x}", splitmix64(&mut seed) as u32))
            .collect();
        format!(
            "{}-{}-{}-{}-{}",
            &digits[0..8],
            &digits[8..12],
            &digits[12..16],
            &digits[16..20],
            &digits[20..32]
        )
    }
}

/// The configuration to store for a share created with `requested`.
///
/// Every member the request leaves out takes this service's default, so the stored
/// record is complete and a read never has to invent one.
pub fn share_configuration(
    requested: Option<&ResourceShareConfiguration>,
) -> ResourceShareConfiguration {
    ResourceShareConfiguration {
        retain_sharing_on_account_leave_organization: Some(
            requested
                .and_then(|configuration| {
                    configuration.retain_sharing_on_account_leave_organization
                })
                .unwrap_or(DEFAULT_RETAIN_SHARING_ON_ACCOUNT_LEAVE_ORGANIZATION),
        ),
    }
}

/// The message of the `UnknownResourceException` every operation that takes a
/// `resourceShareArn` renders when no share has it.
///
/// One template across every such operation, built from the ARN the request named,
/// so a caller can read the ARN back out of the message it gets.
pub fn unknown_share_message(arn: &str) -> String {
    format!("ResourceShare {arn} could not be found.")
}

/// The message of the `ResourceArnNotFoundException` a read of a resource's policies
/// renders when no live share of this account shares the resource the ARN names.
///
/// Worded like [`unknown_share_message`]: the error code says what is wrong with the
/// ARN and the message says which resource it was, so a caller can read the ARN it
/// named back out of the message.
pub fn unknown_resource_arn_message(arn: &str) -> String {
    format!("Resource {arn} could not be found.")
}

/// The message RAM reports with every error that refuses a
/// `resourceShareInvitationArn`: the ARN itself.
///
/// RAM's invitation-ARN errors (`InvalidResourceShareInvitationArn.NotFound`,
/// `.AlreadyRejected`, `.Expired`) carry the ARN the request named as their whole
/// message — the error code says what is wrong with it and the message says which
/// invitation it was — so one template serves every operation that takes the member.
pub fn invitation_arn_message(arn: &str) -> String {
    arn.to_string()
}

/// The `type` RAM reports for a shared resource: `<service-code>:<resource-code>`,
/// or `None` when the ARN's resource field names no type.
///
/// From `Resource$type`: "The resource type. This takes the form of:
/// service-code:resource-code … For example, an Amazon EC2 Subnet would be
/// represented by the string ec2:subnet" — so both halves are read off the
/// resource's own ARN rather than stored beside it.
pub fn resource_type_of_arn(arn: &str) -> Option<String> {
    let service = arn_service(arn)?;
    // The resource field is everything after the fifth colon, and its type is the
    // first segment of it: `subnet/subnet-1111` and `subnet:subnet-1111` both name
    // the type `subnet`. A resource field with no separator (an S3 bucket name, say)
    // names no type.
    let resource = arn.splitn(6, ':').nth(5)?;
    let (code, _) = resource.split_once(['/', ':'])?;
    (!code.is_empty() && !service.is_empty()).then(|| format!("{service}:{code}"))
}

/// Whether two resource-type names name the same resource type.
///
/// `Resource$type` is read off a shared resource's own ARN, where the resource code is
/// spelled the way the owning service spells it there (`prefix-list`), while a managed
/// permission's `resourceType` carries the spelling RAM publishes for the same type
/// (`ec2:PrefixList`). Those two spellings differ only in case and in the separators
/// inside the resource code, and the member's own documentation says the name "is not
/// case sensitive" (`ListPermissions$resourceType`), so the comparison is on the
/// letters and digits of each half alone.
pub fn resource_type_matches(left: &str, right: &str) -> bool {
    resource_type_key(left) == resource_type_key(right)
}

/// `resource_type` reduced to what [`resource_type_matches`] compares: its letters and
/// digits, lowercased, with the colon that separates the service code from the
/// resource code kept so the two halves cannot run together.
fn resource_type_key(resource_type: &str) -> String {
    resource_type
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ':')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The `ResourceRegionScope` of the resource `arn` names.
///
/// An ARN that names no Region belongs to a service whose resources are reachable
/// "from any Amazon Web Services Region", which is what `GLOBAL` reports; one that
/// names a Region exists in it and is `REGIONAL`
/// (`Resource$resourceRegionScope`).
pub fn resource_region_scope(arn: &str) -> &'static str {
    match arn_region(arn) {
        Some("") | None => RESOURCE_REGION_SCOPE_GLOBAL,
        Some(_) => RESOURCE_REGION_SCOPE_REGIONAL,
    }
}

/// Whether a resource of this scope passes a `ResourceRegionScopeFilter`.
///
/// "ALL – the results include both global and regional resources … GLOBAL – … only
/// global resources … REGIONAL – … only regional resources", and "The default value
/// is ALL", so a request that names no filter keeps everything.
pub fn resource_region_scope_matches(filter: Option<&str>, scope: &str) -> bool {
    match filter.unwrap_or(RESOURCE_REGION_SCOPE_FILTER_ALL) {
        RESOURCE_REGION_SCOPE_FILTER_ALL => true,
        requested => requested == scope,
    }
}

/// A resource association as the modelled `Resource` wire shape.
///
/// Everything a `Resource` reports about a shared resource is either read off its
/// ARN (the type and the Region scope) or taken from the association that shares it:
/// `creationTime` is "the date and time when the resource was associated with the
/// resource share" and `status` is where that association has got to — a resource
/// whose association is still being made is `PENDING`, one whose association has
/// been made is `AVAILABLE`, and one the share no longer shares is `UNAVAILABLE`.
pub fn resource_view(record: &AssociationRecord) -> Resource {
    let status = match record.status.as_str() {
        ASSOCIATION_STATUS_ASSOCIATED => RESOURCE_STATUS_AVAILABLE,
        ASSOCIATION_STATUS_ASSOCIATING => RESOURCE_STATUS_PENDING,
        _ => RESOURCE_STATUS_UNAVAILABLE,
    };
    Resource {
        arn: Some(record.associated_entity.clone()),
        r#type: resource_type_of_arn(&record.associated_entity),
        resource_share_arn: Some(record.resource_share_arn.clone()),
        // "This value is available only if the resource is part of a resource
        // group", and nothing in this service puts one in a group.
        resource_group_arn: None,
        status: Some(status.to_string()),
        status_message: record.status_message.clone(),
        creation_time: Some(to_date_time(record.creation_time)),
        last_updated_time: Some(to_date_time(record.last_updated_time)),
        resource_region_scope: Some(resource_region_scope(&record.associated_entity).to_string()),
    }
}

/// The `statusMessage` an association of this type carries while it has not settled
/// yet, or `None` when there is nothing to report.
///
/// RAM shares with a principal by INVITATION: the invited principal accepts or
/// rejects it (the modelled `AcceptResourceShareInvitation` /
/// `RejectResourceShareInvitation` operations, and the invitations
/// `GetResourceShareInvitations` reports), so what a principal association that has
/// not completed is waiting for is that answer, and
/// `ResourceShareAssociation$statusMessage` ("A message about the status of the
/// association") is the member that says so. A resource or a source association
/// names no principal, waits for nobody, and reports no message. A settled
/// association waits for nothing either, so the member is dropped when it reaches
/// `ASSOCIATED`.
pub fn associating_status_message(association_type: &str) -> Option<&'static str> {
    (association_type == ASSOCIATION_TYPE_PRINCIPAL)
        .then_some(ASSOCIATING_PRINCIPAL_STATUS_MESSAGE)
}

/// Whether an association status is one on the ATTACH path: the association is being
/// made (`ASSOCIATING`) or has been made (`ASSOCIATED`).
///
/// This is what "the share shares this entity" means: an association that was torn
/// down (`DISASSOCIATING`, `DISASSOCIATED`) or that failed shares nothing, and
/// re-associating an entity on this path changes nothing.
pub fn association_is_attaching(status: &str) -> bool {
    matches!(
        status,
        ASSOCIATION_STATUS_ASSOCIATING | ASSOCIATION_STATUS_ASSOCIATED
    )
}

/// Whether an association status is TERMINAL: one no further move is owed from.
///
/// Of the modelled `ResourceShareAssociationStatus` values
/// (`smithy_query(query="shape",shape="ResourceShareAssociationStatus")`), the ones
/// that name a settled outcome are `ASSOCIATED`, `DISASSOCIATED` and `FAILED`; the
/// `-ING` ones (`ASSOCIATING`, `DISASSOCIATING`, `SUSPENDING`, `RESTORING`) each name
/// a move still under way. This slice only ever stores the first four of those, but
/// the predicate is written over the enum so it stays right if another is.
pub fn association_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        ASSOCIATION_STATUS_ASSOCIATED | ASSOCIATION_STATUS_DISASSOCIATED | "FAILED"
    )
}

/// Tags as the wire shape wants them, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// The tags of a create request, keyed by tag key. Call it only after
/// [`tags_rejection`] has passed, which is what guarantees every tag has a key.
pub fn tag_map(tags: &[Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|tag| {
            let key = tag.key.clone()?;
            Some((key, tag.value.clone().unwrap_or_default()))
        })
        .collect()
}

/// Why a create request's `tags` are unacceptable, or `None` when they are fine.
///
/// Each variant names the modelled error the operation renders it as, because RAM
/// words the three refusals differently.
#[derive(Debug, Clone)]
pub enum TagRejection {
    /// A tag the model admits but the service cannot store —
    /// `InvalidParameterException`.
    Invalid(String),
    /// More tags than one resource may carry — `TagLimitExceededException`.
    LimitExceeded(String),
    /// A tag key AWS reserves — `TagPolicyViolationException`.
    PolicyViolation(String),
}

/// Why a resource share cannot carry `total` tags, or `None` when it can.
///
/// The cap is on the share, not on one request, so an operation that ADDS tags to a
/// share that already has some passes the total it would end up with.
pub fn tag_count_rejection(total: usize) -> Option<String> {
    (total > MAX_TAGS_PER_SHARE).then(|| {
        format!(
            "the resource share would carry {total} tags, more than the \
             {MAX_TAGS_PER_SHARE} a resource share accepts"
        )
    })
}

/// Why `tags` cannot be attached to a resource share, or `None` when they can.
pub fn tags_rejection(tags: &[Tag]) -> Option<TagRejection> {
    if let Some(why) = tag_count_rejection(tags.len()) {
        return Some(TagRejection::LimitExceeded(why));
    }
    for tag in tags {
        // "Every tag must have a key" (modelled `Tag$key` documentation).
        let Some(key) = tag.key.as_deref().filter(|key| !key.is_empty()) else {
            return Some(TagRejection::Invalid(
                "every entry in tags must carry a non-empty key".to_string(),
            ));
        };
        if key.len() > MAX_NAME_LEN {
            return Some(TagRejection::Invalid(format!(
                "tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                truncate(key)
            )));
        }
        if tag.value.as_deref().unwrap_or_default().len() > MAX_NAME_LEN {
            return Some(TagRejection::Invalid(format!(
                "the value of tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                truncate(key)
            )));
        }
        if key
            .to_ascii_lowercase()
            .starts_with(RESERVED_TAG_KEY_PREFIX)
        {
            return Some(TagRejection::PolicyViolation(format!(
                "tag key '{}' uses the reserved prefix '{RESERVED_TAG_KEY_PREFIX}'",
                truncate(key)
            )));
        }
    }
    None
}

/// Why `tagKeys` cannot be detached from a resource share, or `None` when they can.
///
/// Rendered as `InvalidParameterException`: the same shape rules the keys of a
/// `TagList` must satisfy, applied to the bare keys an untag names.
pub fn tag_keys_rejection(keys: &[String]) -> Option<String> {
    keys.iter()
        .find(|key| key.is_empty() || key.len() > MAX_NAME_LEN)
        .map(|key| {
            if key.is_empty() {
                "every entry in tagKeys must be a non-empty key".to_string()
            } else {
                format!(
                    "tag key '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
                    truncate(key)
                )
            }
        })
}

/// Whether `tags` satisfies `filters`.
///
/// A filter matches a share that carries its key; when the filter also lists
/// values, the share's value for that key must be one of them. Several filters are
/// evaluated as an OR condition, which the modelled `TagFilter` documentation
/// states; no filters at all match everything.
pub fn tags_match_filters(tags: &BTreeMap<String, String>, filters: &[TagFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|filter| {
        let Some(key) = filter.tag_key.as_deref() else {
            return false;
        };
        let Some(value) = tags.get(key) else {
            return false;
        };
        // "If no values are provided, then the filter matches any tag with the
        // specified key, regardless of its value" (modelled `TagFilter$tagValues`).
        filter.tag_values.as_deref().is_none_or(|candidates| {
            candidates.is_empty() || candidates.iter().any(|candidate| candidate == value)
        })
    })
}

/// Whether `value` is a syntactically valid ARN: the six colon-separated fields
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a partition, a
/// service and a resource.
pub fn is_arn(value: &str) -> bool {
    if value.len() > MAX_ARN_LEN {
        return false;
    }
    let mut fields = value.splitn(6, ':');
    let Some("arn") = fields.next() else {
        return false;
    };
    let (Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The account field of an ARN, or `None` when `value` is not one.
pub fn arn_account(value: &str) -> Option<&str> {
    if !is_arn(value) {
        return None;
    }
    value.split(':').nth(4)
}

/// The region field of an ARN, or `None` when `value` is not one. Empty for an ARN
/// that names no Region.
pub fn arn_region(value: &str) -> Option<&str> {
    if !is_arn(value) {
        return None;
    }
    value.split(':').nth(3)
}

/// The service field of an ARN, or `None` when `value` is not one.
pub fn arn_service(value: &str) -> Option<&str> {
    if !is_arn(value) {
        return None;
    }
    value.split(':').nth(2)
}

/// Whether `value` is an ARN of THIS service's resource-share resource.
pub fn is_share_arn(value: &str) -> bool {
    is_arn_of_resource(value, SHARE_ARN_RESOURCE_PREFIX)
}

/// Whether `value` is an ARN of THIS service's resource-share-invitation resource.
pub fn is_invitation_arn(value: &str) -> bool {
    is_arn_of_resource(value, INVITATION_ARN_RESOURCE_PREFIX)
}

/// Whether `value` is an ARN of this service naming a resource of the kind
/// `prefix` introduces, with a non-empty id after it.
fn is_arn_of_resource(value: &str, prefix: &str) -> bool {
    if !is_arn(value) {
        return false;
    }
    let mut fields = value.splitn(6, ':');
    let (service, resource) = (fields.nth(2), fields.nth(2));
    service == Some(ARN_SERVICE)
        && resource
            .and_then(|resource| resource.strip_prefix(prefix))
            .is_some_and(|id| !id.is_empty())
}

/// The ARN of the resource share invitation `id` in `region`.
pub fn invitation_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INVITATION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The account an invited principal belongs to — the one that answers its invitation
/// — or `None` when `entity` names no account.
///
/// `ResourceShareAssociation$associatedEntity` admits several forms for a principal;
/// of those, an ARN carries the account in its account field, and a bare Amazon Web
/// Services account id IS the account. A service principal name (`…amazonaws.com`)
/// names no account, so there is nobody to invite.
pub fn invitation_receiver_account(entity: &str) -> Option<&str> {
    if let Some(account) = arn_account(entity) {
        return (!account.is_empty()).then_some(account);
    }
    is_account_id(entity).then_some(entity)
}

/// The IAM principal an invitation was sent to, when the invited entity names one
/// rather than a whole account.
///
/// `ResourceShareInvitation$receiverArn` is "the ARN of the IAM user or role that
/// received the invitation", so only an `iam` ARN fills it; an account id or any
/// other form invites the account itself and leaves the member out.
pub fn invitation_receiver_arn(entity: &str) -> Option<&str> {
    (arn_service(entity) == Some(IAM_ARN_SERVICE)).then_some(entity)
}

/// Whether `value` is an Amazon Web Services account id: twelve digits.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The ARN of the resource share `id` in `region`.
pub fn share_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{SHARE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the customer managed permission `name` in `region`.
///
/// A customer managed permission is named by its own name rather than by a generated
/// id: the name "must be unique within the Amazon Web Services Region"
/// (`CreatePermission$name`), and it "is created in the Amazon Web Services Region in
/// which you call the operation", so the Region and the name together identify it.
pub fn permission_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PERMISSION_ARN_RESOURCE_PREFIX}{name}"),
    )
}

/// Whether `value` is an ARN of THIS service's managed-permission resource.
pub fn is_permission_arn(value: &str) -> bool {
    is_arn_of_resource(value, PERMISSION_ARN_RESOURCE_PREFIX)
}

/// Why a value an operation expected to name a managed permission is not one of this
/// service's permission ARNs, or `None` when it is.
///
/// Worded like [`share_arn_rejection`]: a value that is not a permission ARN is
/// malformed, whether or not any permission has it — the `MalformedArnException`
/// case, as distinct from the `UnknownResourceException` one.
pub fn permission_arn_rejection(member: &str, arn: &str) -> Option<String> {
    (!is_permission_arn(arn)).then(|| {
        format!(
            "{member} '{}' is not a managed permission ARN of the form \
             arn:<partition>:{ARN_SERVICE}:<region>:<account>:{PERMISSION_ARN_RESOURCE_PREFIX}<name>",
            truncate(arn)
        )
    })
}

/// Whether a managed permission of this `PermissionType` passes a
/// `PermissionTypeFilter`.
///
/// Worded like [`resource_region_scope_matches`]: the filter's `ALL` keeps both kinds,
/// every other value is one of the `PermissionType` values a permission carries, and a
/// read that names no filter keeps everything
/// ([`PERMISSION_TYPE_FILTER_ALL`]).
pub fn permission_type_matches(filter: Option<&str>, permission_type: &str) -> bool {
    match filter.unwrap_or(PERMISSION_TYPE_FILTER_ALL) {
        PERMISSION_TYPE_FILTER_ALL => true,
        requested => requested == permission_type,
    }
}

/// Whether a managed permission reporting `status` passes a `permissionStatus` filter.
///
/// Worded like [`permission_type_matches`]: a read that names no filter keeps every
/// permission, and one that names a `PermissionStatus` keeps the permissions reporting
/// exactly it — `ListPermissions$permissionStatus` "filter[s] the results to include
/// only permissions with the specified status".
pub fn permission_status_matches(filter: Option<&str>, status: &str) -> bool {
    filter.is_none_or(|requested| requested == status)
}

/// Whether the managed permission `arn` names is one a request signed for `region`
/// sees.
///
/// A customer managed permission "is created in the Amazon Web Services Region in
/// which you call the operation" (`CreatePermission$name`) and its ARN carries that
/// Region ([`permission_arn`]), so it belongs to that Region alone. A permission Amazon
/// Web Services manages is RAM's own and its ARN names no Region
/// ([`aws_managed_permission_arn`]), so every Region sees it.
pub fn permission_visible_in_region(arn: &str, region: &str) -> bool {
    match arn_region(arn) {
        Some("") | None => true,
        Some(permission_region) => permission_region == region,
    }
}

/// The message of the `UnknownResourceException` an operation on a managed permission
/// renders when this account holds none under that ARN.
///
/// Worded like [`unknown_share_message`]: the error code says what is wrong with the
/// ARN and the message says which permission it was.
pub fn unknown_permission_message(arn: &str) -> String {
    format!("Permission {arn} could not be found.")
}

/// The message of the `UnknownResourceException` an operation renders when the
/// permission exists but carries no version of the number the request named.
pub fn unknown_permission_version_message(arn: &str, version: i32) -> String {
    format!("Permission {arn} has no version {version}.")
}

/// The message of the `PermissionAlreadyExistsException` a create renders when the
/// Region already holds a permission of that name.
///
/// The refusal identifies the permission that already holds the name by its ARN — the
/// one thing that names the conflicting resource, so a caller can read it back — and
/// says nothing else: the message is fixed text around the stored permission's ARN.
pub fn permission_exists_message(arn: &str) -> String {
    format!("The permission {arn} already exists.")
}

/// Whether `entity` names a part of an AWS Organizations hierarchy: an organization,
/// a root, an organizational unit, or an account inside one — either by an ARN of the
/// `organizations` service ([`ORGANIZATIONS_ARN_SERVICE`]) or by the bare id
/// Organizations gives it ([`ORGANIZATIONS_ID_PREFIXES`]).
///
/// `ResourceShareAssociation$associatedEntity` admits these forms for a principal
/// ("For a principal association, this is one of the following: … an organization in
/// Organizations … an organizational unit (OU) in Organizations"), and RAM admits only
/// the CALLER'S OWN organization, root or OU there, so an entity of this form belongs
/// to the owner's organization by construction.
pub fn entity_is_organization_scoped(entity: &str) -> bool {
    if let Some(service) = arn_service(entity) {
        return service == ORGANIZATIONS_ARN_SERVICE;
    }
    ORGANIZATIONS_ID_PREFIXES.iter().any(|prefix| {
        entity.strip_prefix(prefix).is_some_and(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    })
}

/// Whether an associated entity lies OUTSIDE the organization of the account that owns
/// the share.
///
/// `ResourceShareAssociation$external` "indicates whether the principal belongs to the
/// same organization in Organizations as the Amazon Web Services account that owns the
/// resource share" (that member's own documentation), so the flag is the negation of
/// in-organization membership, derived from the entity and the share's owner:
///
/// * an entity that names the owning account ITSELF — a bare account id equal to it, or
///   an ARN whose account field is it — is that account, hence in its organization;
/// * an entity that names a part of an Organizations hierarchy
///   ([`entity_is_organization_scoped`]) is inside the owner's organization whatever
///   account the ARN carries, because RAM admits only the owner's own organization
///   there — so the management account the ARN names does not make it external;
/// * every other entity — another account, a principal in one, a service principal —
///   is one this service cannot place inside the owner's organization, and an
///   association it cannot place there is reported as external.
pub fn entity_is_external(owner: &str, entity: &str) -> bool {
    if entity_is_organization_scoped(entity) {
        return false;
    }
    match arn_account(entity) {
        Some(account) => account != owner,
        None => entity != owner,
    }
}

/// Why `arn` is not usable as a `resourceShareArn`, or `None` when it is.
///
/// A value that is not an ARN of this service's resource-share resource is
/// malformed, whether or not any share has it — the
/// `MalformedArnException` case, as distinct from the `UnknownResourceException` one.
pub fn share_arn_rejection(member: &str, arn: &str) -> Option<String> {
    (!is_share_arn(arn)).then(|| {
        format!(
            "{member} '{}' is not a resource share ARN of the form \
             arn:<partition>:{ARN_SERVICE}:<region>:<account>:{SHARE_ARN_RESOURCE_PREFIX}<id>",
            truncate(arn)
        )
    })
}

/// Why an entry of `resourceArns` (or a `resourceArn` filter) is not usable, or
/// `None` when every entry is an ARN.
pub fn resource_arn_rejection(member: &str, arns: &[String]) -> Option<String> {
    arns.iter()
        .find(|arn| !is_arn(arn))
        .map(|arn| not_an_arn_message(member, arn))
}

/// The message RAM refuses a value that is not a shareable resource's ARN with.
///
/// The refusal is about the KIND of resource the ARN names, not about any resource the
/// store holds, so the text names nothing from the request: no ARN, id, account or
/// Region.
pub const NON_SHAREABLE_RESOURCE_TYPE_MESSAGE: &str =
    "You cannot share the selected resource type.";

/// Whether `arn` names a resource of a type this service can share.
///
/// The types it can share are [`RESOURCE_TYPES`], the catalog `ListResourceTypes`
/// reports, each spelled `<service-code>:<resource-code>`. A resource's own ARN carries
/// the service code in its service field, and the resource code as one of the segments
/// of its resource field: a field that leads with the code (`subnet/subnet-1111`), one
/// that nests it under another resource (`outpost/op-1/bucket/example`) and one that is
/// the code alone (`catalog`) all name it. The two spellings are compared the way
/// [`resource_type_matches`] compares them, because the owning service spells the code
/// in an ARN the way it spells it there (`prefix-list`) rather than the way RAM
/// publishes the type (`ec2:PrefixList`).
pub fn is_shareable_resource_arn(arn: &str) -> bool {
    let Some(service) = arn_service(arn) else {
        return false;
    };
    let Some(resource) = arn.splitn(6, ':').nth(5) else {
        return false;
    };
    resource
        .split(['/', ':'])
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            let candidate = format!("{service}:{segment}");
            RESOURCE_TYPES
                .iter()
                .any(|entry| resource_type_matches(entry.resource_type, &candidate))
        })
}

/// Why an entry of `resourceArns` is not usable as the ARN of a shareable resource, or
/// `None` when every entry is one.
///
/// Classification by KIND, which is what every member that takes the ARN of a resource
/// to share — or of one whose sharing this service reports — needs before it reaches the
/// store: a value that is no ARN at all is refused with [`not_an_arn_message`], and an
/// ARN of a resource type this service cannot share with
/// [`NON_SHAREABLE_RESOURCE_TYPE_MESSAGE`]. Both are the `MalformedArnException` case,
/// as distinct from the state question — does any share of this account hold the
/// resource? — that the operations answer with `ResourceArnNotFoundException` or
/// `UnknownResourceException` only once an entry has passed this check.
pub fn shareable_resource_arn_rejection(member: &str, arns: &[String]) -> Option<String> {
    resource_arn_rejection(member, arns).or_else(|| {
        arns.iter()
            .any(|arn| !is_shareable_resource_arn(arn))
            .then(|| NON_SHAREABLE_RESOURCE_TYPE_MESSAGE.to_string())
    })
}

/// Why a value this service expected to be an ARN cannot be read as one, as the
/// message of the `MalformedArnException` that refuses it: one template for every
/// member that takes an ARN, naming the member and the value it refused.
pub fn not_an_arn_message(member: &str, value: &str) -> String {
    format!(
        "{member} '{}' is not an ARN of the form arn:<partition>:<service>:<region>:<account>:<resource>",
        truncate(value)
    )
}

/// Why a list of entities one call names is too long for it, or `None`.
///
/// One template for every member that takes a list of entities, whichever modelled
/// error the operation renders it as: `ResourceShareLimitExceededException` where a
/// mutation refuses the list it was asked to associate, and
/// `InvalidParameterException` where a read refuses the list it was asked about.
pub fn entity_count_rejection(member: &str, count: usize) -> Option<String> {
    (count > MAX_ENTITIES_PER_CALL).then(|| {
        format!(
            "{member} names {count} entities, more than the {MAX_ENTITIES_PER_CALL} this service accepts in one call"
        )
    })
}

/// Why an entry of a principal or source list cannot be stored, or `None`.
///
/// RAM accepts several forms here — an account id, an ARN, an organization id or
/// path, a service principal name — so the only rule applied is that an entry is
/// present and fits the store.
pub fn entity_value_rejection(member: &str, entities: &[String]) -> Option<String> {
    entities
        .iter()
        .find(|entity| entity.is_empty() || entity.len() > MAX_ARN_LEN)
        .map(|entity| {
            if entity.is_empty() {
                format!("{member} must not contain an empty value")
            } else {
                format!(
                    "{member} entry '{}' is longer than the {MAX_ARN_LEN} characters this service accepts",
                    truncate(entity)
                )
            }
        })
}

/// Why a resource share `name` cannot be stored, or `None` when it can.
pub fn name_rejection(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("name must not be empty".to_string());
    }
    (name.len() > MAX_NAME_LEN).then(|| {
        format!(
            "name '{}' is longer than the {MAX_NAME_LEN} characters this service accepts",
            truncate(name)
        )
    })
}

/// The service code half of a resource type: the part before the colon of a
/// `<service-code>:<resource-code>` value.
pub fn resource_type_service(resource_type: &str) -> Option<&str> {
    resource_type.split_once(':').map(|(service, _)| service)
}

/// Why a `resourceType` cannot name a resource type a customer managed permission can
/// be created for, or `None` when it can.
///
/// From `CreatePermission$resourceType`: "Specifies the name of the resource type
/// that this customer managed permission applies to. The format is `:` and is case
/// sensitive. For example, to specify an Amazon EC2 Subnet, you can use the string
/// ec2:Subnet. To see the list of valid values for this parameter, query the
/// ListResourceTypes operation. This value must match the display name of the
/// resource" — so a value RAM accepts here names a service code and a resource code
/// separated by one colon, is one of the resource types this service shares, and is
/// one of those a CUSTOMER managed permission can be created for: a resource type
/// shared only through the permissions Amazon Web Services manages for it takes no
/// customer managed permission ([`RESOURCE_TYPES`]). Rendered as
/// `InvalidParameterException`.
pub fn resource_type_rejection(member: &str, value: &str) -> Option<String> {
    let well_formed = resource_type_service(value).is_some_and(|service| {
        let code = &value[service.len() + 1..];
        !service.is_empty()
            && !code.is_empty()
            && service
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    if !well_formed {
        return Some(format!(
            "{member} '{}' is not a resource type of the form <service-code>:<resource-code>",
            truncate(value)
        ));
    }
    let Some(entry) = resource_type_entry_of(value) else {
        return Some(format!(
            "{member} '{}' is not a resource type that this service can share; \
             ListResourceTypes reports the resource types it can",
            truncate(value)
        ));
    };
    (!entry.customer_managed_permissions).then(|| {
        // RAM words this refusal without naming the member: the resource type is not
        // one a customer managed permission can be created for at all.
        format!("Customer managed permission is not supported for resource type {value}")
    })
}

/// The catalog entry for `value`, or `None` when this service shares no resource type
/// of that name.
///
/// Matched without regard to case, the way `Resource$type` describes a resource type
/// ("this takes the form of: `service-code`:`resource-code`, and is case-insensitive").
pub fn resource_type_entry_of(value: &str) -> Option<&'static ResourceTypeEntry> {
    RESOURCE_TYPES
        .iter()
        .find(|entry| entry.resource_type.eq_ignore_ascii_case(value))
}

/// Where `value` sits in the published catalog, or `None` when the catalog carries no
/// resource type of that name.
///
/// The position is what a page of the catalog resumes at, because [`RESOURCE_TYPES`] is
/// published in its own order rather than in the order the display names sort in.
pub fn resource_type_position(value: &str) -> Option<usize> {
    RESOURCE_TYPES
        .iter()
        .position(|entry| entry.resource_type.eq_ignore_ascii_case(value))
}

/// What is wrong with a managed permission's `policyTemplate`, told apart by the
/// modelled error RAM words that refusal with.
#[derive(Debug, Clone)]
pub enum PolicyTemplateRejection {
    /// The template is past what this service reads at all, so it is not a policy
    /// template this service can hold: `MalformedPolicyTemplateException`.
    Malformed(String),
    /// The template is not a policy a managed permission may carry — it cannot be
    /// read as a policy statement, its effect is not an allow, or it names an action
    /// the permission's resource type does not support: `InvalidPolicyException`.
    ///
    /// A document that cannot be read as a policy statement carries RAM's fixed
    /// [`INVALID_POLICY_FORMAT_MESSAGE`]; the rules a readable statement breaks are
    /// reported with the rule that was broken.
    InvalidPolicy(String),
}

/// Why the policy template of a permission for `resource_type` cannot be stored, or
/// `None` when it can.
///
/// From `CreatePermission$policyTemplate`: it is "a string in JSON format string that
/// contains the following elements of a resource-based policy: Effect: must be set to
/// ALLOW. Action: specifies the actions that are allowed by this customer managed
/// permission. The list must contain only actions that are supported by the specified
/// resource type … Condition: (optional)". One template across every operation that
/// takes one, so a permission and a later version of it are held to one rule.
///
/// A document that cannot be read as that policy statement at all — one that is not
/// JSON, not a JSON object, or carrying an element that is not of the type a policy
/// statement's element takes — is one refusal class with one fixed message
/// ([`invalid_policy_format`]); a statement that reads but breaks one of the rules
/// above is reported with the rule it broke — an action the resource type does not
/// support ([`action_supported_for_resource_type`]) with
/// [`action_not_allowed_message`].
pub fn policy_template_rejection(
    resource_type: &str,
    template: &str,
) -> Option<PolicyTemplateRejection> {
    if template.len() > MAX_POLICY_TEMPLATE_LEN {
        return Some(PolicyTemplateRejection::Malformed(format!(
            "policyTemplate is longer than the {MAX_POLICY_TEMPLATE_LEN} characters this service \
             accepts"
        )));
    }
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(template) else {
        return invalid_policy_format();
    };
    let Some(elements) = document.as_object() else {
        return invalid_policy_format();
    };
    let Some(effect) = policy_template_element(elements, POLICY_TEMPLATE_EFFECT_ELEMENT)
        .and_then(::serde_json::Value::as_str)
    else {
        return invalid_policy_format();
    };
    if !effect.eq_ignore_ascii_case(POLICY_TEMPLATE_EFFECT_ALLOW) {
        return Some(PolicyTemplateRejection::InvalidPolicy(format!(
            "policyTemplate {POLICY_TEMPLATE_EFFECT_ELEMENT} '{}' must be \
             {POLICY_TEMPLATE_EFFECT_ALLOW}",
            truncate(effect)
        )));
    }
    // `Action` names one action or a list of them, as an IAM policy statement's does.
    let actions: Vec<&str> = match policy_template_element(elements, POLICY_TEMPLATE_ACTION_ELEMENT)
    {
        Some(::serde_json::Value::String(action)) => vec![action.as_str()],
        Some(::serde_json::Value::Array(entries)) => {
            let named: Vec<&str> = entries
                .iter()
                .filter_map(::serde_json::Value::as_str)
                .collect();
            if named.len() != entries.len() {
                return invalid_policy_format();
            }
            named
        }
        _ => return invalid_policy_format(),
    };
    if actions.is_empty() {
        return invalid_policy_format();
    }
    if let Some(condition) = policy_template_element(elements, POLICY_TEMPLATE_CONDITION_ELEMENT) {
        if !condition.is_object() {
            return invalid_policy_format();
        }
    }
    // "The list must contain only actions that are supported by the specified resource
    // type": an action the resource type does not support makes what the template says
    // a policy this permission may not carry
    // ([`action_supported_for_resource_type`]).
    if actions
        .iter()
        .any(|action| !action_supported_for_resource_type(resource_type, action))
    {
        return Some(PolicyTemplateRejection::InvalidPolicy(
            action_not_allowed_message(&truncate(resource_type)),
        ));
    }
    None
}

/// The refusal of a policy document that cannot be read as a policy statement: the
/// `InvalidPolicyException` class, worded with its one fixed sentence.
fn invalid_policy_format() -> Option<PolicyTemplateRejection> {
    Some(PolicyTemplateRejection::InvalidPolicy(
        INVALID_POLICY_FORMAT_MESSAGE.to_string(),
    ))
}

/// One element of a policy template, looked up however the request cased its name.
fn policy_template_element<'a>(
    elements: &'a ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Option<&'a ::serde_json::Value> {
    elements
        .iter()
        .find(|(element, _)| element.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// Why a `clientToken` is not a valid idempotency identifier, or `None`.
///
/// Rendered as `InvalidClientTokenException`. The model publishes no shape for the
/// token, so the rule is the weakest one that keeps it storable: present, and
/// within the store's bound.
pub fn client_token_rejection(token: &str) -> Option<String> {
    if token.is_empty() {
        return Some("clientToken must not be empty".to_string());
    }
    (token.len() > MAX_NAME_LEN).then(|| {
        format!(
            "clientToken is longer than the {MAX_NAME_LEN} characters this service accepts"
        )
    })
}

/// The page size this read uses: whatever the request asked for, clamped into
/// 1..=[`MAX_PAGE_SIZE`], so the page a handler allocates is sized by the server.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.map_or(MAX_PAGE_SIZE, |requested| {
        requested.clamp(1, MAX_PAGE_SIZE as i32) as usize
    })
}

/// A `nextToken` naming the ARN the next page starts at: a resource share, an
/// invitation or a shared resource, whichever that read pages over.
///
/// `nextToken` is modelled opaque, so the ARN is hex-encoded rather than handed
/// back in the clear — opaque to a caller, not secret. One encoder serves every
/// read that pages over an ARN; what differs between them is which ARN a token is
/// allowed to name, which is the decoder's job ([`decode_share_cursor`],
/// [`decode_invitation_cursor`], [`decode_resource_cursor`]).
pub fn encode_arn_cursor(arn: &str) -> String {
    hex::encode(arn)
}

/// The share ARN a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
pub fn decode_share_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|arn| is_share_arn(arn))
}

/// A `nextToken` naming the association the next page starts at.
pub fn encode_association_cursor(key: &AssociationKey) -> String {
    hex::encode(format!(
        "{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}",
        key.0, key.1, key.2
    ))
}

/// The association key a `nextToken` names, or `None` when the token is not one
/// this service issued.
pub fn decode_association_cursor(token: &str) -> Option<AssociationKey> {
    let decoded = decode_cursor(token)?;
    let mut fields = decoded.split(FIELD_SEPARATOR);
    let (Some(arn), Some(association_type), Some(entity), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if !is_share_arn(arn) || !crate::ops::types::RESOURCE_SHARE_ASSOCIATION_TYPE_VALUES
        .contains(&association_type)
    {
        return None;
    }
    Some((
        arn.to_string(),
        association_type.to_string(),
        entity.to_string(),
    ))
}

/// The resource ARN a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
pub fn decode_resource_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|arn| is_arn(arn))
}

/// The invitation ARN a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
pub fn decode_invitation_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|arn| is_invitation_arn(arn))
}

/// The managed-permission ARN a `nextToken` names, or `None` when the token is not one
/// this service issued — the `InvalidNextTokenException` case.
pub fn decode_permission_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|arn| is_permission_arn(arn))
}

/// A `nextToken` naming the permission version the next page starts at.
///
/// A read that pages over the versions of ONE permission pages over version numbers
/// rather than over ARNs, so the value a token carries is the number itself, encoded
/// the way every other cursor of this service is.
pub fn encode_permission_version_cursor(version: i32) -> String {
    hex::encode(version.to_string())
}

/// The permission version a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
///
/// A token this service issued names a version a permission could carry, so the test is
/// on the FORM of the value, as every other decoder's is: a version number at or above
/// the first one ([`FIRST_PERMISSION_VERSION`]), whether or not any permission still
/// carries that one.
pub fn decode_permission_version_cursor(token: &str) -> Option<i32> {
    decode_cursor(token)
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|version| *version >= FIRST_PERMISSION_VERSION)
}

/// A `nextToken` naming the resource type the next page of the catalog starts at.
///
/// The catalog is not a store of ARNs, so the page a token resumes is named by the
/// display name of a resource type rather than by an ARN; the value is hex-encoded the
/// way every other cursor of this service is, so `nextToken` stays opaque to a caller.
pub fn encode_resource_type_cursor(resource_type: &str) -> String {
    hex::encode(resource_type)
}

/// The resource type a `nextToken` names, or `None` when the token is not one this
/// service issued — the `InvalidNextTokenException` case.
///
/// A token this service issued names a resource type, whether or not the catalog still
/// carries that one, so the test is on the FORM of the value:
/// `<service-code>:<resource-code>`.
pub fn decode_resource_type_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|value| {
        value
            .split_once(':')
            .is_some_and(|(service, code)| !service.is_empty() && !code.is_empty())
    })
}

fn decode_cursor(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// A digest of the parameters of one idempotent call, so a later call with the same
/// `clientToken` can be told apart from a repeat of the same request.
///
/// Each field is length-prefixed, so no two different parameter lists can produce
/// the same input to the hash.
pub fn fingerprint(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// One list parameter, rendered for [`fingerprint`]: sorted, so the same entities
/// in another order are the same request, as RAM's own set semantics imply.
pub fn fingerprint_list(values: &[String]) -> String {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.join(&FIELD_SEPARATOR.to_string())
}

/// One map parameter (a share's tags), rendered for [`fingerprint`] in key order.
pub fn fingerprint_map(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}{FIELD_SEPARATOR}{value}"))
        .collect::<Vec<_>>()
        .join(&FIELD_SEPARATOR.to_string())
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not a plain region token is refused here and the default is
/// used instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Epoch milliseconds as the modelled `DateTime` timestamps carry them.
pub fn to_date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, in milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Caller text, shortened for an error message: a refusal names the value it
/// refused, but never a whole 2 KiB ARN.
fn truncate(value: &str) -> String {
    const LIMIT: usize = 64;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }
    let head: String = value.chars().take(LIMIT).collect();
    format!("{head}…")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRamAmazonresourcesharingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRamAmazonresourcesharingState>)
}

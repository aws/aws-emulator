// SPDX-License-Identifier: CC0-1.0

//! The catalog of AWS managed permissions this service ships.
//!
//! RAM maintains managed permissions of its own for the resource types it shares, and
//! those permissions belong to Amazon Web Services rather than to an account: their
//! ARNs name no Region and carry `aws` where an account id would be
//! (`arn:aws:ram::aws:permission/<PermissionName>`), their `permissionType` is
//! `AWS_MANAGED` — "Amazon Web Services created and manages this managed permission.
//! You can associate it with your resource shares, but you can't modify it"
//! (`ResourceSharePermissionDetail$permissionType`) — and they exist before any caller
//! creates anything. So the store holds them from the moment it is first touched
//! (`ComAmazonawsRamAmazonresourcesharingState::default`), beside the customer managed
//! permissions `CreatePermission` mints, and every read that resolves a permission ARN
//! resolves these too.
//!
//! Source of the table below: the managed permissions AWS Resource Access Manager
//! publishes for its shareable resource types — the RAM Permissions library, which
//! `ListPermissions` with `permissionType` `AWS` reports and which
//! `GetPermission$permissionArn` points a caller at to discover them ("To find the ARN
//! for a permission, use either the ListPermissions operation or go to the Permissions
//! library page in the RAM console") — one entry per published permission, with the
//! resource type it applies to, the actions its policy allows and whether it is that
//! resource type's default. The model enumerates none of them, so the catalog is
//! written out here, exactly as the resource-type catalog in [`super::RESOURCE_TYPES`]
//! is; every entry's resource type is one of that catalog's.
//!
//! MEMBERSHIP is part of that data. `ListPermissions` is the only enumeration API for
//! managed permissions, so the catalog carries an entry per permission the library
//! publishes — a resource type whose owning service maintains a family of certificate
//! templates gets one entry per published permission of that family, the revocation
//! permission beside the issuance ones — and a name no permission is published under
//! carries no entry, so it resolves nowhere and an ARN a caller discovers here can be
//! attached. A plausible name is not a published one: a permission the library publishes
//! for one certificate template, or under one spelling of a type's name, is no reason to
//! record a sibling for every other template or spelling.
//!
//! So is each entry's published VERSION ([`AwsManagedPermission::version`]), the
//! `resourceType` its owning service publishes
//! ([`AwsManagedPermission::resource_type`]) and the is-resource-type-default flag
//! ([`AwsManagedPermission::is_resource_type_default`]). The library revises a managed
//! permission in place and publishes the revision as the permission's version, so an
//! entry carries the version the library publishes it at rather than one number for the
//! whole catalog: that is the version every read of it reports and the default version a
//! share attaches. Each of those three is read off the entry on every read and defaulted
//! nowhere.

use std::collections::BTreeMap;

use super::{
    PermissionRecord, PermissionVersionRecord, ARN_SERVICE, FIRST_PERMISSION_VERSION,
    PERMISSION_ARN_RESOURCE_PREFIX, PERMISSION_FEATURE_SET_STANDARD, PERMISSION_STATUS_ATTACHABLE,
    POLICY_EFFECT_ALLOW, POLICY_TEMPLATE_ACTION_ELEMENT, POLICY_TEMPLATE_EFFECT_ELEMENT,
};

/// The `PermissionType` of a permission Amazon Web Services created and manages, the
/// other value of the modelled `PermissionType` enum beside
/// [`super::PERMISSION_TYPE_CUSTOMER_MANAGED`].
pub const PERMISSION_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The partition field of every ARN this emulator reports.
pub const ARN_PARTITION: &str = "aws";

/// What an AWS managed permission ARN carries where an account id would be: the
/// permission is Amazon Web Services' own rather than any account's.
pub const AWS_MANAGED_PERMISSION_ARN_ACCOUNT: &str = "aws";

/// One entry of the AWS managed permission catalog.
#[derive(Debug, Clone, Copy)]
pub struct AwsManagedPermission {
    /// The permission's name, which is also the resource id of its ARN.
    pub name: &'static str,
    /// The resource type it applies to, `<service-code>:<resource-code>`; one of
    /// [`super::RESOURCE_TYPES`].
    ///
    /// The spelling is the one the OWNING service publishes for the type — the resource
    /// code of a resource's own ARN, which is what the shareable-resource-type catalog
    /// carries — rather than one derived from this permission's name: a permission named
    /// for a capability can govern a type of another service entirely, and a type the
    /// owning service publishes in the plural, or as a second version of an earlier type,
    /// is spelled that way here. A client filters a read by this string, so it is the
    /// published one or the read misses the permission.
    pub resource_type: &'static str,
    /// The actions its policy allows, which is what the `permission` document reports.
    pub actions: &'static [&'static str],
    /// Whether this is "the default managed permission for all resources of this
    /// resource type" (`ResourceSharePermissionDetail$isResourceTypeDefault`) — the
    /// one RAM attaches when a share names no permission for the type.
    ///
    /// A published property of the permission, not a position in the family: the default
    /// of a resource type the library publishes several permissions for is whichever one
    /// the library marks, which is not necessarily the first of them or the most
    /// permissive. Where the library marks more than one permission of a type, every one
    /// of them reports the flag, and the one this service attaches to a share that names
    /// none is [`default_aws_managed_permission`]'s.
    pub is_resource_type_default: bool,
    /// The version Amazon Web Services publishes this permission at, which is therefore
    /// its default version: a permission the library has revised since it was first
    /// published is published at the revision, and a read of it reports that number
    /// (`ResourceSharePermissionSummary$version`). Counting from
    /// [`FIRST_PERMISSION_VERSION`], the version every permission starts at.
    ///
    /// Per ENTRY, never per catalog: the library revises one permission without touching
    /// the next, so the permissions published for one resource type sit at different
    /// numbers and an entry carries the one the library publishes IT at — written with
    /// [`entry_at_version`] for a revision and with [`entry`] for a permission the library
    /// publishes at the first version. Reporting the first version for an entry whose
    /// published number is a revision points a caller at a policy that does not exist, so
    /// the number is the entry's own published one rather than a fallback.
    pub version: i32,
}

/// The resource type a private certificate authority's ARN names, spelled as
/// [`super::RESOURCE_TYPES`] publishes it. Named once because the permissions Amazon Web
/// Services publishes for it are a family rather than a single entry.
const CERTIFICATE_AUTHORITY_RESOURCE_TYPE: &str = "acm-pca:CertificateAuthority";

/// The actions every managed permission for ISSUING a certificate of a private
/// certificate authority allows: the reads a receiving account makes about the shared CA,
/// plus the issuance the share exists to grant.
///
/// The issuance permissions of that family differ from one another in the certificate
/// TEMPLATE a condition on the issuance admits — which is the `Condition` element of each
/// published policy, not its `Action` element — so every issuance entry of the family
/// allows the same actions. A catalog record carries a permission's effect and its actions
/// (`ResourceSharePermissionDetail$permission`, "The permission's effect and actions in JSON
/// format"), so the template condition is not a field of one.
const CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS: &[&str] = &[
    "acm-pca:DescribeCertificateAuthority",
    "acm-pca:GetCertificate",
    "acm-pca:GetCertificateAuthorityCertificate",
    "acm-pca:IssueCertificate",
    "acm-pca:ListPermissions",
    "acm-pca:ListTags",
];

/// The actions the managed permission for REVOKING a certificate of a shared private
/// certificate authority allows: the same reads the issuance permissions of the family
/// allow, with the revocation the permission exists to grant in place of the issuance.
const CERTIFICATE_AUTHORITY_REVOCATION_ACTIONS: &[&str] = &[
    "acm-pca:DescribeCertificateAuthority",
    "acm-pca:GetCertificate",
    "acm-pca:GetCertificateAuthorityCertificate",
    "acm-pca:ListPermissions",
    "acm-pca:ListTags",
    "acm-pca:RevokeCertificate",
];

/// The actions the managed permissions for an IP Address Manager pool allow: the reads
/// a receiving account makes about the shared pool, plus the allocation the share exists
/// to grant. Named once because the library publishes a family for the type.
const IPAM_POOL_ACTIONS: &[&str] = &[
    "ec2:AllocateIpamPoolCidr",
    "ec2:DescribeIpamPools",
    "ec2:GetIpamPoolAllocations",
    "ec2:GetIpamPoolCidrs",
];

/// The actions the managed permissions for a Network Firewall firewall policy allow.
const FIREWALL_POLICY_ACTIONS: &[&str] = &[
    "network-firewall:AssociateFirewallPolicy",
    "network-firewall:DescribeFirewallPolicy",
    "network-firewall:ListFirewallPolicies",
];

/// The actions a Network Firewall rule-group permission allows without the describe of
/// the rule group's own rules: the library publishes a rule-group permission in this
/// form and in the `WithDescribe` form below.
const RULE_GROUP_ACTIONS: &[&str] = &["network-firewall:ListRuleGroups"];

/// The actions a Network Firewall rule-group permission of the `WithDescribe` form
/// allows: the listing above plus the describe of the shared rule group itself.
const RULE_GROUP_WITH_DESCRIBE_ACTIONS: &[&str] = &[
    "network-firewall:DescribeRuleGroup",
    "network-firewall:ListRuleGroups",
];

/// The actions an agent-registry permission allows a reader of a shared registry.
const AGENT_REGISTRY_READ_ACTIONS: &[&str] = &[
    "agent-registry:GetAgent",
    "agent-registry:GetRegistry",
    "agent-registry:ListAgents",
];

/// The actions the agent-registry permissions for an account that maintains agents in a
/// shared registry allow: the reads above plus the agents' own lifecycle.
const AGENT_REGISTRY_PUBLISH_ACTIONS: &[&str] = &[
    "agent-registry:CreateAgent",
    "agent-registry:GetAgent",
    "agent-registry:GetRegistry",
    "agent-registry:ListAgents",
    "agent-registry:UpdateAgent",
];

/// The actions a Glue data-catalog permission allows a reader of the shared catalog.
const GLUE_CATALOG_READ_ACTIONS: &[&str] = &[
    "glue:GetCatalog",
    "glue:GetDatabase",
    "glue:GetDatabases",
    "glue:GetTable",
    "glue:GetTables",
];

/// The actions the Glue catalog permissions that grant a receiving account the DATABASES
/// of a shared catalog allow: the catalog's own reads plus the database lifecycle inside
/// it.
const GLUE_CATALOG_DATABASE_ACTIONS: &[&str] = &[
    "glue:CreateDatabase",
    "glue:DeleteDatabase",
    "glue:GetCatalog",
    "glue:GetDatabase",
    "glue:GetDatabases",
    "glue:UpdateDatabase",
];

/// The actions the Glue catalog permissions that grant a receiving account the TABLES of
/// a shared catalog allow.
const GLUE_CATALOG_TABLE_ACTIONS: &[&str] = &[
    "glue:CreateTable",
    "glue:DeleteTable",
    "glue:GetCatalog",
    "glue:GetDatabase",
    "glue:GetTable",
    "glue:GetTables",
    "glue:UpdateTable",
];

/// The actions the Glue database permissions that grant a receiving account the TABLES of
/// a shared database allow.
const GLUE_DATABASE_TABLE_ACTIONS: &[&str] = &[
    "glue:CreateTable",
    "glue:DeleteTable",
    "glue:GetDatabase",
    "glue:GetTable",
    "glue:GetTables",
    "glue:UpdateTable",
];

/// The actions the Glue table permissions that grant a receiving account the PARTITIONS
/// of a shared table allow.
const GLUE_TABLE_PARTITION_ACTIONS: &[&str] = &[
    "glue:BatchCreatePartition",
    "glue:CreatePartition",
    "glue:DeletePartition",
    "glue:GetPartition",
    "glue:GetPartitions",
    "glue:GetTable",
    "glue:UpdatePartition",
];

/// The actions a DataZone domain permission allows a reader of the shared domain.
const DATAZONE_DOMAIN_READ_ACTIONS: &[&str] = &[
    "datazone:GetDomain",
    "datazone:GetProject",
    "datazone:ListDomains",
    "datazone:ListProjects",
];

/// The actions the DataZone domain permissions that extend a shared domain to another
/// service's environments allow.
const DATAZONE_DOMAIN_SERVICE_ACTIONS: &[&str] = &[
    "datazone:GetDomain",
    "datazone:GetEnvironment",
    "datazone:ListEnvironments",
];

/// The actions the managed permissions for an Outpost allow.
const OUTPOST_ACTIONS: &[&str] = &[
    "outposts:GetOutpost",
    "outposts:GetOutpostInstanceTypes",
    "outposts:ListOutposts",
];

/// The actions the managed permissions for an Outposts site allow.
const OUTPOSTS_SITE_ACTIONS: &[&str] = &["outposts:GetSite", "outposts:ListSites"];

/// The actions a SageMaker feature-group permission allows a reader of the shared group.
const FEATURE_GROUP_READ_ACTIONS: &[&str] = &[
    "sagemaker:BatchGetRecord",
    "sagemaker:DescribeFeatureGroup",
    "sagemaker:GetRecord",
];

/// The actions the SageMaker feature-group permissions that grant a receiving account the
/// records of a shared group allow.
const FEATURE_GROUP_WRITE_ACTIONS: &[&str] = &[
    "sagemaker:BatchGetRecord",
    "sagemaker:DeleteRecord",
    "sagemaker:DescribeFeatureGroup",
    "sagemaker:GetRecord",
    "sagemaker:PutRecord",
];

/// The actions a SageMaker model-package-group permission allows a reader of the shared
/// group.
const MODEL_PACKAGE_GROUP_READ_ACTIONS: &[&str] = &[
    "sagemaker:DescribeModelPackage",
    "sagemaker:DescribeModelPackageGroup",
    "sagemaker:ListModelPackages",
];

/// The actions a SageMaker hub permission allows a reader of the shared hub.
const HUB_READ_ACTIONS: &[&str] = &[
    "sagemaker:DescribeHub",
    "sagemaker:DescribeHubContent",
    "sagemaker:ListHubContents",
];

/// The actions a Cloud Map namespace permission allows a reader of the shared namespace.
const SERVICE_DISCOVERY_READ_ACTIONS: &[&str] = &[
    "servicediscovery:DiscoverInstances",
    "servicediscovery:GetNamespace",
    "servicediscovery:ListServices",
];

/// The actions an AWS Systems Manager parameter permission allows a reader of the shared
/// parameter.
const SSM_PARAMETER_READ_ACTIONS: &[&str] = &[
    "ssm:DescribeParameters",
    "ssm:GetParameter",
    "ssm:GetParameters",
];

/// The actions an Amazon S3 Access Grants instance permission allows a reader of the
/// shared instance.
const ACCESS_GRANTS_READ_ACTIONS: &[&str] = &[
    "s3:GetAccessGrantsInstance",
    "s3:GetDataAccess",
    "s3:ListAccessGrants",
];

/// The actions a managed prefix list permission allows: the library publishes more than
/// one permission for the type.
const PREFIX_LIST_ACTIONS: &[&str] = &[
    "ec2:DescribeManagedPrefixLists",
    "ec2:GetManagedPrefixListEntries",
];

/// The actions an Amazon EBS volume permission allows a reader of the shared volume.
const EBS_VOLUME_READ_ACTIONS: &[&str] = &[
    "ec2:AttachVolume",
    "ec2:DescribeVolumes",
    "ec2:DetachVolume",
];

/// The AWS managed permissions this service serves, one entry per published
/// permission. See the module documentation for the source.
pub const AWS_MANAGED_PERMISSIONS: &[AwsManagedPermission] = &[
    entry(
        "AWSRAMDefaultPermissionCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMBlankEndEntityCertificateAPIPassthroughIssuanceCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMSubordinateCACertificatePathLen0IssuanceCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMRevokeCertificateCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_REVOCATION_ACTIONS,
        false,
    ),
    entry(
        "APIGatewayPrivateDomainNameManagementPolicyDefaultPermission",
        "apigateway:Domainnames",
        &["apigateway:CreateAccessAssociation"],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionAppMesh",
        "appmesh:Mesh",
        &[
            "appmesh:CreateVirtualNode",
            "appmesh:DescribeMesh",
            "appmesh:DescribeVirtualNode",
            "appmesh:ListVirtualNodes",
        ],
        true,
        3,
    ),
    entry(
        "AWSRAMDefaultPermissionCodeBuildProject",
        "codebuild:Project",
        &[
            "codebuild:BatchGetProjects",
            "codebuild:ListBuildsForProject",
            "codebuild:StartBuild",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionCodeBuildReportGroup",
        "codebuild:ReportGroup",
        &[
            "codebuild:BatchGetReportGroups",
            "codebuild:BatchGetReports",
            "codebuild:ListReportsForReportGroup",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionCapacityReservation",
        "ec2:CapacityReservation",
        &["ec2:DescribeCapacityReservations", "ec2:RunInstances"],
        true,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionDedicatedHost",
        "ec2:DedicatedHost",
        &["ec2:DescribeHosts", "ec2:RunInstances"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionPrefixList",
        "ec2:PrefixList",
        PREFIX_LIST_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionSubnet",
        "ec2:Subnet",
        &[
            "ec2:CreateNetworkInterface",
            "ec2:DescribeSubnets",
            "ec2:RunInstances",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionTransitGateway",
        "ec2:TransitGateway",
        &[
            "ec2:CreateTransitGatewayVpcAttachment",
            "ec2:DeleteTransitGatewayVpcAttachment",
            "ec2:DescribeTransitGatewayVpcAttachments",
            "ec2:DescribeTransitGateways",
        ],
        true,
        3,
    ),
    entry(
        "AWSRAMDefaultPermissionGlueDatabase",
        "glue:Database",
        &[
            "glue:GetDatabase",
            "glue:GetDatabases",
            "glue:GetTable",
            "glue:GetTables",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueDatabaseReadWrite",
        "glue:Database",
        &[
            "glue:CreateTable",
            "glue:DeleteTable",
            "glue:GetDatabase",
            "glue:GetTable",
            "glue:UpdateTable",
        ],
        false,
        3,
    ),
    entry(
        "AWSRAMDefaultPermissionGlueTable",
        "glue:Table",
        &[
            "glue:GetPartition",
            "glue:GetPartitions",
            "glue:GetTable",
            "glue:GetTables",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionImageBuilderComponent",
        "imagebuilder:Component",
        &[
            "imagebuilder:GetComponent",
            "imagebuilder:ListComponentBuildVersions",
            "imagebuilder:ListComponents",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionImageBuilderContainerRecipe",
        "imagebuilder:ContainerRecipe",
        &[
            "imagebuilder:GetContainerRecipe",
            "imagebuilder:ListContainerRecipes",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionImageBuilderImage",
        "imagebuilder:Image",
        &[
            "imagebuilder:GetImage",
            "imagebuilder:ListImageBuildVersions",
            "imagebuilder:ListImages",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionImageBuilderImageRecipe",
        "imagebuilder:ImageRecipe",
        &[
            "imagebuilder:GetImageRecipe",
            "imagebuilder:ListImageRecipes",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionLicenseConfiguration",
        "license-manager:LicenseConfiguration",
        &[
            "license-manager:GetLicenseConfiguration",
            "license-manager:ListLicenseSpecificationsForResource",
            "license-manager:ListUsageForLicenseConfiguration",
            "license-manager:UpdateLicenseSpecificationsForResource",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionResourceGroup",
        "resource-groups:Group",
        &[
            "resource-groups:GetGroup",
            "resource-groups:GetGroupConfiguration",
            "resource-groups:ListGroupResources",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionResolverRule",
        "route53resolver:ResolverRule",
        &[
            "route53resolver:AssociateResolverRule",
            "route53resolver:DisassociateResolverRule",
            "route53resolver:GetResolverRule",
            "route53resolver:ListResolverRuleAssociations",
        ],
        true,
    ),
    // The rest of the private-CA issuance family: one published permission per
    // certificate template the shared certificate authority may issue under.
    entry(
        "AWSRAMEndEntityClientAuthCertificateIssuanceCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMEndEntityServerAuthCertificateIssuanceCertificateAuthority",
        CERTIFICATE_AUTHORITY_RESOURCE_TYPE,
        CERTIFICATE_AUTHORITY_ISSUANCE_ACTIONS,
        false,
    ),
    // An agent registry: the read-only permission is the type's default and the three
    // role-shaped ones are published beside it.
    entry_at_version(
        "AWSRAMDefaultPermissionAgentRegistryReadOnly",
        "agent-registry:Registry",
        AGENT_REGISTRY_READ_ACTIONS,
        true,
        2,
    ),
    entry_at_version(
        "AWSRAMPermissionAgentRegistryForAdmin",
        "agent-registry:Registry",
        &[
            "agent-registry:CreateAgent",
            "agent-registry:DeleteAgent",
            "agent-registry:GetAgent",
            "agent-registry:GetRegistry",
            "agent-registry:ListAgents",
            "agent-registry:UpdateAgent",
        ],
        false,
        2,
    ),
    entry(
        "AWSRAMPermissionAgentRegistryForConsumer",
        "agent-registry:Registry",
        AGENT_REGISTRY_READ_ACTIONS,
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionAgentRegistryForPublisher",
        "agent-registry:Registry",
        AGENT_REGISTRY_PUBLISH_ACTIONS,
        false,
        2,
    ),
    // An API Gateway developer-portal product.
    entry(
        "AWSRAMDefaultPermissionAPIGatewayDeveloperPortalProduct",
        "apigateway:Portalproducts",
        &[
            "apigateway:GetPortalProduct",
            "apigateway:GetPortalProducts",
        ],
        true,
    ),
    // An App Mesh Preview service mesh, published beside the App Mesh one above.
    entry_at_version(
        "AWSRAMDefaultPermissionAppMeshPreview",
        "appmesh-preview:Mesh",
        &[
            "appmesh-preview:CreateVirtualNode",
            "appmesh-preview:DescribeMesh",
            "appmesh-preview:DescribeVirtualNode",
            "appmesh-preview:ListVirtualNodes",
        ],
        true,
        3,
    ),
    // An AWS AppSync API: a source API shared with a merged API, and the invoke paths
    // published for it.
    entry(
        "AWSRAMPermissionAppSyncSourceApiOperationAccess",
        "appsync:Apis",
        &[
            "appsync:AssociateMergedGraphqlApi",
            "appsync:GetGraphqlApi",
            "appsync:ListGraphqlApis",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionAppSyncMergedApiOperationAccess",
        "appsync:Apis",
        &[
            "appsync:AssociateSourceGraphqlApi",
            "appsync:GetGraphqlApi",
            "appsync:ListGraphqlApis",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionAppSyncAllowSourceGraphQLAccess",
        "appsync:Apis",
        &["appsync:GetGraphqlApi", "appsync:SourceGraphQL"],
        false,
    ),
    entry(
        "AWSRAMPermissionAppSyncGraphQLApiInvokeAccess",
        "appsync:Apis",
        &["appsync:GetGraphqlApi", "appsync:GraphQL"],
        false,
    ),
    // An AWS Backup vault.
    entry_at_version(
        "AWSRAMPermissionBackupVaultReadOnly",
        "backup:BackupVault",
        &[
            "backup:DescribeBackupVault",
            "backup:DescribeRecoveryPoint",
            "backup:ListRecoveryPointsByBackupVault",
        ],
        true,
        3,
    ),
    // An Amazon Bedrock custom model.
    entry(
        "AWSRAMPermissionBedrockCustomModel",
        "bedrock:CustomModel",
        &[
            "bedrock:GetCustomModel",
            "bedrock:InvokeModel",
            "bedrock:ListCustomModels",
        ],
        true,
    ),
    // An AWS CloudHSM backup.
    entry(
        "AWSRAMPermissionCloudHsmBackup",
        "cloudhsm:Backup",
        &["cloudhsm:DescribeBackups", "cloudhsm:ListTags"],
        true,
    ),
    // An AWS CodeConnections connection.
    entry(
        "AWSRAMPermissionCodeConnectionsConnection",
        "codeconnections:Connection",
        &[
            "codeconnections:GetConnection",
            "codeconnections:ListConnections",
            "codeconnections:UseConnection",
        ],
        true,
    ),
    // An Amazon DataZone domain: the domain's own default, the portal-access variants,
    // and the permissions another service's principal uses against a shared domain.
    entry_at_version(
        "AWSRAMDefaultPermissionAmazonDataZoneDomain",
        "datazone:Domain",
        DATAZONE_DOMAIN_READ_ACTIONS,
        true,
        10,
    ),
    entry_at_version(
        "AWSRAMPermissionAmazonDataZoneDomainFullAccessWithPortalAccess",
        "datazone:Domain",
        &[
            "datazone:GetDomain",
            "datazone:GetIamPortalLoginUrl",
            "datazone:GetProject",
            "datazone:ListProjects",
        ],
        false,
        7,
    ),
    entry_at_version(
        "AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceAccess",
        "datazone:Domain",
        DATAZONE_DOMAIN_SERVICE_ACTIONS,
        false,
        10,
    ),
    entry_at_version(
        "AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceWithPortalAccess",
        "datazone:Domain",
        &[
            "datazone:GetDomain",
            "datazone:GetEnvironment",
            "datazone:GetIamPortalLoginUrl",
            "datazone:ListEnvironments",
        ],
        false,
        8,
    ),
    entry_at_version(
        "AWSRAMSageMakerServicePrincipalPermissionAmazonDataZoneDomain",
        "datazone:Domain",
        DATAZONE_DOMAIN_SERVICE_ACTIONS,
        false,
        3,
    ),
    entry(
        "AWSRAMPermissionSageMakerDataZoneProjectMembershipAccess",
        "datazone:Domain",
        &[
            "datazone:GetDomain",
            "datazone:GetProject",
            "datazone:ListProjectMemberships",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionSageMakerCatalogResourceSearch",
        "sagemaker:SagemakerCatalog",
        &[
            "sagemaker:DescribeFeatureGroup",
            "sagemaker:ListFeatureGroups",
            "sagemaker:Search",
        ],
        true,
    ),
    // Amazon EC2 and Amazon VPC resources the library publishes a permission for beside
    // the ones above.
    entry(
        "AWSRAMDefaultPermissionCoipPool",
        "ec2:CoipPool",
        &["ec2:DescribeCoipPools", "ec2:GetCoipPoolUsage"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionEBSVolume",
        "ec2:Volume",
        EBS_VOLUME_READ_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMPermissionEBSVolumeCopyAccess",
        "ec2:Volume",
        &[
            "ec2:CreateSnapshot",
            "ec2:DescribeSnapshots",
            "ec2:DescribeVolumes",
        ],
        false,
    ),
    entry(
        "AWSRAMDefaultPermissionLocalGateway",
        "ec2:LocalGatewayRouteTable",
        &[
            "ec2:CreateLocalGatewayRouteTableVpcAssociation",
            "ec2:DescribeLocalGatewayRouteTables",
            "ec2:DescribeLocalGateways",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionsIpamPool",
        "ec2:IpamPool",
        IPAM_POOL_ACTIONS,
        true,
        4,
    ),
    entry(
        "AWSRAMPermissionIpamPoolByoipCidrImport",
        "ec2:IpamPool",
        &["ec2:DescribeIpamPools", "ec2:ProvisionPublicIpv4PoolCidr"],
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionIpamResourceDiscovery",
        "ec2:IpamResourceDiscovery",
        &[
            "ec2:DescribeIpamResourceDiscoveries",
            "ec2:GetIpamDiscoveredAccounts",
            "ec2:GetIpamDiscoveredResourceCidrs",
        ],
        true,
        3,
    ),
    entry(
        "AWSRAMDefaultPermissionsPlacementGroup",
        "ec2:PlacementGroup",
        &["ec2:DescribePlacementGroups", "ec2:RunInstances"],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionsSecurityGroup",
        "ec2:SecurityGroup",
        &[
            "ec2:DescribeSecurityGroupRules",
            "ec2:DescribeSecurityGroups",
        ],
        true,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionTrafficMirror",
        "ec2:TrafficMirrorTarget",
        &[
            "ec2:CreateTrafficMirrorSession",
            "ec2:DescribeTrafficMirrorTargets",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionTransitGatewayMulticastDomain",
        "ec2:TransitGatewayMulticastDomain",
        &[
            "ec2:AssociateTransitGatewayMulticastDomain",
            "ec2:DescribeTransitGatewayMulticastDomains",
            "ec2:RegisterTransitGatewayMulticastGroupMembers",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionVerifiedAccessGroup",
        "ec2:VerifiedAccessGroup",
        &[
            "ec2:CreateVerifiedAccessEndpoint",
            "ec2:DescribeVerifiedAccessGroups",
        ],
        true,
    ),
    entry(
        "AWSRAMFSxRosettaPermissionPrefixList",
        "ec2:PrefixList",
        PREFIX_LIST_ACTIONS,
        false,
    ),
    // An Elastic Load Balancing trust store.
    entry(
        "AWSRAMPermissionElasticLoadBalancingTrustStore",
        "elasticloadbalancing:TrustStore",
        &[
            "elasticloadbalancing:DescribeTrustStores",
            "elasticloadbalancing:GetTrustStoreCaCertificatesBundle",
        ],
        true,
    ),
    // An Amazon EventBridge event bus, one permission per access shape the library
    // publishes for it.
    entry(
        "AWSRAMEventBridgeEventBusV2FullAccess",
        "events:EventBusv2",
        &[
            "events:DeleteRule",
            "events:DescribeEventBus",
            "events:ListRules",
            "events:PutEvents",
            "events:PutRule",
            "events:PutTargets",
        ],
        false,
    ),
    entry(
        "AWSRAMEventBridgeEventBusV2EventSourceAccess",
        "events:EventBusv2",
        &["events:DescribeEventBus", "events:PutEvents"],
        false,
    ),
    entry(
        "AWSRAMEventBridgeEventBusV2PublishOnly",
        "events:EventBusv2",
        &["events:DescribeEventBus", "events:PutEvents"],
        false,
    ),
    entry(
        "AWSRAMEventBridgeEventBusV2SubscribeOnly",
        "events:EventBusv2",
        &[
            "events:DescribeEventBus",
            "events:ListRules",
            "events:PutRule",
            "events:PutTargets",
        ],
        true,
    ),
    // An Amazon FSx volume.
    entry(
        "AWSRAMDefaultPermissionFsx",
        "fsx:Volume",
        &["fsx:DescribeFileSystems", "fsx:DescribeVolumes"],
        true,
    ),
    // The AWS Glue and Lake Formation families: one default per Glue resource type, and
    // the read-write and LF-tag variants the library publishes beside them, each named
    // for the resource type it is attached to.
    entry(
        "AWSRAMDefaultPermissionGlueCatalog",
        "glue:Catalog",
        GLUE_CATALOG_READ_ACTIONS,
        true,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueAllTablesReadWriteForCatalog",
        "glue:Catalog",
        GLUE_CATALOG_TABLE_ACTIONS,
        false,
        3,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueDatabaseReadWriteForCatalog",
        "glue:Catalog",
        GLUE_CATALOG_DATABASE_ACTIONS,
        false,
        3,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueTableReadWriteForCatalog",
        "glue:Catalog",
        GLUE_CATALOG_TABLE_ACTIONS,
        false,
        3,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueDatabaseReadWriteForCatalog",
        "glue:Catalog",
        GLUE_CATALOG_DATABASE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueTableReadWriteForCatalog",
        "glue:Catalog",
        GLUE_CATALOG_TABLE_ACTIONS,
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueAllTablesReadWriteForDatabase",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
        3,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueTableReadWriteForDatabase",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
        3,
    ),
    entry(
        "AWSRAMLFEnabledGlueAllTablesReadWriteForDatabase",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMLFEnabledGlueDatabaseReadWrite",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueDatabaseReadWrite",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueTableReadWriteForDatabase",
        "glue:Database",
        GLUE_DATABASE_TABLE_ACTIONS,
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueTableReadWrite",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
        2,
    ),
    entry_at_version(
        "AWSRAMPermissionGlueDatabaseReadWriteForTable",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
        2,
    ),
    entry(
        "AWSRAMLFEnabledGlueDatabaseReadWriteForTable",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMLFEnabledGlueTableReadWrite",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueDatabaseReadWriteForTable",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionLFTagGlueTableReadWrite",
        "glue:Table",
        GLUE_TABLE_PARTITION_ACTIONS,
        false,
    ),
    // AWS Network Firewall: the firewall policy's own default, the rule-group variants
    // published with and without the describe of the shared rule group, the firewall
    // itself and the container association.
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallPolicy",
        "network-firewall:FirewallPolicy",
        FIREWALL_POLICY_ACTIONS,
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroup",
        "network-firewall:StatefulRulegroup",
        RULE_GROUP_ACTIONS,
        true,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroupWithDescribe",
        "network-firewall:StatefulRulegroup",
        RULE_GROUP_WITH_DESCRIBE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallStatelessRuleGroup",
        "network-firewall:StatelessRulegroup",
        RULE_GROUP_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallStatelessRuleGroupWithDescribe",
        "network-firewall:StatelessRulegroup",
        RULE_GROUP_WITH_DESCRIBE_ACTIONS,
        false,
    ),
    entry(
        "AWSRAMPermissionNetworkFirewallFirewall",
        "network-firewall:Firewall",
        &[
            "network-firewall:AssociateSubnets",
            "network-firewall:DescribeFirewall",
            "network-firewall:ListFirewalls",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallContainerAssociation",
        "network-firewall:ContainerAssociation",
        &[
            "network-firewall:DescribeFirewall",
            "network-firewall:ListFirewalls",
        ],
        true,
    ),
    // Oracle Database@AWS resources.
    entry(
        "AWSRAMDefaultPermissionODBCloudExadataInfrastructure",
        "odb:CloudExadataInfrastructure",
        &[
            "odb:GetCloudExadataInfrastructure",
            "odb:ListCloudExadataInfrastructures",
            "odb:ListDbServers",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionODBExascaleDbStorageVault",
        "odb:ExascaleDbStorageVault",
        &[
            "odb:GetExascaleDbStorageVault",
            "odb:ListExascaleDbStorageVaults",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionODBNetwork",
        "odb:OdbNetwork",
        &["odb:GetOdbNetwork", "odb:ListOdbNetworks"],
        true,
        2,
    ),
    // AWS Outposts: the permissions published beside the Outpost and site defaults
    // above, including the one that shares an Outpost's Amazon S3 capacity.
    entry(
        "AWSRAMDefaultPermissionOutpostsOutpost",
        "outposts:Outpost",
        OUTPOST_ACTIONS,
        true,
    ),
    // The S3 on Outposts capacity of a shared Outpost is governed by its own published
    // permission, which is that resource type's default.
    entry_at_version(
        "AWSRAMDefaultPermissionS3Outposts",
        "s3-outposts:Outpost",
        &["s3-outposts:CreateEndpoint", "s3-outposts:ListEndpoints"],
        true,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionOutpostsSite",
        "outposts:Site",
        OUTPOSTS_SITE_ACTIONS,
        true,
    ),
    // An Amazon Aurora DB cluster.
    entry(
        "AWSRAMDefaultPermissionRDSCluster",
        "rds:Cluster",
        &[
            "rds:DescribeDBClusters",
            "rds:DescribeDBInstances",
            "rds:DescribeDBSubnetGroups",
        ],
        true,
    ),
    // An AWS Migration Hub Refactor Spaces environment.
    entry_at_version(
        "AWSRAMDefaultPermissionRefactorSpacesEnvironment",
        "refactor-spaces:Environment",
        &[
            "refactor-spaces:CreateApplication",
            "refactor-spaces:GetApplication",
            "refactor-spaces:GetEnvironment",
            "refactor-spaces:ListApplications",
        ],
        true,
        2,
    ),
    // An AWS Resource Explorer view.
    entry(
        "AWSRAMPermissionResourceExplorerViews",
        "resource-explorer-2:View",
        &[
            "resource-explorer-2:GetView",
            "resource-explorer-2:ListViews",
            "resource-explorer-2:Search",
        ],
        true,
    ),
    // An Amazon Route 53 profile.
    entry(
        "AWSRAMPermissionRoute53ProfileAllowAssociation",
        "route53profiles:Profile",
        &[
            "route53profiles:AssociateProfile",
            "route53profiles:DisassociateProfile",
            "route53profiles:GetProfile",
            "route53profiles:ListProfiles",
        ],
        true,
    ),
    // The Route 53 Resolver resources the library publishes a permission for beside the
    // resolver rule above.
    entry(
        "AWSRAMDefaultPermissionResolverFirewallRuleGroup",
        "route53resolver:FirewallRuleGroup",
        &[
            "route53resolver:AssociateFirewallRuleGroup",
            "route53resolver:DisassociateFirewallRuleGroup",
            "route53resolver:GetFirewallRuleGroup",
            "route53resolver:ListFirewallRuleGroups",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionResolverQueryLogConfig",
        "route53resolver:ResolverQueryLogConfig",
        &[
            "route53resolver:AssociateResolverQueryLogConfig",
            "route53resolver:DisassociateResolverQueryLogConfig",
            "route53resolver:GetResolverQueryLogConfig",
            "route53resolver:ListResolverQueryLogConfigs",
        ],
        true,
    ),
    // An Amazon S3 Access Grants instance.
    entry_at_version(
        "AWSRAMPermissionAccessGrantsReadAccess",
        "s3:AccessGrants",
        ACCESS_GRANTS_READ_ACTIONS,
        false,
        2,
    ),
    entry(
        "AWSRAMPermissionAccessGrantsControl",
        "s3:AccessGrants",
        &[
            "s3:CreateAccessGrant",
            "s3:DeleteAccessGrant",
            "s3:GetAccessGrantsInstance",
            "s3:ListAccessGrants",
        ],
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionAccessGrantsData",
        "s3:AccessGrants",
        &["s3:GetAccessGrantsInstance", "s3:GetDataAccess"],
        true,
        2,
    ),
    // Amazon SageMaker AI resources, one default per shareable type and the variants the
    // library publishes beside each.
    entry_at_version(
        "AWSRAMPermissionSageMakerFeatureGroupReadOnly",
        "sagemaker:FeatureGroup",
        FEATURE_GROUP_READ_ACTIONS,
        true,
        2,
    ),
    entry_at_version(
        "AWSRAMPermissionSageMakerFeatureGroupReadWrite",
        "sagemaker:FeatureGroup",
        FEATURE_GROUP_WRITE_ACTIONS,
        false,
        2,
    ),
    entry_at_version(
        "AWSRAMPermissionSageMakerFeatureGroupAdmin",
        "sagemaker:FeatureGroup",
        FEATURE_GROUP_WRITE_ACTIONS,
        false,
        2,
    ),
    entry(
        "AWSRAMPermissionSagemakerHubReadOnly",
        "sagemaker:Hub",
        HUB_READ_ACTIONS,
        false,
    ),
    entry_at_version(
        "AWSRAMPermissionSagemakerHubReadAndUse",
        "sagemaker:Hub",
        &[
            "sagemaker:DescribeHub",
            "sagemaker:DescribeHubContent",
            "sagemaker:ImportHubContent",
            "sagemaker:ListHubContents",
        ],
        true,
        2,
    ),
    entry_at_version(
        "AWSRAMPermissionSagemakerHubFullAccess",
        "sagemaker:Hub",
        &[
            "sagemaker:DeleteHubContent",
            "sagemaker:DescribeHub",
            "sagemaker:DescribeHubContent",
            "sagemaker:ImportHubContent",
            "sagemaker:ListHubContents",
        ],
        false,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionsSageMakerLineageGroup",
        "sagemaker:LineageGroup",
        &[
            "sagemaker:DescribeLineageGroup",
            "sagemaker:ListLineageGroups",
            "sagemaker:QueryLineage",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionMlflowDefaultPolicy",
        "sagemaker:MlflowApp",
        &[
            "sagemaker:CreatePresignedMlflowAppUrl",
            "sagemaker:DescribeMlflowApp",
            "sagemaker:ListMlflowApps",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionSageMakerModelCards",
        "sagemaker:ModelCard",
        &[
            "sagemaker:DescribeModelCard",
            "sagemaker:ListModelCardVersions",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionSageMakerModelCardsAllowExport",
        "sagemaker:ModelCard",
        &[
            "sagemaker:CreateModelCardExportJob",
            "sagemaker:DescribeModelCard",
            "sagemaker:DescribeModelCardExportJob",
        ],
        false,
    ),
    entry(
        "AWSRAMDefaultPermissionSageMakerModelPackageGroup",
        "sagemaker:ModelPackageGroup",
        MODEL_PACKAGE_GROUP_READ_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMPermissionSageMakerModelPackageGroupAllowDeploy",
        "sagemaker:ModelPackageGroup",
        &[
            "sagemaker:CreateModel",
            "sagemaker:DescribeModelPackage",
            "sagemaker:DescribeModelPackageGroup",
            "sagemaker:ListModelPackages",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionSageMakerModelPackageGroupAllowRegister",
        "sagemaker:ModelPackageGroup",
        &[
            "sagemaker:CreateModelPackage",
            "sagemaker:DescribeModelPackage",
            "sagemaker:DescribeModelPackageGroup",
            "sagemaker:ListModelPackages",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionPartnerAppDefaultPolicy",
        "sagemaker:PartnerApp",
        &[
            "sagemaker:CallPartnerAppApi",
            "sagemaker:DescribePartnerApp",
            "sagemaker:ListPartnerApps",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionSageMakerPipeline",
        "sagemaker:Pipeline",
        &[
            "sagemaker:DescribePipeline",
            "sagemaker:ListPipelineExecutions",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionSageMakerPipelineAllowExecution",
        "sagemaker:Pipeline",
        &[
            "sagemaker:DescribePipeline",
            "sagemaker:ListPipelineExecutions",
            "sagemaker:StartPipelineExecution",
        ],
        false,
    ),
    // AWS Service Catalog AppRegistry applications and attribute groups.
    entry(
        "AWSRAMPermissionServiceCatalogAppRegistryApplicationAllowAssociation",
        "servicecatalog:Applications",
        &[
            "servicecatalog:AssociateResource",
            "servicecatalog:DisassociateResource",
            "servicecatalog:GetApplication",
            "servicecatalog:ListAssociatedResources",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionServiceCatalogAppRegistryApplicationReadOnly",
        "servicecatalog:Applications",
        &[
            "servicecatalog:GetApplication",
            "servicecatalog:ListAssociatedResources",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionServiceCatalogAppRegistryAttributeGroupAllowAssociation",
        "servicecatalog:AttributeGroups",
        &[
            "servicecatalog:AssociateAttributeGroup",
            "servicecatalog:DisassociateAttributeGroup",
            "servicecatalog:GetAttributeGroup",
        ],
        false,
    ),
    entry(
        "AWSRAMPermissionServiceCatalogAppRegistryAttributeGroupReadOnly",
        "servicecatalog:AttributeGroups",
        &[
            "servicecatalog:GetAttributeGroup",
            "servicecatalog:ListAttributeGroups",
        ],
        true,
    ),
    // An AWS Cloud Map namespace.
    entry(
        "AWSRAMDefaultPermissionServiceDiscoveryNamespace",
        "servicediscovery:Namespace",
        SERVICE_DISCOVERY_READ_ACTIONS,
        true,
    ),
    entry_at_version(
        "AWSRAMPermissionCloudMapECSFullPermission",
        "servicediscovery:Namespace",
        &[
            "servicediscovery:CreateService",
            "servicediscovery:DeleteService",
            "servicediscovery:DeregisterInstance",
            "servicediscovery:DiscoverInstances",
            "servicediscovery:GetNamespace",
            "servicediscovery:ListServices",
            "servicediscovery:RegisterInstance",
        ],
        false,
        2,
    ),
    // AWS End User Messaging resources: one published permission per shareable type.
    entry(
        "AWSRAMPermissionSMSVoiceOptOutList",
        "sms-voice:OptOutList",
        &[
            "sms-voice:DescribeOptOutLists",
            "sms-voice:ListOptedOutNumbers",
            "sms-voice:PutOptedOutNumber",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionSMSVoicePhoneNumber",
        "sms-voice:PhoneNumber",
        &[
            "sms-voice:DescribePhoneNumbers",
            "sms-voice:SendTextMessage",
            "sms-voice:SendVoiceMessage",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionSMSVoicePool",
        "sms-voice:Pool",
        &["sms-voice:DescribePools", "sms-voice:SendTextMessage"],
        true,
    ),
    entry(
        "AWSRAMPermissionSMSVoiceRCSAgent",
        "sms-voice:RcsAgent",
        &["sms-voice:SendMediaMessage", "sms-voice:SendTextMessage"],
        true,
    ),
    entry(
        "AWSRAMPermissionSMSVoiceSenderId",
        "sms-voice:SenderId",
        &["sms-voice:DescribeSenderIds", "sms-voice:SendTextMessage"],
        true,
    ),
    // AWS Systems Manager resources.
    entry_at_version(
        "AWSRAMPermissionSsmDocument",
        "ssm:Document",
        &[
            "ssm:DescribeDocument",
            "ssm:GetDocument",
            "ssm:ListDocuments",
            "ssm:SendCommand",
            "ssm:StartAutomationExecution",
        ],
        true,
        2,
    ),
    entry(
        "AWSRAMDefaultPermissionSSMParameterReadOnly",
        "ssm:Parameter",
        SSM_PARAMETER_READ_ACTIONS,
        true,
    ),
    entry(
        "AWSRAMPermissionSSMParameterReadOnlyWithHistory",
        "ssm:Parameter",
        &[
            "ssm:GetParameter",
            "ssm:GetParameterHistory",
            "ssm:GetParameters",
        ],
        false,
    ),
    entry(
        "AWSRAMDefaultPermissionServerManageability",
        "ec2:DedicatedHost",
        &[
            "ssm:DescribeInstanceInformation",
            "ssm:SendCommand",
            "ssm:StartSession",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionSSMContactsContact",
        "ssm-contacts:Contact",
        &[
            "ssm-contacts:GetContact",
            "ssm-contacts:ListContacts",
            "ssm-contacts:StartEngagement",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMDefaultPermissionSSMIncidentsResponsePlan",
        "ssm-incidents:ResponsePlan",
        &[
            "ssm-incidents:GetResponsePlan",
            "ssm-incidents:ListResponsePlans",
            "ssm-incidents:StartIncident",
        ],
        true,
        2,
    ),
    // Amazon VPC Lattice resources beside the service network default above.
    entry_at_version(
        "AWSRAMPermissionVpcLatticeServiceNetworkReadWrite",
        "vpc-lattice:ServiceNetwork",
        &[
            "vpc-lattice:CreateServiceNetworkServiceAssociation",
            "vpc-lattice:CreateServiceNetworkVpcAssociation",
            "vpc-lattice:GetServiceNetwork",
            "vpc-lattice:ListServiceNetworks",
        ],
        true,
        2,
    ),
    entry(
        "AWSRAMPermissionVpcLatticeServiceReadWrite",
        "vpc-lattice:Service",
        &[
            "vpc-lattice:CreateServiceNetworkServiceAssociation",
            "vpc-lattice:GetService",
            "vpc-lattice:ListServices",
        ],
        true,
    ),
    entry_at_version(
        "AWSRAMPermissionVpcLatticeResourceConfiguration",
        "vpc-lattice:ResourceConfiguration",
        &[
            "vpc-lattice:CreateResourceEndpointAssociation",
            "vpc-lattice:GetResourceConfiguration",
            "vpc-lattice:ListResourceConfigurations",
        ],
        true,
        2,
    ),
];

/// One catalog entry, at the version a permission is first published at.
///
/// An entry written this way is one the library publishes at its FIRST version — it has
/// not been revised since publication — while one published at a revision is written with
/// [`entry_at_version`] and carries that number.
const fn entry(
    name: &'static str,
    resource_type: &'static str,
    actions: &'static [&'static str],
    is_resource_type_default: bool,
) -> AwsManagedPermission {
    entry_at_version(
        name,
        resource_type,
        actions,
        is_resource_type_default,
        FIRST_PERMISSION_VERSION,
    )
}

/// One catalog entry whose permission the library has REVISED since it was first
/// published, so the version it publishes — and the one every read of it reports — is
/// `version` rather than the first.
const fn entry_at_version(
    name: &'static str,
    resource_type: &'static str,
    actions: &'static [&'static str],
    is_resource_type_default: bool,
    version: i32,
) -> AwsManagedPermission {
    AwsManagedPermission {
        name,
        resource_type,
        actions,
        is_resource_type_default,
        version,
    }
}

/// The ARN of the AWS managed permission `name`.
///
/// An AWS managed permission is Amazon Web Services' own and exists in every Region,
/// so — unlike [`super::permission_arn`], which names a customer managed permission in
/// one account and Region — its ARN carries no Region and
/// [`AWS_MANAGED_PERMISSION_ARN_ACCOUNT`] in place of an account id.
pub fn aws_managed_permission_arn(name: &str) -> String {
    format!(
        "arn:{ARN_PARTITION}:{ARN_SERVICE}::{AWS_MANAGED_PERMISSION_ARN_ACCOUNT}:\
         {PERMISSION_ARN_RESOURCE_PREFIX}{name}"
    )
}

impl AwsManagedPermission {
    /// This entry's ARN.
    pub fn arn(&self) -> String {
        aws_managed_permission_arn(self.name)
    }

    /// The policy document this entry's version reports: the effect and the actions,
    /// the two elements `ResourceSharePermissionDetail$permission` describes ("The
    /// permission's effect and actions in JSON format").
    pub fn policy_document(&self) -> String {
        ::serde_json::json!({
            POLICY_TEMPLATE_EFFECT_ELEMENT: POLICY_EFFECT_ALLOW,
            POLICY_TEMPLATE_ACTION_ELEMENT: self.actions,
        })
        .to_string()
    }

    /// This entry as the record the store holds, created at `now`.
    ///
    /// Amazon Web Services publishes each of these permissions at one version
    /// ([`AwsManagedPermission::version`]), which is therefore its default version, and
    /// that version is `ATTACHABLE`: it "can be associated with resource shares"
    /// (`ResourceSharePermissionDetail$status`), which is the whole point of the catalog.
    /// A permission nobody can modify carries no tags.
    fn record(&self, now: i64) -> PermissionRecord {
        let version = self.version;
        PermissionRecord {
            arn: self.arn(),
            name: self.name.to_string(),
            resource_type: self.resource_type.to_string(),
            permission_type: PERMISSION_TYPE_AWS_MANAGED.to_string(),
            feature_set: PERMISSION_FEATURE_SET_STANDARD.to_string(),
            tags: BTreeMap::new(),
            creation_time: now,
            last_updated_time: now,
            default_version: version,
            is_resource_type_default: self.is_resource_type_default,
            // A permission Amazon Web Services publishes is attachable, and no caller
            // can deprecate one: `UpdatePermission` acts on a customer managed
            // permission only.
            deprecated: false,
            versions: [(
                version,
                PermissionVersionRecord {
                    version,
                    policy_template: self.policy_document(),
                    status: PERMISSION_STATUS_ATTACHABLE.to_string(),
                    creation_time: now,
                    last_updated_time: now,
                    // A permission Amazon Web Services manages carries no deleted
                    // version: no caller can delete one of its versions.
                    deletion: None,
                },
            )]
            .into_iter()
            .collect(),
            // A permission Amazon Web Services manages is one no caller can delete, so
            // no catalog entry is ever on the delete path.
            deletion: None,
        }
    }
}

/// The whole catalog as the records the store starts from, keyed by ARN exactly as the
/// customer managed permissions beside them are, so one lookup resolves either kind.
pub fn catalog_records(now: i64) -> BTreeMap<String, PermissionRecord> {
    AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| (entry.arn(), entry.record(now)))
        .collect()
}

/// The AWS managed permission RAM attaches to a share that shares a resource of
/// `resource_type` and names no permission for that type: the catalog entry that is
/// "the default managed permission for all resources of this resource type"
/// ([`AwsManagedPermission::is_resource_type_default`]), or `None` when the catalog
/// publishes no permission for the type at all.
///
/// The type is matched the way [`super::resource_type_matches`] matches one, so the
/// spelling read off a shared resource's ARN finds the entry the catalog publishes
/// under the display spelling of the same type. Where the library publishes more than one
/// permission of a type as that type's default, the first of them in catalog order is the
/// one a share that names no permission is governed by.
pub fn default_aws_managed_permission(
    resource_type: &str,
) -> Option<&'static AwsManagedPermission> {
    AWS_MANAGED_PERMISSIONS.iter().find(|entry| {
        entry.is_resource_type_default
            && super::resource_type_matches(entry.resource_type, resource_type)
    })
}


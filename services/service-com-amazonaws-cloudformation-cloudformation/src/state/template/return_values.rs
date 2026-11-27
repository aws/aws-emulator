// SPDX-License-Identifier: CC0-1.0

use super::*;

// What a template's outputs and its intrinsic functions are evaluated against, split out so
// this file holds the per-type return values alone. Re-exported, so the one path every caller
// of `Scope` and of an evaluation uses is unchanged.
mod outputs;
// The Amazon EventBridge entries of this catalogue — the resource types and the values their
// `Ref` and `Fn::GetAtt` resolve to — beside the types that reach them.
mod event_bridge;
pub use event_bridge::*;
mod type_names;
// The AWS AppSync types' return values, in a module of their own for the reason it records.
// Re-exported, so the one path every caller of a type name uses is unchanged.
mod appsync;
// The AWS Cloud Map types' return values, in a module of their own for the same reason, and
// re-exported the same way.
mod service_discovery;
mod edge_and_api_identities;
// The per-type `Ref`/`Fn::GetAtt` table itself, split out for the same reason and
// re-exported the same way.
mod ref_and_get_att;
pub(crate) use ref_and_get_att::*;
pub use appsync::*;
pub use service_discovery::*;
pub use edge_and_api_identities::*;
pub use type_names::*;
pub use outputs::*;

// The two Lake Formation permission types: their own NAMES and the projections of what each
// publishes live beside the arms that provision the grant
// ([`crate::state::provisioner::lake_formation_permissions`]), so this catalogue and the
// provisioner cannot disagree about what identifies one.
pub(super) use crate::state::provisioner::lake_formation_permissions;
pub(super) use crate::state::provisioner::lake_formation_permissions::{
    LAKEFORMATION_PERMISSIONS, LAKEFORMATION_PRINCIPAL_PERMISSIONS,
};

// The three Amazon Redshift Serverless types: their own NAMES and the attributes each
// publishes live beside the arms that provision them
// ([`crate::state::provisioner::redshift_serverless`]), so this catalogue and the provisioner
// cannot name the same attribute differently.
pub(super) use crate::state::provisioner::redshift_serverless;

// The Amazon AppFlow connector profile: its own NAME and the attributes it publishes live
// beside the arms that provision it
// ([`crate::state::provisioner::appflow_connector_profiles`]), for the same reason.
pub(super) use crate::state::provisioner::appflow_connector_profiles;

// The two IAM Access Analyzer types: their own NAMES, the ARN forms they publish and the
// attributes each of them publishes live beside the arms that provision them
// ([`crate::state::provisioner::access_analyzer`]), so this catalogue and the provisioner
// cannot name the same attribute differently.
pub(in crate::state) use crate::state::provisioner::access_analyzer;

// The three Amazon EFS types: their own NAMES and the names of the attributes each publishes
// live beside the arms that provision them
// ([`crate::state::provisioning::efs_resources`]), for the same reason.
pub(super) use crate::state::provisioning::efs_resources;
// The three Amazon Lex V2 types: their own NAMES, the ARN forms they publish and the
// attributes each of them publishes live beside the arms that provision them
// ([`crate::state::provisioner::lex_bots`]), so this catalogue and the provisioner cannot
// name the same attribute differently.
pub(in crate::state) use crate::state::provisioner::lex_bots;
pub(super) use crate::state::provisioning::efs_resources::{
    EFS_ACCESS_POINT, EFS_FILE_SYSTEM, EFS_MOUNT_TARGET,
};

/// Amazon EFS's ARN namespace, which its own model carries as `awsService.arnNamespace`
/// (`elasticfilesystem`) — the service segment of a file system's and an access point's ARN.
pub(super) const EFS_ARN_NAMESPACE: &str = "elasticfilesystem";

/// How many lowercase hexadecimal characters the identifier Amazon EFS mints carries after
/// its prefix: 17, which is the width that service mints its own ids at and which every
/// consumer of one admits (its own members take 8 to 40 digits, and AWS Lambda accepts an
/// access point only as `…:access-point/fsap-[a-f0-9]{17}`).
pub(super) const EFS_ID_HEX_DIGITS: usize = 17;

/// A DNS name with its terminal dot dropped, which is how CloudFormation reports the name of
/// a resource whose return value IS a DNS record name.
///
/// One dot: the root label is written at most once, so a name that carries none is already in
/// this form and is answered as it stands.
pub(super) fn without_terminal_dot(name: &str) -> &str {
    name.strip_suffix('.').unwrap_or(name)
}

/// The key ONE ELEMENT of a list-valued `Fn::GetAtt` attribute is carried under in a
/// resource's attribute map: the attribute's own name and the element's index, in the DOTTED
/// notation `Fn::GetAtt` addresses a path with ("For attributes that return a list, use
/// dotted notation").
///
/// The attribute map is string-valued, so a LIST attribute — a hosted zone's `NameServers`,
/// an endpoint's addresses — is carried element by element, and the attribute's own name
/// resolves to the whole list ([`list_attribute`]). Writing the elements under the path
/// `Fn::GetAtt` already reads means an index named in a template resolves to that element
/// with nothing further to do.
pub fn list_attribute_element(attribute: &str, index: usize) -> String {
    format!("{attribute}.{index}")
}

/// Every element of the list-valued attribute `attribute`, in index order, or `None` where
/// the resource publishes no such list.
///
/// The elements are read by INDEX from the first, so the list is the run the owning service
/// reported ([`list_attribute_element`]) in that service's own order, and a gap ends it
/// rather than silently reordering what follows.
pub fn list_attribute(
    attributes: &BTreeMap<String, String>,
    attribute: &str,
) -> Option<Vec<String>> {
    let mut elements = Vec::new();
    while let Some(element) = attributes.get(&list_attribute_element(attribute, elements.len())) {
        elements.push(element.clone());
    }
    (!elements.is_empty()).then_some(elements)
}

/// Whether a resource publishes `attribute` at all: as a value of its own, or as the
/// list-valued attribute its elements make up.
pub fn publishes_attribute(attributes: &BTreeMap<String, String>, attribute: &str) -> bool {
    attributes.contains_key(attribute) || list_attribute(attributes, attribute).is_some()
}

/// The ARN namespace Amazon Web Services Private CA mints its resources under (that
/// model's `awsService.arnNamespace`), which is `acm-pca` rather than the `ACMPCA` its
/// resource types are spelled with.
pub(super) const ACMPCA_ARN_NAMESPACE: &str = "acm-pca";

/// The stand-in a published return value only the OWNING SERVICE can produce — a
/// certification request, a certificate body, a certificate chain — is resolved to before
/// that service has been asked for it.
///
/// A document is not a value these naming rules can compute, and an output or another
/// resource's property may name it before the resource exists, so the attribute carries a
/// value that names the resource and the attribute it stands for.
/// [`super::provisioning::repoint_references`] re-points it at what the owning service
/// really answered, exactly as it re-points a minted identifier. It is deliberately NOT a
/// well-formed PEM body: a request that reaches a service still carrying it is refused by
/// that service rather than taken for a certificate.
pub(super) fn service_assigned_document(name: &str, attribute: &str) -> String {
    format!("the {attribute} of {name} is not available until it has been created")
}

/// The NAMES of the `Fn::GetAtt` attributes a resource of this type publishes — the other
/// half of the catalogue [`return_values`] above keys, read without a resource.
///
/// Source: the very same per-type "Return values" sections. Which attributes a type
/// publishes is a property of the TYPE, while the values [`return_values`] computes are
/// properties of one provisioned resource, so the extension registry can report what a
/// published resource type's schema DEFINES
/// ([`crate::state::resource_schema`](crate::state::resource_schema)) with no stack having
/// declared one. A type this emulator carries no return values for publishes no
/// attributes, and gets the empty list.
///
/// This is the same split [`declared_name_property`] is in: an arm added to
/// [`return_values`] has to be added here too, which
/// `published_attribute_names_are_the_catalogued_return_values` in this module's tests
/// enforces — it compares this list, type by type, against the attributes the arm actually
/// inserts. The order is the sorted order a resource's own attribute map holds them in, so
/// a document built from them is byte-for-byte the same on every read.
pub fn published_attribute_names(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        // The newer Lake Formation permission type publishes the two halves of its composite
        // primary identifier, and nothing else: the principal the grant was made to and the
        // Data Catalog resource it is on.
        LAKEFORMATION_PRINCIPAL_PERMISSIONS => &[
            lake_formation_permissions::PRINCIPAL_IDENTIFIER_PROPERTY,
            lake_formation_permissions::RESOURCE_IDENTIFIER_PROPERTY,
        ],
        ACMPCA_CERTIFICATE_AUTHORITY => &["Arn", "CertificateSigningRequest"],
        // The three Amazon Lex V2 types, each publishing the values that service's own create
        // answers for one: the bot's ARN and the `botId` inside it, the numeric version it
        // assigned when it cut a version, and an alias's ARN, its own id and the status it is
        // in ([`crate::state::provisioner::lex_bots`]).
        lex_bots::LEX_BOT => lex_bots::LEX_BOT_ATTRIBUTES,
        lex_bots::LEX_BOT_VERSION => lex_bots::LEX_BOT_VERSION_ATTRIBUTES,
        lex_bots::LEX_BOT_ALIAS => lex_bots::LEX_BOT_ALIAS_ATTRIBUTES,
        ACMPCA_CERTIFICATE => &["Arn", "Certificate"],
        ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION => &["CompleteCertificateChain"],
        BACKUP_VAULT => &["BackupVaultArn", "BackupVaultName"],
        "AWS::SNS::Topic" => &["TopicArn", "TopicName"],
        SQS_QUEUE => &["Arn", "QueueName", "QueueUrl"],
        // Each of the SageMaker AI types publishes the one attribute that carries the
        // resource's own name.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL => &["ModelName"],
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_ENDPOINT_CONFIG => {
            &["EndpointConfigName"]
        }
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_ENDPOINT => &["EndpointName"],
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE => {
            &["NotebookInstanceName"]
        }
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            &["NotebookInstanceLifecycleConfigName"]
        }
        // The three Studio types, each publishing the handles its own create answers with: the
        // domain's ARN, the `DomainId` SageMaker minted and its Studio URL, the user profile's
        // ARN, and the space's ARN beside the address that service serves it at.
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_DOMAIN => {
            &["DomainArn", "DomainId", "Url"]
        }
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_USER_PROFILE => {
            &["UserProfileArn"]
        }
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_SPACE => &["SpaceArn", "Url"],
        "AWS::S3::Bucket" => &[
            "Arn",
            "DomainName",
            "DualStackDomainName",
            "RegionalDomainName",
            "WebsiteURL",
        ],
        S3TABLES_TABLE_BUCKET => &[S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE],
        S3VECTORS_VECTOR_BUCKET => &["VectorBucketArn"],
        S3VECTORS_INDEX => &["IndexArn"],
        "AWS::DynamoDB::Table" => &["Arn", "StreamArn"],
        // The same type declared for a group of Regions publishes the table's own `TableId`
        // beside them.
        DYNAMODB_GLOBAL_TABLE => &["Arn", "StreamArn", "TableId"],
        "AWS::Lambda::Function" => &["Arn"],
        // "Fn::GetAtt … FunctionArn: the ARN of the version. Version: the version number."
        LAMBDA_VERSION => &["FunctionArn", "Version"],
        // "Fn::GetAtt … AliasArn: the Amazon Resource Name (ARN) of the alias."
        LAMBDA_ALIAS => &["AliasArn"],
        "AWS::Logs::LogGroup" => &["Arn"],
        TRANSFER_SERVER => &["Arn", "ServerId"],
        "AWS::Transfer::Workflow" => &["Arn", "WorkflowId"],
        IAM_ACCESS_KEY => &["SecretAccessKey"],
        "AWS::IAM::Role" => &["Arn"],
        ECR_PUBLIC_REPOSITORY => &["Arn"],
        FIREHOSE_DELIVERY_STREAM => &["Arn"],
        KINESIS_STREAM => &["Arn"],
        ECR_REPOSITORY => &["Arn", "RepositoryUri"],
        // The three Amazon EFS types, each publishing the values its own "Return values"
        // section names: the file system's ARN and id, the access point's id and ARN, and the
        // mount target's id and the address Amazon EFS placed it at.
        EFS_FILE_SYSTEM => &[
            efs_resources::ARN_ATTRIBUTE,
            efs_resources::FILE_SYSTEM_ID_ATTRIBUTE,
        ],
        EFS_ACCESS_POINT => &[
            efs_resources::ACCESS_POINT_ID_ATTRIBUTE,
            efs_resources::ARN_ATTRIBUTE,
        ],
        EFS_MOUNT_TARGET => &[
            efs_resources::ID_ATTRIBUTE,
            efs_resources::IP_ADDRESS_ATTRIBUTE,
        ],
        ECS_CLUSTER => &["Arn"],
        // A pipeline publishes the ARN CodePipeline keys it by and the VERSION that service
        // allocates for it; a webhook the URL that service generates for it.
        CODEPIPELINE_PIPELINE => &["Arn", "Version"],
        CODEPIPELINE_WEBHOOK => &["Url"],
        EKS_CLUSTER => &[
            "Arn",
            "CertificateAuthorityData",
            "ClusterSecurityGroupId",
            "EncryptionConfigKeyArn",
            "Endpoint",
            "OpenIdConnectIssuerUrl",
        ],
        EKS_POD_IDENTITY_ASSOCIATION => &["AssociationArn", "AssociationId"],
        // An add-on and a node group each publish the ARN Amazon EKS minted for the
        // resource (`Addon$addonArn`, `Nodegroup$nodegroupArn`), which is the one value only
        // that service can produce: it carries an identifier that service draws per
        // resource, so it is read off the resource it created rather than computed here.
        EKS_ADDON | EKS_NODEGROUP => &["Arn"],
        SSM_ASSOCIATION => &[SSM_ASSOCIATION_ID_ATTRIBUTE],
        // A patch baseline publishes the id Systems Manager minted for it, which is also what
        // its `Ref` returns.
        crate::state::provisioning::ssm_patch_baseline::SSM_PATCH_BASELINE => {
            &[SSM_PATCH_BASELINE_ID_ATTRIBUTE]
        }
        BATCH_COMPUTE_ENVIRONMENT => &["ComputeEnvironmentArn"],
        BATCH_JOB_QUEUE => &["JobQueueArn"],
        BATCH_JOB_DEFINITION => &["JobDefinitionArn"],
        BATCH_SERVICE_ENVIRONMENT => &["ServiceEnvironmentArn"],
        // The two SHARING-plane types: a scheduling policy publishes the ARN AWS Batch mints
        // for it, and a quota share the ARN that service composes under the queue it is on.
        BATCH_SCHEDULING_POLICY => &[crate::state::provisioner::SCHEDULING_POLICY_ARN_ATTRIBUTE],
        BATCH_QUOTA_SHARE => &[crate::state::provisioner::QUOTA_SHARE_ARN_ATTRIBUTE],
        ECS_TASK_DEFINITION => &["TaskDefinitionArn"],
        ELB_LOAD_BALANCER => &[
            "CanonicalHostedZoneID",
            "DNSName",
            "LoadBalancerFullName",
            "LoadBalancerName",
            "SecurityGroups",
        ],
        ELB_TARGET_GROUP => &[
            "LoadBalancerArns",
            "TargetGroupArn",
            "TargetGroupFullName",
            "TargetGroupName",
        ],
        // Source: `AWS::ElasticLoadBalancingV2::TrustStore`'s own "Return values" section —
        // the number of CA certificates the bundle it was created from holds, the status
        // Elastic Load Balancing reports the trust store in, and the ARN that service minted
        // for it (which is also what its `Ref` returns).
        ELB_TRUST_STORE => &["NumberOfCaCertificates", "Status", "TrustStoreArn"],
        "AWS::RDS::DBCluster" => &[
            "DBClusterArn",
            "DBClusterResourceId",
            "Endpoint.Address",
            "Endpoint.Port",
            MASTER_USER_SECRET_ARN_ATTRIBUTE,
            "ReadEndpoint.Address",
        ],
        "AWS::RDS::DBInstance" => &[
            "DBInstanceArn",
            "DbiResourceId",
            "Endpoint.Address",
            "Endpoint.Port",
            MASTER_USER_SECRET_ARN_ATTRIBUTE,
        ],
        SECRETSMANAGER_SECRET => &["Id"],
        EC2_VPC => &["VpcId"],
        EC2_SUBNET => &["SubnetId"],
        EC2_SECURITY_GROUP => &["GroupId"],
        EC2_INTERNET_GATEWAY => &["InternetGatewayId"],
        EC2_ROUTE_TABLE => &["RouteTableId"],
        // Source: `AWS::EC2::Route`'s own "Return values" section — "CidrBlock: the CIDR block
        // used for the destination match", which is the destination the declaration states
        // ([`ec2_route_destination`]). Its `Ref` is the route's own composite id rather than
        // one of these attributes.
        EC2_ROUTE => &["CidrBlock"],
        EC2_EIP => &["AllocationId", "PublicIp"],
        EC2_NAT_GATEWAY => &["NatGatewayId"],
        EC2_DHCP_OPTIONS => &["DhcpOptionsId"],
        EC2_VPN_GATEWAY => &["VPNGatewayId"],
        EC2_NETWORK_ACL | EC2_NETWORK_INTERFACE | EC2_VPC_ENDPOINT => &["Id"],
        // Source: each rule type's own "Return values" section — "Id: the ID of the security
        // group rule", which is the `sgr-…` identifier Amazon EC2 mints for the rule the
        // authorize call added and what that type's `Ref` returns as well.
        EC2_SECURITY_GROUP_INGRESS | EC2_SECURITY_GROUP_EGRESS => &["Id"],
        EC2_VOLUME => &["VolumeId"],
        // Source: `AWS::EC2::LaunchTemplate`'s own "Return values" section — the id EC2
        // assigned the template, and the default and latest version numbers of it.
        EC2_LAUNCH_TEMPLATE => &[
            "DefaultVersionNumber",
            "LatestVersionNumber",
            "LaunchTemplateId",
        ],
        // Source: `AWS::EC2::Instance`'s own "Return values" section — the zone the instance
        // was placed in, its private DNS name and IPv4 address, its public ones, and the VPC it
        // runs in. "Ref returns the ID of the instance", which is the resource's physical id
        // rather than one of these attributes.
        EC2_INSTANCE => &[
            "AvailabilityZone",
            "PrivateDnsName",
            "PrivateIp",
            "PublicDnsName",
            "PublicIp",
            "VpcId",
        ],
        // Source: `AWS::EC2::SpotFleet`'s own "Return values" section — "Id: the ID of the Spot
        // Fleet", which is also what its `Ref` returns.
        EC2_SPOT_FLEET => &["Id"],
        ECS_SERVICE => &["Name"],
        // "Fn::GetAtt … Id: the ID of the task set" — the `ecs-svc/` identifier Amazon ECS
        // mints, which is the only handle every call on a task set takes. (`Ref` returns the
        // triple that service addresses one by, not this attribute.)
        crate::state::provisioner::ecs_task_sets::ECS_TASK_SET => {
            &[crate::state::provisioner::ecs_task_sets::ID_ATTRIBUTE]
        }
        ELB_LISTENER => &["ListenerArn"],
        ELB_LISTENER_RULE => &["IsDefault", "RuleArn"],
        BEDROCK_GUARDRAIL => &["GuardrailArn", "GuardrailId", "Version"],
        // A flow's ARN, the id Amazon Bedrock Agent minted and the DRAFT version it opens
        // with are the return values these naming rules can compute; its `Status` and the two
        // instants are that service's clock and reach the resource off its own report of the
        // flow, exactly as a knowledge base's do.
        BEDROCK_FLOW => &["Arn", "Id", "Version"],
        BEDROCK_KNOWLEDGE_BASE => &["KnowledgeBaseArn", "KnowledgeBaseId"],
        // The id Amazon Bedrock Agent mints for a data source is the one return value these
        // naming rules can compute a stand-in for; its `DataSourceStatus` and the two instants
        // are that service's clock and reach the resource off its own report of the data
        // source, exactly as a knowledge base's status and instants do.
        BEDROCK_DATA_SOURCE => &[BEDROCK_DATA_SOURCE_ID_ATTRIBUTE],
        // A Billing and Cost Management Dashboards dashboard, and a scheduled report on one,
        // each publish the ARN that service minted — the one return value these naming rules
        // compute a stand-in for before the resource exists — beside the instants of its
        // clock, which reach the resource off its own report rather than from here.
        BCM_DASHBOARD => BCM_DASHBOARD_ATTRIBUTES,
        BCM_SCHEDULED_REPORT => BCM_SCHEDULED_REPORT_ATTRIBUTES,
        // A session's ARN and the uuid the Bedrock Agent runtime minted for it are the return
        // values these naming rules can compute; its `SessionStatus` and the two instants it
        // keeps are that service's clock and reach the resource off its own report of the
        // session, exactly as a flow's do.
        BEDROCK_SESSION => BEDROCK_SESSION_ATTRIBUTES,
        // An application inference profile publishes the ARN its `Ref` also returns and the id
        // Amazon Bedrock minted inside it; its `Status`, its `Type` and the two instants that
        // service keeps reach the resource off its own read of the profile, exactly as a
        // session's do.
        BEDROCK_APPLICATION_INFERENCE_PROFILE => BEDROCK_INFERENCE_PROFILE_ATTRIBUTES,
        // A blueprint, a data automation project and a data automation library each publish the
        // ARN Amazon Bedrock Data Automation minted for the resource — the one return value
        // these naming rules compute a stand-in for before the resource exists. Each one's
        // status and the instants of that service's clock reach the resource off its own report
        // of it, exactly as a flow's do.
        BEDROCK_BLUEPRINT => &[BLUEPRINT_ARN_ATTRIBUTE],
        BEDROCK_DATA_AUTOMATION_PROJECT => &[PROJECT_ARN_ATTRIBUTE],
        BEDROCK_DATA_AUTOMATION_LIBRARY => &[LIBRARY_ARN_ATTRIBUTE],
        // A VPC Lattice service network publishes the ARN its `Ref` also returns, the id
        // that service minted and the two instants of its clock; a subscription publishes the
        // same ARN and id beside the monitored resource that service resolved. Only each ARN
        // and the id inside it are values these naming rules compute a stand-in for.
        VPCLATTICE_SERVICE_NETWORK => VPCLATTICE_SERVICE_NETWORK_ATTRIBUTES,
        VPCLATTICE_ACCESS_LOG_SUBSCRIPTION => VPCLATTICE_ACCESS_LOG_SUBSCRIPTION_ATTRIBUTES,
        // A spending limit's ARN is the one return value these naming rules can compute; its
        // `CreatedAt` is Amazon Braket's clock, which reaches the resource off that
        // service's own report of the limit rather than from here, exactly as a knowledge
        // base's instants do.
        // An investigation group's ARN is the one return value these naming rules can compute;
        // the caller Amazon CloudWatch investigations records for it and the two instants of
        // its clock reach the resource off that service's own report of the group rather than
        // from here, exactly as a spending limit's `CreatedAt` does.
        AIOPS_INVESTIGATION_GROUP => &["Arn"],
        BRAKET_SPENDING_LIMIT => &["SpendingLimitArn"],
        // An Amazon GameLift Servers build and a Realtime script each publish the id that
        // service minted and the ARN built around it, which are the two return values these
        // naming rules compute a stand-in for before the resource exists.
        GAMELIFT_BUILD => GAMELIFT_BUILD_ATTRIBUTES,
        GAMELIFT_SCRIPT => GAMELIFT_SCRIPT_ATTRIBUTES,
        // Each AWS End User Messaging SMS and Voice type publishes the ARN that service
        // minted for the resource, and the four whose identity that service assigns publish
        // the identifier inside it beside it. (A RESOURCE POLICY publishes none: its one
        // return value is the ARN its `Ref` returns.)
        SMSVOICE_CONFIGURATION_SET | SMSVOICE_OPT_OUT_LIST => SMSVOICE_NAMED_ATTRIBUTES,
        SMSVOICE_PROTECT_CONFIGURATION => SMSVOICE_PROTECT_CONFIGURATION_ATTRIBUTES,
        SMSVOICE_RCS_AGENT => SMSVOICE_RCS_AGENT_ATTRIBUTES,
        SMSVOICE_REGISTRATION => SMSVOICE_REGISTRATION_ATTRIBUTES,
        SMSVOICE_REGISTRATION_ATTACHMENT => SMSVOICE_REGISTRATION_ATTACHMENT_ATTRIBUTES,
        // Each Amazon VPC Lattice RESOURCE-ACCESS type publishes the ARN that service minted
        // and the id inside it, which are the two return values these naming rules compute a
        // stand-in for before the resource exists.
        VPCLATTICE_RESOURCE_GATEWAY
        | VPCLATTICE_RESOURCE_CONFIGURATION
        | VPCLATTICE_SERVICE_NETWORK_RESOURCE_ASSOCIATION
        | VPCLATTICE_SERVICE_NETWORK_VPC_ASSOCIATION => VPCLATTICE_RESOURCE_ACCESS_ATTRIBUTES,
        // An AWS HealthLake FHIR data store publishes the four read-only values its own
        // "Return values" section names, and a data transformation profile the five its own
        // does; only each resource's ARN and the id inside it, and the version and target
        // format a created profile's own contract states, are values these naming rules
        // compute, the store's endpoint and status and the profile's instant reaching the
        // resource off that service's own report of it.
        HEALTHLAKE_FHIR_DATASTORE => HEALTHLAKE_FHIR_DATASTORE_ATTRIBUTES,
        HEALTHLAKE_DATA_TRANSFORMATION_PROFILE => HEALTHLAKE_PROFILE_ATTRIBUTES,
        // A Direct Connect gateway and a resiliency group each publish the identifier AWS
        // Direct Connect minted for them, which is the one return value these naming rules
        // compute a stand-in for before the resource exists.
        DIRECT_CONNECT_GATEWAY => DIRECT_CONNECT_GATEWAY_ATTRIBUTES,
        DIRECT_CONNECT_RESILIENCY_GROUP => DIRECT_CONNECT_RESILIENCY_GROUP_ATTRIBUTES,
        // Either Directory Service directory publishes the `Alias` that service answers its
        // access URL under — the directory id itself until an alias is assigned — and a
        // Simple AD the `DirectoryId` it minted beside it. Their `DnsIpAddresses` is a LIST,
        // so no attribute carries it ([`DIRECTORY_SERVICE_MICROSOFT_AD_ATTRIBUTES`]).
        DIRECTORY_SERVICE_MICROSOFT_AD => DIRECTORY_SERVICE_MICROSOFT_AD_ATTRIBUTES,
        DIRECTORY_SERVICE_SIMPLE_AD => DIRECTORY_SERVICE_SIMPLE_AD_ATTRIBUTES,
        // The private Amazon ECR registry's own SIGNING CONFIGURATION publishes the registry it
        // belongs to, which these naming rules compute from the account this emulator mints.
        ECR_SIGNING_CONFIGURATION => ECR_SIGNING_CONFIGURATION_ATTRIBUTES,
        // An Application Signals service level objective publishes the ARN these naming rules
        // compute from its name beside the four read-only values that service decides; a
        // GROUPING CONFIGURATION publishes the one instant its put recorded; and an
        // INSTRUMENTATION CONFIG publishes the ARN, the instant and the location hash that
        // service answered for the point it recorded.
        APPLICATION_SIGNALS_SERVICE_LEVEL_OBJECTIVE => APPLICATION_SIGNALS_SLO_ATTRIBUTES,
        APPLICATION_SIGNALS_GROUPING_CONFIGURATION => {
            APPLICATION_SIGNALS_GROUPING_CONFIGURATION_ATTRIBUTES
        }
        APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG => {
            APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_ATTRIBUTES
        }
        // A training dataset publishes the ARN Clean Rooms ML minted for it, which is the one
        // return value these naming rules compute a stand-in for before it exists; its
        // `Status` is that service's own to report and reaches the resource off its report of
        // the created dataset rather than from here, exactly as a spending limit's `CreatedAt`
        // does.
        CLEANROOMSML_TRAINING_DATASET => &[CLEANROOMSML_TRAINING_DATASET_ARN_ATTRIBUTE],
        // A Quick Setup configuration manager publishes the ARN that service minted for it,
        // which is the one return value these naming rules compute a stand-in for before it
        // exists; its `CreatedAt`, `LastModifiedAt` and `StatusSummaries` are that service's
        // own to report and reach the resource off its report of the created manager rather
        // than from here, exactly as a spending limit's `CreatedAt` does.
        SSM_QUICKSETUP_CONFIGURATION_MANAGER => {
            &[SSM_QUICKSETUP_CONFIGURATION_MANAGER_ARN_ATTRIBUTE]
        }
        // A CloudHSM cluster publishes the ARN its `Ref` also returns and the five read-only
        // values CloudHSM reports for it; only the ARN and the id inside it are values these
        // naming rules compute a stand-in for, the rest reaching the resource off that
        // service's own description of the created cluster.
        CLOUDHSM_CLUSTER => CLOUDHSM_CLUSTER_ATTRIBUTES,
        // A hypervisor publishes the ARN Backup Gateway minted for it, which is the one return
        // value these naming rules compute a stand-in for before it exists.
        BACKUP_GATEWAY_HYPERVISOR => BACKUP_GATEWAY_HYPERVISOR_ATTRIBUTES,
        // An analyzer publishes the ARN IAM Access Analyzer records for it, which these naming
        // rules compute from the name the declaration states; an ARCHIVE RULE of one publishes
        // that same ARN with the rule's own name beneath it, beside the two instants that
        // service keeps for the rule — its own clock, which reach the resource off its report of
        // the rule rather than from here, exactly as a spending limit's `CreatedAt` does.
        access_analyzer::ACCESSANALYZER_ANALYZER => access_analyzer::ANALYZER_ATTRIBUTES,
        access_analyzer::ACCESSANALYZER_ARCHIVE_RULE => access_analyzer::ARCHIVE_RULE_ATTRIBUTES,
        // An IoT Core Device Advisor suite definition publishes the id that service mints for
        // it, the ARN built around that id, and the version label it keeps the suite's live
        // configuration under — the three return values these naming rules compute a stand-in
        // for before the suite exists.
        IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION => {
            IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ATTRIBUTES
        }
        // A retention rule publishes the ARN and the identifier Recycle Bin minted for it and
        // the lock state it keeps, all three of which these naming rules compute a stand-in
        // for before the rule exists.
        RBIN_RULE => RBIN_RULE_ATTRIBUTES,
        // A lifecycle policy publishes the ARN Amazon Data Lifecycle Manager built around the
        // id it minted, which these naming rules compute a stand-in for before the policy
        // exists.
        DLM_LIFECYCLE_POLICY => DLM_LIFECYCLE_POLICY_ATTRIBUTES,
        // Each AWS Network Security Manager entity publishes the ARN that service minted for
        // it and the id inside that ARN, both of which these naming rules compute a stand-in
        // for before the entity exists.
        NETWORK_SECURITY_MANAGER_RULE => NETWORK_SECURITY_MANAGER_RULE_ATTRIBUTES,
        NETWORK_SECURITY_MANAGER_TEMPLATE => NETWORK_SECURITY_MANAGER_TEMPLATE_ATTRIBUTES,
        NETWORK_SECURITY_MANAGER_POLICY => NETWORK_SECURITY_MANAGER_POLICY_ATTRIBUTES,
        NETWORK_SECURITY_MANAGER_SCOPE => NETWORK_SECURITY_MANAGER_SCOPE_ATTRIBUTES,
        // A Neptune Analytics graph publishes the endpoint, the ARN and the id that service
        // produces for it, all three of which these naming rules compute a stand-in for
        // before the graph exists; a SNAPSHOT of one publishes the ARN and the id that
        // service produces for it.
        NEPTUNE_GRAPH_GRAPH => NEPTUNE_GRAPH_GRAPH_ATTRIBUTES,
        NEPTUNE_GRAPH_GRAPH_SNAPSHOT => NEPTUNE_GRAPH_SNAPSHOT_ATTRIBUTES,
        // Each Amazon Redshift Serverless resource publishes the values that service reports
        // for it, under the prefix its own "Return values" section names — the catalogue living
        // beside the arms that read them off that service's answers
        // ([`crate::state::provisioner::redshift_serverless`]), so the two cannot name the same
        // attribute differently.
        redshift_serverless::REDSHIFT_SERVERLESS_NAMESPACE => {
            redshift_serverless::REDSHIFT_SERVERLESS_NAMESPACE_ATTRIBUTE_NAMES
        }
        redshift_serverless::REDSHIFT_SERVERLESS_WORKGROUP => {
            redshift_serverless::REDSHIFT_SERVERLESS_WORKGROUP_ATTRIBUTE_NAMES
        }
        redshift_serverless::REDSHIFT_SERVERLESS_SNAPSHOT => {
            redshift_serverless::REDSHIFT_SERVERLESS_SNAPSHOT_ATTRIBUTE_NAMES
        }
        // An Amazon AppFlow connector profile publishes the ARN that service minted for it and
        // the handle it holds the profile's credentials under, the catalogue living beside the
        // arms that read them off that service's own answer.
        appflow_connector_profiles::APPFLOW_CONNECTOR_PROFILE => {
            appflow_connector_profiles::CONNECTOR_PROFILE_ATTRIBUTE_NAMES
        }
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_THEME => AMPLIFYUIBUILDER_TIMED_ATTRIBUTES,
        AMPLIFYUIBUILDER_FORM => AMPLIFYUIBUILDER_ATTRIBUTES,
        // A hosted configuration version publishes the version number AppConfig minted for the
        // content, and a deployment the number it allocated for the rollout beside the
        // `DeploymentState` it reports that rollout in — each a value only that service produces,
        // which these naming rules compute a stand-in for before the resource exists.
        APPCONFIG_HOSTED_CONFIGURATION_VERSION => &[APPCONFIG_VERSION_NUMBER_ATTRIBUTE],
        APPCONFIG_DEPLOYMENT => APPCONFIG_DEPLOYMENT_ATTRIBUTES,
        AMPLIFY_APP => AMPLIFY_APP_ATTRIBUTES,
        AMPLIFY_BRANCH => AMPLIFY_BRANCH_ATTRIBUTES,
        MWAA_ENVIRONMENT => MWAA_ENVIRONMENT_ATTRIBUTES,
        // A Kinesis Video Streams stream and signaling channel each publish the ARN that
        // service minted for the resource, which is the one return value these naming rules
        // compute a stand-in for before the resource exists.
        KINESISVIDEO_STREAM | KINESISVIDEO_SIGNALING_CHANNEL => KINESISVIDEO_ATTRIBUTES,
        CUSTOMER_PROFILES_DOMAIN => CUSTOMER_PROFILES_DOMAIN_ATTRIBUTES,
        "AWS::BedrockAgentCore::Memory" => &["MemoryArn", "MemoryId"],
        "AWS::BedrockAgentCore::Runtime" => {
            &["AgentRuntimeArn", "AgentRuntimeId", "AgentRuntimeVersion"]
        }
        "AWS::BedrockAgentCore::CodeInterpreter" => &["CodeInterpreterArn", "CodeInterpreterId"],
        "AWS::BedrockAgentCore::Browser" => &["BrowserArn", "BrowserId"],
        "AWS::BedrockAgentCore::Gateway" => &["GatewayArn", "GatewayId"],
        // A gateway TARGET publishes the id the control plane minted for it beside the ARN of
        // the gateway it is served by, both read off that create's own answer.
        crate::state::provisioner::BEDROCK_AGENTCORE_GATEWAY_TARGET => {
            crate::state::provisioner::bedrock_agentcore_gateways::GATEWAY_TARGET_ATTRIBUTE_NAMES
        }
        "AWS::BedrockAgentCore::WorkloadIdentity" => &["Name", "WorkloadIdentityArn"],
        "AWS::RUM::AppMonitor" => &["Id"],
        "AWS::CloudWatch::Alarm" => &["Arn"],
        crate::state::provisioner::cloudwatch_composite_alarms::CLOUDWATCH_COMPOSITE_ALARM => {
            &["Arn"]
        }
        CODEBUILD_REPORT_GROUP => &["Arn"],
        CODEBUILD_PROJECT => &["Arn"],
        CLOUDWATCH_METRIC_STREAM => &["Arn"],
        CODEARTIFACT_DOMAIN => &["Arn", "EncryptionKey", "Name", "Owner"],
        CODEARTIFACT_REPOSITORY => &["Arn", "DomainOwner", "Name"],
        COGNITO_USER_POOL => &["Arn", "ProviderName", "ProviderURL"],
        COGNITO_USER_POOL_CLIENT => &["ClientSecret"],
        FIS_EXPERIMENT_TEMPLATE => &["Id"],
        CLOUDFRONT_DISTRIBUTION => &["DomainName", "Id"],
        CLOUDFRONT_ORIGIN_ACCESS_CONTROL => &["Id"],
        CLOUDFRONT_ORIGIN_ACCESS_IDENTITY => &["Id", "S3CanonicalUserId"],
        CLOUDFRONT_CACHE_POLICY => &["Id", "LastModifiedTime"],
        CLOUDFRONT_ORIGIN_REQUEST_POLICY => &["Id", "LastModifiedTime"],
        CLOUDFRONT_RESPONSE_HEADERS_POLICY => &["Id"],
        // A function publishes the ARN a cache behaviour names it by — under the two spellings
        // the type carries it — and the stage it is in.
        CLOUDFRONT_FUNCTION => &["FunctionARN", "FunctionMetadata.FunctionARN", "Stage"],
        // A public key and a key group each publish the id CloudFront minted for them beside
        // the instant that service reports for the resource.
        CLOUDFRONT_PUBLIC_KEY => &["CreatedTime", "Id"],
        CLOUDFRONT_KEY_GROUP => &["Id", "LastModifiedTime"],
        // A key value store publishes the ARN that identifies it, the id inside that ARN and
        // the status CloudFront reports it in.
        CLOUDFRONT_KEY_VALUE_STORE => &["Arn", "Id", "Status"],
        // A connection function publishes the ARN CloudFront built for it, the id its own
        // publish, update and delete take, and that service's report of the function.
        CLOUDFRONT_CONNECTION_FUNCTION => &[
            "ConnectionFunctionArn",
            "CreatedTime",
            "Id",
            "LastModifiedTime",
            "Stage",
            "Status",
        ],
        // A connection group publishes its ARN, its id, the routing endpoint CloudFront
        // assigned it and that service's report of the group.
        CLOUDFRONT_CONNECTION_GROUP => &[
            "Arn",
            "CreatedTime",
            "Id",
            "IsDefault",
            "LastModifiedTime",
            "RoutingEndpoint",
            "Status",
        ],
        // A VPC origin publishes the ARN and the id CloudFront minted for it, the status that
        // service reports it in and the two instants of its own clock.
        CLOUDFRONT_VPC_ORIGIN => &["Arn", "CreatedTime", "Id", "LastModifiedTime", "Status"],
        // A real-time log configuration publishes the ARN CloudFront minted for it, which is
        // also what its `Ref` returns: the type declares the configuration's name, its
        // sampling rate, its fields and its endpoints, and the ARN is the one read-only value
        // beside them.
        CLOUDFRONT_REALTIME_LOG_CONFIG => &["Arn"],
        APIGATEWAY_REST_API => &["RestApiId", "RootResourceId"],
        APIGATEWAY_RESOURCE => &["ResourceId"],
        // The AppSync types' own half of the catalogue, beside their return values.
        APPSYNC_GRAPHQL_API
        | APPSYNC_GRAPHQL_SCHEMA
        | APPSYNC_DATA_SOURCE
        | APPSYNC_RESOLVER
        | APPSYNC_FUNCTION_CONFIGURATION
        | APPSYNC_API_KEY
        | APPSYNC_API
        | APPSYNC_CHANNEL_NAMESPACE
        | APPSYNC_SOURCE_API_ASSOCIATION => appsync::published_attributes(resource_type),
        // The AWS Cloud Map types' own half of the catalogue, beside their return values.
        SERVICEDISCOVERY_HTTP_NAMESPACE
        | SERVICEDISCOVERY_PUBLIC_DNS_NAMESPACE
        | SERVICEDISCOVERY_PRIVATE_DNS_NAMESPACE
        | SERVICEDISCOVERY_SERVICE
        | SERVICEDISCOVERY_INSTANCE => service_discovery::published_attributes(resource_type),
        APIGATEWAY_DEPLOYMENT => &["DeploymentId"],
        APIGATEWAY_API_KEY => &["APIKeyId"],
        APIGATEWAY_AUTHORIZER => &["AuthorizerId"],
        APIGATEWAY_REQUEST_VALIDATOR => &["RequestValidatorId"],
        APIGATEWAY_CLIENT_CERTIFICATE => &["ClientCertificateId"],
        APIGATEWAY_ACCOUNT | APIGATEWAY_USAGE_PLAN | APIGATEWAY_USAGE_PLAN_KEY => &["Id"],
        APIGATEWAYV2_API => &["ApiEndpoint", "ApiId", "ExecuteApiArn"],
        APIGATEWAYV2_API_MAPPING => &["ApiMappingId"],
        APIGATEWAYV2_DOMAIN_NAME => &["DomainNameArn", "RegionalDomainName"],
        // A routing rule of such a domain name publishes the ARN API Gateway v2 built for it
        // and the id inside that ARN, both of which these naming rules compute a stand-in for
        // before the rule exists.
        APIGATEWAYV2_ROUTING_RULE => &[
            crate::state::provisioner::ROUTING_RULE_ARN_ATTRIBUTE,
            crate::state::provisioner::ROUTING_RULE_ID_ATTRIBUTE,
        ],
        GUARDDUTY_DETECTOR | GUARDDUTY_IP_SET => &["Id"],
        ROUTE53_HEALTH_CHECK => &["HealthCheckId"],
        ROUTE53_HOSTED_ZONE => &["Id"],
        ROUTE53_CIDR_COLLECTION => &["Arn", "Id"],
        ROUTE53_DELEGATION_SET => &["Id"],
        ROUTE53_TRAFFIC_POLICY => &["Id", "Type", "Version"],
        KMS_KEY => &["Arn", "KeyId"],
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => &["Arn"],
        // A configuration publishes its ARN and the LATEST REVISION's number and creation
        // instant, each read under the nested name the type publishes it at. The revision's
        // DESCRIPTION is not among them, for the reason
        // [`MSK_CONFIGURATION_REVISION_DESCRIPTION`] records.
        MSK_CONFIGURATION => &[
            "Arn",
            MSK_CONFIGURATION_REVISION_CREATION_TIME,
            MSK_CONFIGURATION_REVISION_NUMBER,
        ],
        MQ_BROKER => &["Arn"],
        // An Amazon MQ configuration publishes the ARN and the id that service minted for it
        // beside the number of its LATEST revision.
        MQ_CONFIGURATION => &["Arn", "Id", MQ_CONFIGURATION_REVISION_ATTRIBUTE],
        EVENTS_RULE => &["Arn"],
        EVENTS_EVENT_BUS => EVENTS_EVENT_BUS_ATTRIBUTES,
        SSM_PARAMETER => &["Type", "Value"],
        STEP_FUNCTIONS_ACTIVITY => &["Arn", "Name"],
        STEP_FUNCTIONS_STATE_MACHINE => &["Arn", "Name"],
        SCHEDULER_SCHEDULE_GROUP | SCHEDULER_SCHEDULE => &["Arn"],
        RESOURCE_GROUPS_GROUP => &["Arn"],
        // An application publishes its own ARN, under the member name that type's return values
        // section spells it with (`ApplicationARN`).
        APPLICATION_INSIGHTS_APPLICATION => &[APPLICATION_INSIGHTS_APPLICATION_ARN_ATTRIBUTE],
        // Each Amazon MemoryDB group publishes its own ARN, under the upper-case member
        // name that service's model spells it with (`ParameterGroup$ARN`,
        // `SubnetGroup$ARN`).
        MEMORYDB_PARAMETER_GROUP | MEMORYDB_SUBNET_GROUP => {
            &[crate::state::template::UPPER_CASE_ARN_ATTRIBUTE]
        }
        // Each AWS Elemental MediaConvert type publishes its own ARN, under the member name
        // that service's model spells it with (`Queue$Arn`, `JobTemplate$Arn`, `Preset$Arn`:
        // "an identifier for this resource that is unique within all of AWS").
        MEDIACONVERT_QUEUE | MEDIACONVERT_JOB_TEMPLATE | MEDIACONVERT_PRESET => &["Arn"],
        // An Amazon Athena capacity reservation publishes its own ARN, which is also the
        // type's primary identifier and what its `Ref` resolves to.
        crate::state::provisioning::athena_resources::ATHENA_CAPACITY_RESERVATION => &["Arn"],
        _ => &[],
    }
}

/// Which of the attributes a type publishes are LIST-valued: a load balancer's
/// `SecurityGroups` ("the IDs of the security groups for the load balancer") and a target
/// group's `LoadBalancerArns` ("the Amazon Resource Name (ARN) of the load balancer that
/// routes traffic to this target group"), each a list in the owning service's own model
/// (`smithy_query(query="shape",shape="LoadBalancer")` /
/// `shape="TargetGroup"`).
///
/// Such an attribute carries its entries as ONE value — the resource's record holds a string
/// per attribute — so a template that wants a single entry of it reads the entries with
/// `Fn::Select`, which is what that function's own documentation says of these attributes:
/// "for attributes that return a list, use `Fn::Select`". This is the catalogue that reading
/// is keyed on ([`super::attribute_items`]); a type with no entry publishes no list-valued
/// return value, so every attribute of it is the one value it reads as.
pub fn published_list_attribute_names(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        ELB_LOAD_BALANCER => &["SecurityGroups"],
        ELB_TARGET_GROUP => &["LoadBalancerArns"],
        _ => &[],
    }
}

/// Whether the PHYSICAL ID of a resource of this type is assigned by the service that owns
/// it rather than derived from what the template declares.
///
/// For such a type the identifier in force is the one that service minted: it cannot be
/// recomputed from the template, so a later deployment of the same declaration carries the
/// recorded one forward instead of replacing the resource
/// ([`super::StoredStack::deploy_template`]). Every other type's physical id is a function
/// of its declared or generated name, which any deployment recomputes identically.
///
/// Nine types this emulator provisions are in that position: an experiment template's id
/// is minted by FIS (`Ref` returns it, and the type has no name property at all), an AWS
/// WAF Classic regional IPSet's id is minted by `CreateIPSet` ("Ref returns the IPSet ID"),
/// an Amazon Managed Grafana workspace's id is minted by `CreateWorkspace` (`Ref` returns
/// the workspace id, a `^g-[0-9a-f]{10}$` value), where that type's `Name` is only what the
/// create asks for, an Amazon EMR cluster's is the `j-` job-flow id `RunJobFlow` mints
/// ("Ref returns the cluster ID"), where the type's `Name` is only the name the flow runs
/// under, a GuardDuty detector's is minted by `CreateDetector`, which takes no identifier
/// at all, an OpenSearch Serverless collection's `Ref` is the `CollectionId` its create
/// mints — a template declares the collection's NAME, never that id — and an ACM ACME
/// endpoint's is the `AcmeEndpointArn` `CreateAcmeEndpoint` mints, that type declaring no
/// name or identifier property either, and each kind of Amazon MSK cluster's is the
/// `ClusterArn` its create answers with ("Ref returns the ARN of the cluster"), whose
/// resource path carries an identifier Amazon MSK mints beside the declared cluster name.
///
/// The three Route 53 Resolver types are in the same position: that service mints a
/// `rslvr-`-prefixed id for an endpoint, a rule and a rule association, none of its creates
/// takes one, and each type's `Ref` returns that id — the `Name` such a type declares is
/// only "a friendly name that lets you easily find [it] in the Resolver dashboard". Its
/// three DNS Firewall types are the same case ([`ROUTE53RESOLVER_FIREWALL_DOMAIN_LIST`] and
/// its siblings): a domain list, a rule group and a rule group association each carry the
/// `rslvr-`-prefixed id that service minted, which is what their `Ref` returns and what
/// every call on them takes.
///
/// An API Gateway v2 api and an api mapping are two more: `CreateApi` and
/// `CreateApiMapping` take no identifier and answer the `ApiId` / `ApiMappingId` that
/// service assigned, and each type's `Ref` returns that id — the `Name` an api declares is
/// only what the api is called, and a mapping declares no name at all. The integration, the
/// route, the deployment and the authorizer of an api are in exactly that position
/// ([`APIGATEWAYV2_INTEGRATION`] and its siblings): each create answers the id that service
/// minted, which is what the type's `Ref` returns and what the api's other resources name it
/// by — a route's `Target` names its integration as `integrations/<IntegrationId>` and its
/// `AuthorizerId` names an authorizer by the minted id, and no name could stand in for
/// either. (A custom `AWS::ApiGatewayV2::DomainName` is NOT one of them: its identifier is
/// the domain name the template states; nor is an `AWS::ApiGatewayV2::Stage`, whose
/// identifier is the stage name it states.) The INTEGRATION RESPONSE, the ROUTE RESPONSE and
/// the MODEL of an api are three more ([`APIGATEWAYV2_INTEGRATION_RESPONSE`] and its
/// siblings): `CreateIntegrationResponse`, `CreateRouteResponse` and `CreateModel` each take
/// no identifier and answer the one that service assigned, which is what the type's `Ref`
/// returns and the handle every later call on the resource takes.
///
/// An AWS Transfer Family server is one more: `CreateServer` takes no identifier and
/// answers the `ServerId` it assigned, and that type's `Ref` returns the server's ARN,
/// which carries that id — so neither the id nor the ARN can be recomputed from the
/// template.
///
/// The four WAFv2 types are in that position too ([`WAFV2_WEB_ACL`] and its siblings): each
/// create answers an `Id` that service minted, and the entity's ARN carries it, so a
/// deployment can recompute neither.
///
/// A KMS key is one more: `CreateKey` takes no identifier and answers the `KeyId` KMS
/// assigned, that type's `Ref` returns the key id, and the key ARN that identifier is
/// embedded in is what a queue's `KmsMasterKeyId` (or any other reference to the key)
/// resolves through.
///
/// An Amazon Route 53 hosted zone is one more: `CreateHostedZone` takes no identifier and
/// answers the `Z…` id Route 53 assigned, and "Ref returns the hosted zone ID" — the zone's
/// `Name` is only the DNS name it answers for, and no operation of that service takes a name
/// where it takes a zone id. (An `AWS::Route53::RecordSet` is NOT one of them: "Ref returns
/// the record name", which is the `Name` its own declaration states.)
pub fn identity_is_service_assigned(resource_type: &str) -> bool {
    resource_type == APIGATEWAYV2_API
        // A REST API, a resource of one and a deployment of it are three more, for the same
        // reason the API Gateway v2 types below are: `CreateRestApi`, `CreateResource` and
        // `CreateDeployment` each take no identifier for the resource they make and answer
        // the `id` that service assigned, and each type's `Ref` returns that id — the `Name`
        // an api declares is only what the api is called, a resource declares only its path
        // segment and a deployment declares no name at all, and every call on one takes the
        // minted id (`GetRestApi$restApiId`, `PutMethod$resourceId`,
        // `Stage$deploymentId`). (An `AWS::ApiGateway::Stage` is NOT one of them: "Ref
        // returns the stage name", which its own declaration states, and neither is an
        // `AWS::ApiGateway::Method`, which API Gateway addresses by the api, the resource and
        // the verb rather than by an id of its own.)
        || resource_type == APIGATEWAY_REST_API
        || resource_type == APIGATEWAY_RESOURCE
        || resource_type == APIGATEWAY_DEPLOYMENT
        // The four types that govern ACCESS to a REST API are four more, for the reason
        // their own constants record ([`APIGATEWAY_API_KEY`] and its siblings): each
        // create takes no identifier and answers the id API Gateway assigned, every call on
        // one takes that id, and a template declares a name or an association's members
        // rather than any of them.
        || resource_type == APIGATEWAY_API_KEY
        || resource_type == APIGATEWAY_AUTHORIZER
        || resource_type == APIGATEWAY_USAGE_PLAN
        || resource_type == APIGATEWAY_USAGE_PLAN_KEY
        // A method's REQUEST VALIDATOR is one more, for the reason its own constant records
        // ([`APIGATEWAY_REQUEST_VALIDATOR`]): `CreateRequestValidator` takes no identifier
        // and answers the id API Gateway assigned, the method that names it in its
        // `requestValidatorId` takes that id, and a template declares the validator's name
        // and the halves it checks rather than its identifier. (An `AWS::ApiGateway::Model`
        // is NOT one of them: "Ref returns the model name", which its own declaration
        // states, and that name is what every call on a model takes.)
        || resource_type == APIGATEWAY_REQUEST_VALIDATOR
        // A VPC LINK is one more, for the reason its own constant records
        // ([`APIGATEWAY_VPC_LINK`]): `CreateVpcLink` takes no identifier and answers the id
        // API Gateway assigned, every call on a link — and the private integration that
        // reaches a VPC through it — takes that id, and a template declares the link's name
        // rather than its identifier.
        || resource_type == APIGATEWAY_VPC_LINK
        // A CLIENT CERTIFICATE is one more, for the reason its own constant records
        // ([`APIGATEWAY_CLIENT_CERTIFICATE`]): `GenerateClientCertificate` takes no identifier
        // and ISSUES the certificate, answering the `clientCertificateId` API Gateway minted,
        // every call on one — and the stage that presents it to a backend — takes that id, and
        // a template declares the certificate's description rather than its identifier.
        || resource_type == APIGATEWAY_CLIENT_CERTIFICATE
        // Every AWS AppSync type is one more, for the reason [`appsync`] records:
        // `CreateGraphqlApi` takes the API's name and no identifier and answers the `apiId`
        // it minted beside the ARN built on it, each child's create is addressed by that id
        // and answers the ARN AppSync built for the child, and `CreateApiKey` answers the key
        // that service generated — none of which a naming rule could compute, and all of
        // which every later call on the resource takes.
        || appsync::is_appsync_type(resource_type)
        // A Cloud Map NAMESPACE and a SERVICE are two more, for the reason
        // [`service_discovery`] records: each namespace create takes the namespace's name and
        // no identifier and reports the `ns-…` id Cloud Map minted through its operation's
        // `Targets`, `CreateService` answers the `srv-…` id it minted, and each type's `Ref`
        // returns that id — which every later call on the resource takes. (An
        // `AWS::ServiceDiscovery::Instance` is NOT one of them: `RegisterInstance$InstanceId`
        // is "an identifier that you want to associate with the instance", so the id in force
        // is the declaration's own.)
        || service_discovery::identity_is_assigned(resource_type)
        // An Amazon EKS pod identity association is one more: that create takes no
        // identifier and answers the ARN and the id Amazon EKS minted for the association,
        // and "Ref returns the association ARN" — a template declares the cluster, the
        // namespace, the service account and the role, never the association's own id.
        || resource_type == EKS_POD_IDENTITY_ASSOCIATION
        // An Amazon ECS TASK SET is one more: `CreateTaskSet` takes no identifier for the task
        // set — a declaration states the cluster, the service, the task definition and the
        // scale — and answers the `TaskSet$id` that service minted, an `ecs-svc/` identifier no
        // naming rule could compute. That id is the last part of this type's own primary
        // identifier, which is what its `Ref` returns
        // ([`crate::state::provisioner::ecs_task_sets::ecs_task_set_identity`]), and the handle
        // `DescribeTaskSets`, `UpdateTaskSet`, `DeleteTaskSet` and
        // `UpdateServicePrimaryTaskSet$primaryTaskSet` each take. (An
        // `AWS::ECS::PrimaryTaskSet` and an `AWS::ECS::ClusterCapacityProviderAssociations` are
        // NOT: every part of each one's identity is a value its own declaration states, so
        // [`composite_identity`] computes it.)
        || resource_type == crate::state::provisioner::ecs_task_sets::ECS_TASK_SET
        // A State Manager association is one more: `CreateAssociation` takes no identifier
        // for the association — the `Name` a declaration states names the DOCUMENT the
        // association applies, and Systems Manager would refuse it as an association id —
        // and answers the `AssociationId` it minted, which is what "Ref returns the
        // association ID" publishes and what `DescribeAssociation`, `UpdateAssociation` and
        // `DeleteAssociation` each take.
        || resource_type == SSM_ASSOCIATION
        // A MAINTENANCE WINDOW and a target registered with one are two more:
        // `CreateMaintenanceWindow` takes the window's schedule and the `Name` it is CALLED
        // by and answers the `mw-…` `WindowId` Systems Manager minted, and
        // `RegisterTargetWithMaintenanceWindow` takes no identifier for the target at all and
        // answers the `WindowTargetId` it minted — each the handle every call on that
        // resource takes, and neither a value a naming rule here could compute.
        || resource_type == crate::state::provisioner::ssm_maintenance_windows::SSM_MAINTENANCE_WINDOW
        || resource_type
            == crate::state::provisioner::ssm_maintenance_windows::SSM_MAINTENANCE_WINDOW_TARGET
        // A PATCH BASELINE is one more: `CreatePatchBaseline` takes the `Name` the baseline
        // is CALLED by and the patches it approves, no identifier at all, and answers the
        // `pb-…` `BaselineId` Systems Manager minted — the handle `GetPatchBaseline`,
        // `UpdatePatchBaseline`, `DeletePatchBaseline` and the patch-group registers each
        // take, and what "Ref returns the patch baseline ID" publishes.
        || resource_type == crate::state::provisioning::ssm_patch_baseline::SSM_PATCH_BASELINE
        // An IAM access key is one more: "Ref returns the AWS::IAM::AccessKey resource's
        // access key ID", `CreateAccessKey` takes no identifier — the declaration states
        // only the user the key belongs to — and that id is what every IAM operation on a
        // key takes (`UpdateAccessKey$AccessKeyId`, `DeleteAccessKey$AccessKeyId`) and what
        // a client signs requests with, so the identifier in force is the one IAM minted.
        || resource_type == IAM_ACCESS_KEY
        // An IAM managed policy is one more: "Ref returns the ARN of the managed policy",
        // the handle every IAM operation on one takes (`GetPolicy$PolicyArn`,
        // `AttachRolePolicy$PolicyArn`), and `CreatePolicy` is what answers it — built from
        // the account and the path IAM created the policy under rather than from the name
        // the template declared.
        || resource_type == "AWS::IAM::ManagedPolicy"
        || resource_type == APIGATEWAYV2_API_MAPPING
        || resource_type == APIGATEWAYV2_INTEGRATION
        || resource_type == APIGATEWAYV2_ROUTE
        || resource_type == APIGATEWAYV2_DEPLOYMENT
        || resource_type == APIGATEWAYV2_AUTHORIZER
        || resource_type == APIGATEWAYV2_INTEGRATION_RESPONSE
        || resource_type == APIGATEWAYV2_ROUTE_RESPONSE
        || resource_type == APIGATEWAYV2_MODEL
        // A ROUTING RULE of a custom domain name is one more: `CreateRoutingRule` takes no
        // identifier for the rule — a declaration states the domain it is on, its priority,
        // its conditions and its actions — and answers the id API Gateway v2 minted together
        // with the ARN it built around that id, which is this type's primary identifier and
        // the handle `GetRoutingRule`, `PutRoutingRule` and `DeleteRoutingRule` address one
        // by.
        || resource_type == APIGATEWAYV2_ROUTING_RULE
        // An Amazon SNS subscription's identity is the ARN `Subscribe` answers with: it
        // carries an id SNS mints per subscription, which no naming rule could compute.
        || resource_type == SNS_SUBSCRIPTION
        // An Amazon Rekognition Custom Labels DATASET is one more: `CreateDataset` takes no
        // identifier for the dataset — a declaration states the project it belongs to, which
        // half of that project's data it is and the source its entries come from — and answers
        // the `DatasetArn` that service minted, whose own pattern ends in a numeric segment
        // Rekognition assigns (`project/…/dataset/(train|test)/[0-9]+`). That ARN is what the
        // type's `Ref` returns and the one handle `DescribeDataset`, `UpdateDatasetEntries` and
        // `DeleteDataset` each take. (An `AWS::Rekognition::Collection` is NOT one of them: its
        // `CollectionId` is the id its own declaration states, and an
        // `AWS::Rekognition::Project` is identified by the `ProjectName` it states.)
        || resource_type == crate::state::provisioner::rekognition_resources::REKOGNITION_DATASET
        // The two Application Auto Scaling types are the same case from the other side: a
        // scalable target's physical id is the
        // `<ResourceId>|<ScalableDimension>|<ServiceNamespace>` composite that service
        // identifies the target by ("scalable targets are uniquely identified by the
        // combination of resource ID, scalable dimension, and namespace"), and a scaling
        // policy's is the `PolicyARN` its own `PutScalingPolicy` answers — neither type
        // declares a name for the naming rules to compute an identifier from, so the id in
        // force is the one its create answered
        // ([`crate::state::provisioning::application_auto_scaling`]). A policy that names its
        // target by `Ref` therefore resolves to that composite, which is what
        // `ScalingTargetId` takes.
        || resource_type
            == crate::state::provisioning::application_auto_scaling::APPLICATION_AUTO_SCALING_SCALABLE_TARGET
        || resource_type
            == crate::state::provisioning::application_auto_scaling::APPLICATION_AUTO_SCALING_SCALING_POLICY
        // An Amazon Athena NAMED QUERY is one more: `CreateNamedQuery` takes the query's
        // `Name`, the database and the SQL and no identifier of its own, and answers the
        // `NamedQueryId` Athena minted, which is what "Ref returns the named query ID"
        // publishes and what `GetNamedQuery`, `UpdateNamedQuery` and `DeleteNamedQuery` each
        // take — a workgroup may hold two queries under one name, so the declared name could
        // not stand in for that id
        // ([`crate::state::provisioning::athena_resources`]). (An `AWS::Athena::WorkGroup` and
        // an `AWS::Athena::DataCatalog` are NOT: each is created UNDER the name its
        // declaration states, which is what every Athena call on one takes.)
        || resource_type == crate::state::provisioning::athena_resources::ATHENA_NAMED_QUERY
        || resource_type == FIS_EXPERIMENT_TEMPLATE
        || resource_type == WAF_REGIONAL_IP_SET
        || resource_type == WAFV2_WEB_ACL
        || resource_type == WAFV2_RULE_GROUP
        || resource_type == WAFV2_IP_SET
        || resource_type == WAFV2_REGEX_PATTERN_SET
        || resource_type == GRAFANA_WORKSPACE
        || resource_type == EMR_CLUSTER
        // An Amazon EMR STUDIO is one more: `CreateStudio` takes no identifier — a template
        // declares the Studio's `Name`, the network it runs in and the roles it assumes —
        // and answers the `es-…` `StudioId` that service minted, which is what "Ref returns
        // the Studio ID" publishes and what `DescribeStudio`, `UpdateStudio`,
        // `DeleteStudio` and every session mapping on the Studio each take.
        || resource_type == crate::state::provisioner::emr_resources::EMR_STUDIO
        || resource_type == GUARDDUTY_DETECTOR
        // The two CloudFront types this emulator provisions are the same case: neither
        // `CreateDistribution` nor `CreateOriginAccessControl` takes an identifier — a
        // template declares a configuration, never an id — and each answers the `E…`
        // identifier CloudFront minted, which is what "Ref returns the distribution ID" and
        // "Ref returns the origin access control's unique identifier" publish and what every
        // CloudFront call on the resource takes.
        || resource_type == CLOUDFRONT_DISTRIBUTION
        || resource_type == CLOUDFRONT_ORIGIN_ACCESS_CONTROL
        // A cache policy, a public key and a key group are the same case: `CreateCachePolicy`,
        // `CreatePublicKey` and `CreateKeyGroup` each take a CONFIGURATION and no identifier,
        // and each answers the id CloudFront minted — the UUID of a policy or a group, the
        // `K`-prefixed id of a key — which is what "Ref returns the cache policy's unique
        // identifier", "Ref returns the public key ID" and "Ref returns the key group ID"
        // publish and what every CloudFront call on one of them takes (a cache behaviour's
        // `CachePolicyId`, a key group's `Items`, `GetKeyGroup$Id`).
        || resource_type == CLOUDFRONT_CACHE_POLICY
        || resource_type == CLOUDFRONT_ORIGIN_REQUEST_POLICY
        || resource_type == CLOUDFRONT_ORIGIN_ACCESS_IDENTITY
        || resource_type == CLOUDFRONT_PUBLIC_KEY
        || resource_type == CLOUDFRONT_KEY_GROUP
        // A key value store, a connection function and a connection group are three more:
        // each create takes the resource's NAME and no identifier, and each answers an
        // identity that service minted — the store's UUID and the ARN around it, the
        // function's id and ARN, the group's id — which is what each type's `Ref` returns and
        // what every CloudFront call on one of them takes (a function's
        // `KeyValueStoreAssociation$KeyValueStoreARN`, `PublishConnectionFunction$Id`,
        // a distribution tenant's `ConnectionGroupId`).
        || resource_type == CLOUDFRONT_KEY_VALUE_STORE
        || resource_type == CLOUDFRONT_FUNCTION
        || resource_type == CLOUDFRONT_CONNECTION_FUNCTION
        || resource_type == CLOUDFRONT_CONNECTION_GROUP
        // A VPC ORIGIN is one more: `CreateVpcOrigin` takes the endpoint CONFIGURATION and no
        // identifier, and answers the `vo_…` id CloudFront minted together with the ARN built
        // around it — which is what "Ref returns the VPC origin ID" publishes and what every
        // call on one takes (`GetVpcOrigin$Id`, `DeleteVpcOrigin$Id`, an origin's
        // `VpcOriginConfig$VpcOriginId`).
        || resource_type == CLOUDFRONT_VPC_ORIGIN
        // A GuardDuty trusted IP list is the same case: `CreateIPSet` takes no identifier
        // for the list — the `Name` a declaration states is only the user-friendly name of
        // the IPSet — and answers the `IpSetId` GuardDuty minted, which is what that
        // type's `Ref` returns and what every GuardDuty call on the list takes.
        || resource_type == GUARDDUTY_IP_SET
        // An AWS Lambda function VERSION is the same case: `PublishVersion` takes no
        // identifier for the version — the declaration states the function it is published
        // from — and answers the number Lambda allocated, which "Ref returns the ARN of the
        // function version" embeds and which every call on the version takes as its
        // qualifier.
        || resource_type == LAMBDA_VERSION
        // A LAYER VERSION is one more, for the reason [`lambda_layer_version`] records: "Ref
        // returns the ARN of the layer version", and that ARN carries the VERSION NUMBER
        // Lambda allocated ("each time you call PublishLayerVersion with the same layer name,
        // a new version is created") — a template declares the layer's name and its content,
        // never that number, and `GetLayerVersion`, `DeleteLayerVersion` and a function's own
        // `Layers` each take the ARN carrying it.
        || resource_type
            == crate::state::provisioning::lambda_layer_version::LAMBDA_LAYER_VERSION
        // An EVENT SOURCE MAPPING is one more, for the reason
        // [`lambda_event_source_mapping`] records: `CreateEventSourceMapping` takes no
        // identifier for the mapping — a declaration states the event source and the function
        // it wires together — and answers the `UUID` Lambda assigned, which is what "Ref
        // returns the event source mapping's ID" publishes and what `GetEventSourceMapping`,
        // `UpdateEventSourceMapping` and `DeleteEventSourceMapping` each take. The type
        // declares no name at all, so nothing else could say which mapping a re-deploy means.
        || resource_type
            == crate::state::provisioning::lambda_event_source_mapping::LAMBDA_EVENT_SOURCE_MAPPING
        // An AWS Step Functions state machine VERSION is the same case:
        // `PublishStateMachineVersion` takes no identifier for the version — the declaration
        // states the machine it is published from — and answers the ARN that service minted,
        // which carries the version number it allocated. An ALIAS of one is too: its ARN is
        // built on the state machine the versions it routes to belong to, which the
        // declaration names only through those versions, and
        // `CreateStateMachineAlias` is what answers it. Each ARN is what that type's `Ref`
        // returns and what every Step Functions call on the resource takes
        // ([`crate::state::provisioning::step_functions_versions_and_aliases`]).
        || resource_type
            == crate::state::provisioning::step_functions_versions_and_aliases::STEP_FUNCTIONS_STATE_MACHINE_VERSION
        || resource_type
            == crate::state::provisioning::step_functions_versions_and_aliases::STEP_FUNCTIONS_STATE_MACHINE_ALIAS
        || resource_type == KMS_KEY
        // All three Amazon EFS types are in the same position: `CreateFileSystem`,
        // `CreateAccessPoint` and `CreateMountTarget` each take NO identifier for the resource
        // they make — none of the three types declares a name at all, a file system's name
        // being a tag of its own tag set — and each answers the identifier that service minted
        // (`fs-`, `fsap-`, `fsmt-`), which is what the type's `Ref` returns and the handle
        // every later Amazon EFS call on the resource takes
        // ([`crate::state::provisioning::efs_resources`]).
        || resource_type == EFS_FILE_SYSTEM
        || resource_type == EFS_ACCESS_POINT
        || resource_type == EFS_MOUNT_TARGET
        // A SageMaker AI Studio DOMAIN is in the same position: `CreateDomain` takes the
        // domain's `DomainName` and no identifier for it, and answers the `d-…` `DomainId`
        // SageMaker assigned — "Ref returns the domain ID", which is the handle
        // `DescribeDomain`, `DeleteDomain` and every member's own create take, and which the
        // type's `DomainId` pattern (`^d-(-*[a-z0-9]){1,61}$`) constrains so that a name is a
        // value that API rejects where an id is required
        // ([`crate::state::provisioning::sagemaker_studio`]). (A USER PROFILE and a SPACE are
        // NOT in it: each is identified by the PAIR its own API addresses it by, which the
        // declaration itself states — see [`composite_identity_properties`].)
        || resource_type == crate::state::provisioning::sagemaker_studio::SAGEMAKER_DOMAIN
        || resource_type == AOSS_COLLECTION
        // An OpenSearch Serverless managed interface ENDPOINT is the same case:
        // `CreateVpcEndpoint` takes the endpoint's `name` and no identifier for it, and
        // answers the `vpce-…` `id` that service minted — which `VpcEndpointId`'s own
        // `^vpce-[0-9a-z]*$` constrains, so a name is a value that service's API rejects
        // where an id is required — and that id is what `BatchGetVpcEndpoint`,
        // `UpdateVpcEndpoint` and `DeleteVpcEndpoint` each take.
        || resource_type == AOSS_VPC_ENDPOINT
        || resource_type == ACM_ACME_ENDPOINT
        // An ACM certificate is the same case: `RequestCertificate` takes no identifier
        // and answers the `CertificateArn` ACM minted, and that type's `Ref` returns that
        // ARN — a template declares the domain the certificate covers, never its id.
        || resource_type == ACM_CERTIFICATE
        // An Amazon Web Services Private CA certificate authority is the same case:
        // `CreateCertificateAuthority` takes no identifier and answers the
        // `CertificateAuthorityArn` that service minted, and "Ref returns the Amazon
        // Resource Name (ARN) of the private CA" — a template declares the CA's key, its
        // signing algorithm and its subject, never its id.
        || resource_type == ACMPCA_CERTIFICATE_AUTHORITY
        // A certificate that private CA issued is one more: `IssueCertificate` takes no
        // identifier and answers the `CertificateArn` it minted — which carries the SERIAL
        // that service allocated, so no naming rule could compute it — and that type's
        // `Ref` returns that ARN. A template declares the certification request and the
        // validity, never the certificate's id.
        || resource_type == ACMPCA_CERTIFICATE
        || resource_type == MSK_CLUSTER
        || resource_type == MSK_SERVERLESS_CLUSTER
        // An Amazon MSK CONFIGURATION is the same case: `CreateConfiguration` takes the
        // configuration's NAME and answers the `Arn` it minted, whose resource path carries
        // an identifier and an ordinal that service allocated — so the ARN that type's `Ref`
        // returns cannot be recomputed from the template.
        || resource_type == MSK_CONFIGURATION
        || resource_type == ROUTE53RESOLVER_ENDPOINT
        || resource_type == ROUTE53RESOLVER_RULE
        || resource_type == ROUTE53RESOLVER_RULE_ASSOCIATION
        || resource_type == ROUTE53RESOLVER_FIREWALL_DOMAIN_LIST
        || resource_type == ROUTE53RESOLVER_FIREWALL_RULE_GROUP
        || resource_type == ROUTE53RESOLVER_FIREWALL_RULE_GROUP_ASSOCIATION
        || resource_type == ROUTE53_HOSTED_ZONE
        // An Amazon Route 53 health check is one more: `CreateHealthCheck` takes no
        // identifier and answers the id Route 53 assigned, "Ref returns the health check
        // ID", and that id is what `GetHealthCheck` and a record set's own `HealthCheckId`
        // take — the type declares no name a rule could compute it from.
        || resource_type == ROUTE53_HEALTH_CHECK
        // The three further Amazon Route 53 types are three more, for the reason their own
        // constants record ([`ROUTE53_CIDR_COLLECTION`] and its siblings): each create
        // answers an identifier that service minted — a collection's UUID, a reusable
        // delegation set's `N…` id, a traffic policy's id — and every call on one takes it,
        // while a template declares a name, a caller reference or a document rather than
        // any of them.
        || resource_type == ROUTE53_CIDR_COLLECTION
        || resource_type == ROUTE53_DELEGATION_SET
        || resource_type == ROUTE53_TRAFFIC_POLICY
        // An Amazon RDS Proxy target group is one more: a proxy comes up WITH the one
        // target group it has, so a declaration of this type states the proxy and the
        // databases the group routes to, never an identifier — and that type's `Ref`
        // returns the `TargetGroupArn` Amazon RDS minted for the group, which is what
        // every call that reports one carries.
        || resource_type == RDS_DB_PROXY_TARGET_GROUP
        || resource_type == TRANSFER_SERVER
        // An Amazon MQ broker is one more: `CreateBroker` takes the broker's NAME and
        // answers with the `brokerId` Amazon MQ generated for it, which is the id that
        // type's `Ref` returns and the one every `mq` operation addresses a broker by.
        || resource_type == MQ_BROKER
        // An Amazon MQ CONFIGURATION is the same case: `CreateConfiguration` takes the
        // configuration's NAME and answers with the `Id` Amazon MQ generated for it, which
        // is the id that type's `Ref` returns and the `@httpLabel` every call on a
        // configuration takes.
        || resource_type == MQ_CONFIGURATION
        // An AWS Backup plan and one of its resource selections are two more:
        // `CreateBackupPlan` takes no identifier and answers the `BackupPlanId` it minted
        // (which is what that type's `Ref` returns), and `CreateBackupSelection` answers
        // the `SelectionId` it assigned to the selection it created. A template declares
        // the plan's BODY and the selection's, never either id. (An `AWS::Backup::
        // BackupVault` is NOT one of them: its identifier is the vault name the template
        // states.)
        || resource_type == BACKUP_PLAN
        || resource_type == BACKUP_SELECTION
        // An Amazon Bedrock guardrail is one more: `CreateGuardrail` takes the guardrail's
        // NAME and answers the `guardrailId` Amazon Bedrock minted (`^[a-z0-9]+$`) with the
        // ARN built around it, which is the identity every guardrail API takes as its
        // `guardrailIdentifier` — a pattern the name itself does not satisfy — so neither
        // the id nor the ARN can be computed from the template.
        || resource_type == BEDROCK_GUARDRAIL
        // An Amazon Bedrock KNOWLEDGE BASE is the same case: `CreateKnowledgeBase` takes the
        // knowledge base's NAME and answers the `knowledgeBaseId` Amazon Bedrock Agent minted
        // (`^[0-9a-zA-Z]{10}$`, exactly ten alphanumeric characters), "Ref returns the
        // knowledge base ID", and that id is the `knowledgeBaseId` every knowledge-base API
        // takes — a pattern no name CloudFormation decides can satisfy — so the identifier in
        // force is the one that service answered.
        || resource_type == BEDROCK_KNOWLEDGE_BASE
        // An Amazon Bedrock DATA SOURCE of one of those knowledge bases is the same case:
        // `CreateDataSource` takes the data source's NAME and answers the `dataSourceId`
        // Amazon Bedrock Agent minted (`^[0-9a-zA-Z]{10}$`), which every later call on the
        // data source takes beside the knowledge base's own id — and the type's primary
        // identifier, which is what `Ref` returns, is that pair
        // ([`bedrock_data_source_identity`]). No name CloudFormation decides satisfies that
        // pattern, so the identifier in force is the one that service answered.
        || resource_type == BEDROCK_DATA_SOURCE
        // An Amazon Bedrock FLOW is the same case: `CreateFlow` takes the flow's NAME and
        // answers the `id` Amazon Bedrock Agent minted (`^[0-9a-zA-Z]{10}$`) with the
        // `flow/<id>` ARN built around it, which is the ARN that type's `Ref` returns and the
        // identity every flow API — the build-time calls and the runtime `InvokeFlow` alike —
        // addresses a flow by, so neither the id nor the ARN can be computed from the
        // template.
        || resource_type == BEDROCK_FLOW
        // The six Amazon Bedrock AgentCore types a stack really creates are six more: none of
        // their creates takes an identifier, and each answers the one that control plane
        // minted — an `agentRuntimeId` and a `memoryId` of the form `<name>-<10 characters>`,
        // a `gatewayId` of the same form lower-cased, a `targetId` of exactly ten
        // alphanumerics, and a `codeInterpreterId` / `browserId` — which is what every later
        // call on the resource takes and what that type's `Ref` returns, a target's beside the
        // gateway it is served by ([`agentcore_gateway_target_identity`]). The suffix each of
        // those identifiers carries is that service's, so no naming rule could compute one.
        // (An `AWS::BedrockAgentCore::WorkloadIdentity` is NOT one of them: "Ref returns the
        // name", which the declaration states and the identity is created under.)
        || agentcore_identity_is_assigned(resource_type)
        // An Amazon Bedrock SESSION is one more: `CreateSession` takes no identifier for the
        // session — the declaration states the encryption key, the metadata and the tags — and
        // answers the `sessionId` the Bedrock Agent runtime drew for it (a uuid) with the
        // `session/<uuid>` ARN built around it, which is the ARN that type's `Ref` returns and
        // one of the two forms every call on a session takes as its `sessionIdentifier`. No
        // name CloudFormation decides is either, so the identity in force is the one that
        // service answered.
        || resource_type == BEDROCK_SESSION
        // An APPLICATION INFERENCE PROFILE is one more: `CreateInferenceProfile` takes the
        // profile's name and its model source and takes no identifier for it, answering the
        // `inferenceProfileArn` Amazon Bedrock minted — whose resource segment carries an id of
        // that service's own (`InferenceProfileId`: `^[a-zA-Z0-9-:.]+$`) — which is the ARN that
        // type's `Ref` returns and one of the two forms every call on a profile takes as its
        // `inferenceProfileIdentifier`. No name CloudFormation decides is either, so the
        // identity in force is the one that service answered.
        || resource_type == BEDROCK_APPLICATION_INFERENCE_PROFILE
        // The three Amazon Bedrock DATA AUTOMATION types are three more: none of their creates
        // takes an identifier for the resource — each takes the name the declaration states —
        // and each answers the ARN that service minted, whose resource segment carries 12 to 36
        // characters of `[a-zA-Z0-9-]` it drew. That ARN is what every call on the resource
        // takes and what the type's `Ref` returns, so no name CloudFormation decides is the
        // identity and the one in force is what that service answered.
        || resource_type == BEDROCK_BLUEPRINT
        || resource_type == BEDROCK_DATA_AUTOMATION_PROJECT
        || resource_type == BEDROCK_DATA_AUTOMATION_LIBRARY
        // An Amazon Braket SPENDING LIMIT is one more: `CreateSpendingLimit` takes no
        // identifier for the limit — the declaration states the device, the amount and the
        // period — and answers the `spendingLimitArn` that service minted, whose resource
        // path carries a uuid of its own, so the identifier in force is the one it answered.
        // That ARN is also what every call on a limit takes (`UpdateSpendingLimit` and
        // `DeleteSpendingLimit` each address one by it).
        || resource_type == BRAKET_SPENDING_LIMIT
        // An Amazon CloudWatch investigations INVESTIGATION GROUP is one more:
        // `CreateInvestigationGroup` takes no identifier for the group — the declaration
        // states its name, the role it gathers data as and its configuration — and answers the
        // `arn` that service minted, whose resource path carries sixteen characters it drew.
        // That ARN is what the type's `Ref` returns and the `identifier` every later call on
        // the group takes, so the identity in force is the one it answered.
        || resource_type == AIOPS_INVESTIGATION_GROUP
        // The two Amazon GameLift Servers types are two more: neither `CreateBuild` nor
        // `CreateScript` takes an identifier for the resource — a template declares the
        // name, the runtime and the Amazon S3 storage location the code is shipped from —
        // and each answers the `build-…`/`script-…` id that service drew, which is what the
        // type's `Ref` returns and the handle every later call on a build or a script takes.
        // No name CloudFormation decides is either, so the identity in force is the one that
        // service answered.
        || resource_type == GAMELIFT_BUILD
        || resource_type == GAMELIFT_SCRIPT
        // The four AWS END USER MESSAGING SMS AND VOICE types whose identity that service
        // assigns are four more: none of their creates takes an identifier for the resource
        // — a template declares a protect configuration's deletion protection, an RCS
        // agent's opt-out list, a registration's type and an attachment's document — and
        // each answers the identifier it drew (a `ProtectConfigurationId`, an `RcsAgentId`,
        // a `RegistrationId`, a `RegistrationAttachmentId`), which is what that type's `Ref`
        // returns and the handle every later call on the resource takes. No name
        // CloudFormation decides is any of them, so the identity in force is the one that
        // service answered. (Its CONFIGURATION SET and OPT-OUT LIST are not here: each is
        // named by the declaration, and that name is what the owning service keys it by.)
        || resource_type == SMSVOICE_PROTECT_CONFIGURATION
        || resource_type == SMSVOICE_RCS_AGENT
        || resource_type == SMSVOICE_REGISTRATION
        || resource_type == SMSVOICE_REGISTRATION_ATTACHMENT
        // The two AWS HEALTHLAKE types are two more: neither create takes an identifier for
        // the resource — a template declares a data store's name and FHIR release version,
        // and a profile's name, source format and initial content — and each answers the
        // identifier HealthLake minted (a `DatastoreId`, a `ProfileId` of the form
        // `^[a-f0-9]{32}$`). The data store's `Ref` returns that id and the profile's the ARN
        // built around its own, and every HealthLake call on either addresses it by one of
        // them, so no name CloudFormation decides is the identity and the one in force is what
        // that service answered.
        || resource_type == HEALTHLAKE_FHIR_DATASTORE
        || resource_type == HEALTHLAKE_DATA_TRANSFORMATION_PROFILE
        // The two Amazon VPC LATTICE types are two more: `CreateServiceNetwork` takes the
        // network's name and answers the `sn-` id it drew with the ARN built around it, and
        // `CreateAccessLogSubscription` takes no identifier at all and answers the `als-` id
        // it drew with its own ARN. Either type's `Ref` returns that ARN, and the id inside it
        // is what every call of that service on the resource takes, so no name CloudFormation
        // decides is either and the identity in force is the one that service answered.
        || resource_type == VPCLATTICE_SERVICE_NETWORK
        || resource_type == VPCLATTICE_ACCESS_LOG_SUBSCRIPTION
        // The four RESOURCE-ACCESS types of that service are four more, for the same reason:
        // a template declares a gateway's name, a configuration's definition and an
        // association's two sides, and each create answers the id it drew (`rgw-`, `rcfg-`,
        // `snra-`, `snva-`) with the ARN built around it, which is what that type's `Ref`
        // returns and the handle every later call on the resource takes.
        || resource_type == VPCLATTICE_RESOURCE_GATEWAY
        || resource_type == VPCLATTICE_RESOURCE_CONFIGURATION
        || resource_type == VPCLATTICE_SERVICE_NETWORK_RESOURCE_ASSOCIATION
        || resource_type == VPCLATTICE_SERVICE_NETWORK_VPC_ASSOCIATION
        // The two AWS DIRECT CONNECT types are two more: neither create takes an identifier
        // for the resource — a template declares a gateway's name and Amazon-side ASN, and a
        // group's name and intended resiliency model — and each answers the identifier that
        // service drew (an opaque `directConnectGatewayId`, a `resiliencyGroupId` of the form
        // `^(dxrg|DXRG)-[0-9a-zA-Z]{17}$`) with the ARN built around it. That ARN is what
        // either type's `Ref` returns, and the id inside it is what every Direct Connect call
        // on the resource takes, so no name CloudFormation decides is either and the identity
        // in force is the one that service answered.
        || resource_type == DIRECT_CONNECT_GATEWAY
        || resource_type == DIRECT_CONNECT_RESILIENCY_GROUP
        // The two DIRECTORY SERVICE directory types are two more: neither `CreateMicrosoftAD`
        // nor `CreateDirectory` takes an identifier — a declaration states the directory's
        // fully qualified name, its administrator password and the subnets its servers go in —
        // and each answers the `DirectoryId` that service minted (`^d-[0-9a-f]{10}$`), which
        // is what "Ref returns the directory ID" publishes and the handle every operation on
        // a directory takes. The `Name` a declaration states is the DNS name the directory
        // answers for, which no call of that service accepts as a directory id, so the
        // identity in force is the one that service answered.
        || resource_type == DIRECTORY_SERVICE_MICROSOFT_AD
        || resource_type == DIRECTORY_SERVICE_SIMPLE_AD
        // The two Billing and Cost Management Dashboards types are two more: neither
        // `CreateDashboard` nor `CreateScheduledReport` takes an identifier — a template
        // declares a dashboard's widgets and a report's schedule — and each answers the
        // `arn` that service minted, whose resource segment carries a 32-character
        // hexadecimal id of its own that no name CloudFormation decides is. That ARN is what
        // either type's `Ref` returns and the handle every call on the resource takes, so the
        // identity in force is the one that service answered.
        || resource_type == BCM_DASHBOARD
        || resource_type == BCM_SCHEDULED_REPORT
        // An Application Signals INSTRUMENTATION CONFIG is one more:
        // `CreateInstrumentationConfiguration` takes no identifier for the configuration — the
        // declaration states the service, the environment, the signal type and the code
        // location — and answers the `LocationHash` it computed from that location (16 lowercase
        // hexadecimal characters, which no name CloudFormation decides is) beside the ARN it
        // minted around it. That hash is the last part of this type's own primary identifier
        // ([`APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY`]), and the ARN the type
        // publishes beside it, so the identity in force is the one that service answered. (A
        // SERVICE LEVEL OBJECTIVE is NOT one of them: its ARN carries the objective's own name,
        // which the naming rules compute; nor is a GROUPING CONFIGURATION, whose identity is the
        // account the configuration belongs to.)
        || resource_type == APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG
        // A CLEAN ROOMS ML training dataset is one more: `CreateTrainingDataset` takes no
        // identifier for the dataset — the declaration states its name, the role that reads
        // the data and the datasets themselves — and answers the `trainingDatasetArn` that
        // service minted, whose resource path carries an identifier of its own. That ARN is
        // what "Ref returns the training dataset ARN" publishes and what `GetTrainingDataset`
        // and `DeleteTrainingDataset` each address a dataset by, and no name CloudFormation
        // decides is such an ARN, so the identity in force is the one that service answered.
        || resource_type == CLEANROOMSML_TRAINING_DATASET
        // A QUICK SETUP configuration manager is one more: `CreateConfigurationManager` takes
        // no identifier for the manager — the declaration states its name, its description and
        // the definitions it deploys — and answers the `ManagerArn` that service minted, whose
        // resource path carries a uuid of its own. That ARN is what "Ref returns the
        // ManagerArn" publishes and what `GetConfigurationManager`,
        // `UpdateConfigurationManager` and `DeleteConfigurationManager` each address a manager
        // by, and no name CloudFormation decides is such an ARN, so the identity in force is
        // the one that service answered.
        || resource_type == SSM_QUICKSETUP_CONFIGURATION_MANAGER
        // A CLOUDHSM cluster is one more: `CreateCluster` takes no identifier for the cluster —
        // the declaration states the subnets it goes in, the HSM type it will hold and the mode
        // it runs in — and answers the `ClusterId` that service minted
        // (`^cluster-[2-7a-zA-Z]{11,16}$`), which is the handle every operation on a cluster
        // takes and what the ARN this type's `Ref` returns carries. No name CloudFormation
        // decides satisfies that pattern, so the identity in force is the one that service
        // answered.
        || resource_type == CLOUDHSM_CLUSTER
        // A RECYCLE BIN retention rule is one more: `CreateRule` takes no identifier for the
        // rule — the declaration states what to retain and for how long — and answers the
        // `Identifier` that service minted (`^[0-9a-zA-Z]{11}$`) with the `RuleArn` built
        // around it, which is what "Ref returns the rule ARN" publishes and what `GetRule`,
        // `UpdateRule` and `DeleteRule` each address a rule by. A name CloudFormation decides
        // satisfies neither, so the identity in force is the one that service answered.
        || resource_type == RBIN_RULE
        // An AMAZON DATA LIFECYCLE MANAGER lifecycle policy is one more:
        // `CreateLifecyclePolicy` takes no identifier for the policy — the declaration states the
        // role it runs under, its activation state and the configuration it runs — and answers
        // the `PolicyId` that service minted (`^policy-[a-f0-9]+$`), which is what "Ref returns
        // the policy id" publishes and what `GetLifecyclePolicy`, `UpdateLifecyclePolicy` and
        // `DeleteLifecyclePolicy` each address a policy by. A name CloudFormation decides
        // satisfies that pattern for no template, so the identity in force is the one that
        // service answered.
        || resource_type == DLM_LIFECYCLE_POLICY
        // The four AWS NETWORK SECURITY MANAGER entities are four more: `CreateRule`,
        // `CreateTemplate`, `CreatePolicy` and `CreateScope` each take the entity's NAME and
        // no identifier at all, and each answers the `…Id` that service minted
        // (`^[a-z0-9]{1,64}$`) with the ARN built around it — the value that type's `Ref`
        // returns and the `…Identifier` every later call on the entity takes, which is what
        // its ARN member's own `resourceIdentifier` says. No name CloudFormation decides is
        // such an ARN, so the identity in force is the one that service answered.
        || resource_type == NETWORK_SECURITY_MANAGER_RULE
        || resource_type == NETWORK_SECURITY_MANAGER_TEMPLATE
        || resource_type == NETWORK_SECURITY_MANAGER_POLICY
        || resource_type == NETWORK_SECURITY_MANAGER_SCOPE
        // A BACKUP GATEWAY hypervisor is one more: `ImportHypervisorConfiguration` takes no
        // identifier for the hypervisor — the declaration states the host, the name and the
        // credentials — and answers the `HypervisorArn` that service minted, which is what
        // "Ref returns the hypervisor ARN" publishes and what `GetHypervisor`,
        // `UpdateHypervisor`, `DeleteHypervisor` and `AssociateGatewayToServer` each address
        // one by. A name CloudFormation decides is no such ARN, so the identity in force is
        // the one that service answered.
        || resource_type == BACKUP_GATEWAY_HYPERVISOR
        // An IoT CORE DEVICE ADVISOR suite definition is one more: `CreateSuiteDefinition`
        // takes no identifier for the suite — the declaration states the suite's configuration
        // and its tags — and answers the `suiteDefinitionId` that service drew (a `UUID`,
        // `@length 12..36`) with the `suitedefinition/<id>` ARN built around it. That id is
        // what "Ref returns the suite definition id" publishes and what `GetSuiteDefinition`,
        // `UpdateSuiteDefinition` and `DeleteSuiteDefinition` each address one by, and no name
        // CloudFormation decides is such a uuid, so the identity in force is the one that
        // service answered.
        || resource_type == IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION
        // A NEPTUNE ANALYTICS graph and a snapshot of one are two more: `CreateGraph` takes
        // the graph's NAME and `CreateGraphSnapshot` the snapshot's, neither takes an
        // identifier, and each answers the id that service minted — `^g-[a-z0-9]{10}$` for a
        // graph and `^gs-[a-z0-9]{10}$` for a snapshot, which is what each type's `Ref`
        // returns and what every call on the resource takes as its `graphIdentifier` /
        // `snapshotIdentifier`. Neither modelled name can satisfy those patterns (both
        // exclude the id prefix by a negative lookahead), so the identity in force is the one
        // that service answered.
        || resource_type == NEPTUNE_GRAPH_GRAPH
        || resource_type == NEPTUNE_GRAPH_GRAPH_SNAPSHOT
        // The three Amplify UI Builder entities are three more: each create takes the app and
        // the environment as its URI labels and the entity's NAME in its payload, takes no
        // identifier at all, and answers the `Uuid` that service assigned — the `id` of the
        // type's own resource shape, which `GetComponent`, `UpdateForm`, `DeleteTheme` and
        // every other call on one is addressed by. A name no such call takes could not stand
        // in for it, so the identifier in force is the one that service answered.
        || resource_type == AMPLIFYUIBUILDER_COMPONENT
        || resource_type == AMPLIFYUIBUILDER_FORM
        || resource_type == AMPLIFYUIBUILDER_THEME
        // An AWS Amplify APP is one more: `CreateApp` takes the app's NAME and no identifier,
        // and answers the `appId` that service minted (`^d[a-z0-9]+$`) with the ARN built
        // around it — which is what that type's `Ref` returns and what every Amplify call on
        // an app, and on everything hanging off one, takes. A name CloudFormation decides
        // satisfies neither, so the identity in force is the one that service answered.
        || resource_type == AMPLIFY_APP
        // An AWS Amplify BRANCH is one more: `CreateBranch` takes the app as its URI label and
        // the branch's NAME in its payload, and answers the `branchArn` that service built —
        // the app's own ARN, around the `appId` it minted, with the branch's name appended —
        // which is what that type's `Ref` returns. No naming rule can compute an ARN carrying a
        // minted id, so the identity in force is the one that service answered.
        || resource_type == AMPLIFY_BRANCH
        // An Amazon EC2 VPC, a subnet of one and a security group of one are three more:
        // `CreateVpc`, `CreateSubnet` and `CreateSecurityGroup` take no identifier at all —
        // EC2 assigns each resource an id of its own published form — and "Ref returns the
        // ID of the VPC" / "Ref returns the subnet ID" / "Ref returns the ID of the security
        // group", so the identifier in force is the one that service answered. The `vpc-`/
        // `subnet-`/`sg-` value [`ec2_network_identity`] computes is the stand-in a
        // declaration is resolved against before any of them exists, which the deployment
        // then re-points at the id EC2 minted. (A security group's NAME is still
        // CloudFormation's — that is what the create is made under — which is why
        // [`declared_name_property`] names the property a declaration states it in.)
        || resource_type == EC2_VPC
        || resource_type == EC2_SUBNET
        || resource_type == EC2_SECURITY_GROUP
        // An Amazon EBS volume is a fourth: `CreateVolume` takes no identifier either — the
        // declaration states the volume's zone, size and type — Amazon EC2 assigns the
        // `vol-` id, and "Ref returns the volume ID", so the identifier in force is the one
        // that service answered.
        || resource_type == EC2_VOLUME
        // A CLIENT VPN ENDPOINT is one more: `CreateClientVpnEndpoint` takes no identifier
        // either — the declaration states the client address range, the certificates the
        // endpoint presents and the options it authenticates a client against — Amazon EC2
        // assigns the `cvpn-endpoint-` id, and "Ref returns the Client VPN endpoint ID", so
        // the identifier in force is the one that service answered the create with.
        || resource_type == crate::state::provisioner::client_vpn::EC2_CLIENT_VPN_ENDPOINT
        // The VPC-NETWORK resources whose identifier Amazon EC2 ASSIGNS are in exactly the
        // same position as the VPC and the subnet above, and for the same reason: each of
        // their creates takes no identifier for the resource it makes and answers the id that
        // service minted, which is what the type's `Ref` returns — "Ref returns the ID of the
        // internet gateway" (`CreateInternetGateway`'s `internetGatewayId`), "Ref returns the
        // route table ID" (`CreateRouteTable`'s `routeTableId`), the subnet-route table
        // association's own id (`AssociateRouteTable`'s `associationId`), "Ref returns the ID
        // of the VPC endpoint" (`CreateVpcEndpoint`'s `vpcEndpointId`), the virtual private
        // gateway's id (`CreateVpnGateway`'s `vpnGatewayId`), "Ref returns the NAT gateway ID"
        // (`CreateNatGateway`'s `natGatewayId`) and, for an Elastic IP, the address
        // `AllocateAddress` allocated. None of them is a function of anything a template
        // declares — a route table and an internet gateway declare no name at all — so the
        // value [`ec2_network_identity`] computes is only the stand-in a declaration is
        // resolved against until the create answers, and a later deployment of the same
        // declaration carries the id in force forward instead of creating the resource all
        // over again.
        || resource_type == EC2_INTERNET_GATEWAY
        || resource_type == EC2_ROUTE_TABLE
        || resource_type == EC2_SUBNET_ROUTE_TABLE_ASSOCIATION
        || resource_type == EC2_VPC_ENDPOINT
        || resource_type == EC2_VPN_GATEWAY
        || resource_type == EC2_NAT_GATEWAY
        || resource_type == EC2_EIP
        // An elastic network INTERFACE is one more: `CreateNetworkInterface` takes the subnet
        // the interface goes in and no identifier for the interface itself, Amazon EC2 assigns
        // the `eni-` id, and "Ref returns the ID of the network interface" — so the identifier
        // in force is the one that service answered the create with.
        || resource_type == EC2_NETWORK_INTERFACE
        // The two types that declare ONE security-group rule as a resource of its own are in
        // the same position: neither create takes an identifier for the rule — the declaration
        // states the group it is on and the permission it grants — Amazon EC2 mints the
        // `sgr-` id for the rule it added, and "Ref returns the ID of the security group
        // rule", so the identifier in force is the one that service answered the authorize
        // with and is what the revoke on the way out names the rule by.
        || resource_type == EC2_SECURITY_GROUP_INGRESS
        || resource_type == EC2_SECURITY_GROUP_EGRESS
        // A LAUNCH TEMPLATE is one more: `CreateLaunchTemplate` takes the template's NAME and
        // no identifier — Amazon EC2 assigns the `lt-` id — and "Ref returns the launch
        // template ID", so the identifier in force is the one that service answered the
        // create with, while the NAME a declaration states is what the template is created
        // under.
        || resource_type == EC2_LAUNCH_TEMPLATE
        // An INSTANCE is a fifth: `RunInstances` takes no identifier either — the declaration
        // states the AMI, the instance type and where the instance goes — Amazon EC2 assigns
        // the `i-` id, and "Ref returns the ID of the instance", so the identifier in force is
        // the one that service answered the launch with.
        || resource_type == EC2_INSTANCE
        // A SPOT FLEET REQUEST is a sixth: `RequestSpotFleet` takes no identifier either — the
        // declaration states the configuration the fleet is to hold capacity under — Amazon EC2
        // assigns the `sfr-` id, and "Ref returns the Spot Fleet request ID", so the identifier
        // in force is the one that service answered the request with.
        || resource_type == EC2_SPOT_FLEET
        // An Amazon SES receipt rule is one more, for the same reason read from the other
        // side: "Ref returns the receipt rule name", and a declaration states that name
        // inside the type's `Rule` property rather than through a name property of its own,
        // so the naming rules here cannot reach it. The create is what answers the name the
        // rule was created under, and the record keeps it — a re-deploy of the same template
        // then sees the rule the stack already has rather than one to create again.
        || resource_type == SES_RECEIPT_RULE
        // An AWS X-Ray sampling rule is one more, and in exactly that position: the rule's
        // identity is the `RuleARN` `CreateSamplingRule` minted, which a declaration never
        // states, and the NAME a declaration states lives inside that type's own
        // `SamplingRule` property rather than in a name property the naming rules here
        // could read. So the identifier in force is the one X-Ray answered with, and a
        // re-deploy of the same template carries it forward rather than replacing the rule.
        || resource_type == XRAY_SAMPLING_RULE
        // An AWS X-Ray GROUP is one more: "Ref returns the group ARN", and the ARN
        // `CreateGroup` minted ends in a value X-Ray draws per group beside the group's
        // name, so no naming rule here could compute it. The NAME a declaration states is
        // only what the group is created under, which is why it stays the type's name
        // property while the identifier in force is the one that service answered with.
        || resource_type == XRAY_GROUP
        // An AWS CodeBuild COMPUTE FLEET is one more: "Ref returns the fleet ARN", and that
        // ARN carries the `<name>:<id>` identifier CodeBuild draws per fleet (`Fleet$id`),
        // which no naming rule here could compute. The NAME a declaration states is only what
        // the fleet is created under, which is why it stays the type's name property while the
        // identifier in force is the one that service answered with — the position
        // [`XRAY_GROUP`] is in.
        || resource_type == crate::state::provisioner::codebuild_fleets::CODEBUILD_FLEET
        // A CodeArtifact PACKAGE GROUP is one more: "Ref returns the Amazon Resource Name
        // (ARN) of the package group", and that ARN's resource path carries the group's
        // PATTERN with every character an ARN does not hold literally escaped — a pattern
        // admits wildcards and sigils (`PackageGroupPattern` is only "no control or
        // whitespace characters"), so no naming rule here could compute it. The pattern the
        // declaration states is what the group is CREATED under and what every CodeArtifact
        // call on the group takes, while the identifier in force is the ARN that service
        // answered with — the position [`CODEBUILD_FLEET`] is in. (The DOMAIN and the
        // REPOSITORY are NOT: each of their ARNs is the plain `<type>/<name>` path the naming
        // rules compute.)
        || resource_type
            == crate::state::provisioner::codeartifact_package_groups::CODEARTIFACT_PACKAGE_GROUP
        // An Amazon Cognito identity pool is one more: `CreateIdentityPool` takes only the
        // pool's NAME — "a string that you provide" — and answers the `IdentityPoolId` it
        // minted (modelled `@pattern ^[\w-]+:[0-9a-f-]+$`, the `REGION:GUID` form), and "Ref
        // returns the Amazon Cognito identity pool ID", which is the handle every
        // cognito-identity operation takes (`GetId`, `DescribeIdentityPool`,
        // `SetIdentityPoolRoles`). No naming rule could compute that GUID, so the identifier
        // in force is the one that service answered. The pool's own NAME is its `Name`
        // attribute, which the create reports.
        || resource_type == COGNITO_IDENTITY_POOL
        // An Amazon Cognito USER POOL and an app client of one are two more:
        // `CreateUserPool` takes only the pool's `PoolName` and answers the `Id` it minted
        // (`UserPoolIdType`, `@pattern ^[\w-]+_[0-9a-zA-Z]+$` — the `<region>_<id>` form),
        // and `CreateUserPoolClient` takes the client's `ClientName` and answers the
        // `ClientId` it assigned (`ClientIdType`, `@pattern ^[\w+]+$`). Each type's `Ref`
        // returns that identifier, and it is the handle every cognito-idp operation on the
        // resource takes (`DescribeUserPool$UserPoolId`,
        // `DescribeUserPoolClient$ClientId`, `AdminCreateUser$UserPoolId`) — a pattern no
        // name CloudFormation decides satisfies — so the identifier in force is the one that
        // service answered, and the name is only what the create was made under
        // ([`crate::state::provisioning::cognito_user_pool`]).
        || resource_type == COGNITO_USER_POOL
        || resource_type == COGNITO_USER_POOL_CLIENT
        // The four AWS AppConfig types are four more, for the reason their own constants
        // record ([`APPCONFIG_APPLICATION`] and its siblings): every one of those creates
        // takes the resource's NAME and no identifier, and answers the `Id` AppConfig minted
        // (`^[a-z0-9]{4,7}$`, which no name CloudFormation decides satisfies), and that id is
        // what every later call on the resource takes.
        || resource_type == APPCONFIG_APPLICATION
        || resource_type == APPCONFIG_ENVIRONMENT
        || resource_type == APPCONFIG_CONFIGURATION_PROFILE
        || resource_type == APPCONFIG_DEPLOYMENT_STRATEGY
        // The two AppConfig ROLLOUT types are two more, for the reason their own constants
        // record ([`APPCONFIG_HOSTED_CONFIGURATION_VERSION`] and its sibling):
        // `CreateHostedConfigurationVersion` takes the content and answers the `VersionNumber`
        // AppConfig minted, `StartDeployment` takes no identifier and answers the
        // `DeploymentNumber` it allocated, and each type is identified by the triple its own API
        // addresses one resource by — of which that number is a part no naming rule could
        // compute ([`appconfig_rollout_identity`]).
        || resource_type == APPCONFIG_HOSTED_CONFIGURATION_VERSION
        || resource_type == APPCONFIG_DEPLOYMENT
        // A Secrets Manager secret is one more: `CreateSecret` takes the secret's NAME and
        // answers the `ARN` that service built for it, whose six trailing characters it draws
        // per secret ([`SECRETSMANAGER_SECRET`]) — so no naming rule could compute it — and
        // "Ref returns the ARN of the secret", which is the handle every Secrets Manager call
        // on one takes. The ARN computed here is the stand-in a declaration is resolved
        // against until that create has answered, which the deployment then re-points.
        || resource_type == SECRETSMANAGER_SECRET
        // Its ROTATION is one more, and for the same reason: the resource is the rotation
        // configured on a secret, "Ref returns the ARN of the secret", and that ARN is the
        // one `RotateSecret` answered rather than a value computed here — so the identifier
        // in force is carried forward and a later deployment applies its declaration to the
        // rotation that exists ([`SECRETSMANAGER_ROTATION_SCHEDULE`]).
        || resource_type == SECRETSMANAGER_ROTATION_SCHEDULE
        // So is its RESOURCE POLICY, for the same reason: the resource is the policy
        // attached to a secret, the identifier it is known by is that secret's ARN as
        // `PutResourcePolicy` answered it, and no naming rule here can compute the six
        // characters that ARN ends in ([`SECRETSMANAGER_RESOURCE_POLICY`]).
        || resource_type == SECRETSMANAGER_RESOURCE_POLICY
        // A NESTED STACK is one more, and the clearest case of it: "Ref returns the stack
        // ID" of the stack CloudFormation created for the resource, which carries the unique
        // id that service minted for it — the name the naming rules compute is only what the
        // create was made under, exactly as a security group's is
        // (`crate::state::provisioning`'s nested-stack provider). That stack id is also what
        // `DescribeStackResource` reports as the resource's `PhysicalResourceId`, and the
        // handle every call on the child stack takes.
        || resource_type == crate::state::STACK_RESOURCE_TYPE
        // The four Elastic Load Balancing v2 types are four more: `CreateLoadBalancer`,
        // `CreateTargetGroup`, `CreateListener` and `CreateRule` take no identifier at all,
        // each answers an ARN carrying a 16-character identifier that service MINTED, and
        // every one of those types' `Ref` returns that ARN — the `Name` a load balancer or a
        // target group declares is only what the resource is called, and a listener and a rule
        // declare no name at all. So the identity in force is the one the create answered
        // ([`crate::state::provisioning::elastic_load_balancing`]), and the ARN computed here
        // is the stand-in a declaration is resolved against until then.
        || resource_type == ELB_LOAD_BALANCER
        || resource_type == ELB_TARGET_GROUP
        || resource_type == ELB_LISTENER
        || resource_type == ELB_LISTENER_RULE
        // A TRUST STORE is a fifth: `CreateTrustStore` takes the store's NAME and no
        // identifier, and answers an ARN carrying the same 16-character identifier that
        // service mints (`truststore/<name>/<id>`), which "Ref returns" — so the identity in
        // force is the one the create answered and the ARN computed here is the stand-in a
        // declaration (an HTTPS listener's `MutualAuthentication.TrustStoreArn`) is resolved
        // against until then.
        || resource_type == ELB_TRUST_STORE
        // An Amazon ECS task-definition REVISION is one more: `RegisterTaskDefinition` takes
        // the FAMILY and no identifier, and Amazon ECS allocates the revision — "the revision
        // is a version number of a task definition in a family", incremented per registration
        // — so the `family:revision` ARN "Ref returns the Amazon Resource Name (ARN) of the
        // task definition" publishes cannot be computed from the template: a family the
        // account already holds revisions of is registered at the next number, not at the
        // first. That ARN is also the `taskDefinition` handle every call on one takes
        // (`DescribeTaskDefinition`, `RunTask`, an EventBridge target's
        // `EcsParameters.TaskDefinitionArn`), so the identity in force is the one that service
        // answered ([`crate::state::provisioning`]'s task-definition provider), and the ARN
        // computed here is the stand-in a declaration is resolved against until then.
        || resource_type == ECS_TASK_DEFINITION
        // An AWS Batch job-definition REVISION is in exactly that position:
        // `RegisterJobDefinition` takes the definition's NAME and no identifier, and AWS
        // Batch allocates the revision — one past the highest revision the account holds
        // under that name — so the `<name>:<revision>` ARN "Ref returns the job definition
        // ARN" publishes cannot be computed from the template: a name the account already
        // holds revisions of is registered at the next number, not at the first. That ARN is
        // also the `jobDefinition` handle every call on one takes (`SubmitJob`,
        // `DescribeJobDefinitions`, `DeregisterJobDefinition`), so the identity in force is
        // the one that service answered
        // ([`crate::state::provisioner`]'s job-definition arms), and the ARN computed here is
        // the stand-in a declaration is resolved against until then.
        || resource_type == BATCH_JOB_DEFINITION
        // Every one of the three Amazon Lex V2 types is one too: that service mints a bot's
        // `botId` and an alias's `botAliasId`, and it assigns the NUMBER a version is cut
        // under, so none of those identities is a value a template could state and the
        // identifiers the naming rules compute for one are stand-ins until the create has
        // answered ([`crate::state::provisioner::lex_bots`]).
        || lex_bots::kind_of(resource_type).is_some()
}

/// Whether `resource_type` is one of the six Amazon Bedrock AgentCore types whose identity is
/// the one that control plane MINTED, which is what
/// [`identity_is_service_assigned`] names them for.
///
/// One reading of the six, so the question has one answer: the registry of provisioned types
/// keys its entries by the same names ([`crate::state::provisioner`]).
fn agentcore_identity_is_assigned(resource_type: &str) -> bool {
    matches!(
        resource_type,
        crate::state::provisioner::BEDROCK_AGENTCORE_RUNTIME
            | crate::state::provisioner::BEDROCK_AGENTCORE_MEMORY
            | crate::state::provisioner::BEDROCK_AGENTCORE_GATEWAY
            | crate::state::provisioner::BEDROCK_AGENTCORE_GATEWAY_TARGET
            | crate::state::provisioner::BEDROCK_AGENTCORE_CODE_INTERPRETER
            | crate::state::provisioner::BEDROCK_AGENTCORE_BROWSER
    )
}

/// Whether the physical identifier of a resource of `resource_type` is an ADDRESS of the
/// endpoint this emulator is served at, so the value in force is the one the owning service
/// REPORTED and the value the naming rules compute is only another spelling of it.
///
/// One type is in that position: an `AWS::SQS::Queue`, whose "Ref returns the queue URL". A
/// queue url is the only handle every queue-addressed call takes and a client sends the string
/// it was handed straight back, so Amazon SQS reports a queue at the endpoint the call REACHED
/// rather than at that service's published regional host — and which endpoint that is depends
/// on where this emulator is served, which no template states.
///
/// What the two spellings share is the `/<accountId>/<QueueName>` PATH that names the queue, so
/// such an identifier is compared by that path
/// ([`crate::state::provisioning::is_same_resource`]) and a record matched under the other
/// spelling keeps the identifier its owning service answered with ([`crate::state::stack`]'s
/// identity pass). Without that, a re-deploy of the very same template reads as asking for a
/// DIFFERENT queue: the stack creates a second one and takes the queue every reference resolves
/// to down with it.
pub fn identity_is_a_reported_address(resource_type: &str) -> bool {
    resource_type == SQS_QUEUE
}

/// The separator CloudFormation joins the parts of a COMPOSITE physical resource id with.
///
/// Public so a reader of such an identity — a test asserting the triple a resource is known by —
/// reads the separator this service really joins with rather than a second literal of its own.
pub const COMPOSITE_IDENTITY_SEPARATOR: &str = "|";

/// The properties whose values ARE a resource's physical resource id, in the order they are
/// joined in, for a type whose primary identifier is COMPOSITE; the empty list for every
/// type a single value identifies.
///
/// Such a type declares no resource of its own: it is a binding BETWEEN resources, and what
/// identifies a binding is what it binds — so the physical resource id of one, which is
/// what `Ref` returns, is those property values joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`]:
///
/// * `AWS::WAFv2::WebACLAssociation` is `<ResourceArn>|<WebACLArn>`, the pair
///   `AssociateWebACL` takes and `DisassociateWebACL` removes;
/// * `AWS::EKS::AccessEntry` is `<ClusterName>|<PrincipalArn>`, the pair every Amazon EKS
///   operation on an access entry addresses one by;
/// * `AWS::Cognito::IdentityPoolPrincipalTag` is `<IdentityPoolId>|<IdentityProviderName>`,
///   the pair `SetPrincipalTagAttributeMap` and `GetPrincipalTagAttributeMap` address one
///   provider's attribute map by;
/// * `AWS::ACMPCA::Permission` is `<CertificateAuthorityArn>|<Principal>`, the pair
///   `DeletePermission` takes and `ListPermissions` reports a grant under.
///
/// A type whose resource is a sub-resource its owning API addresses by SEVERAL values is the
/// same case: an `AWS::Glue::TableOptimizer` is
/// `<CatalogId>|<DatabaseName>|<TableName>|<Type>`, the four `CreateTableOptimizer`,
/// `GetTableOptimizer` and `DeleteTableOptimizer` each take to address one optimizer, and the
/// type declares no name of its own. An `AWS::S3Tables::Namespace` is
/// `<TableBucketARN>|<Namespace>` for that reason too: `GetNamespace` and `DeleteNamespace`
/// bind both as the labels of `/namespaces/{tableBucketARN}/{namespace}`. An
/// `AWS::CodePipeline::CustomActionType` is `<Category>|<Provider>|<Version>`, the triple
/// `DeleteCustomActionType` addresses a registration by and the one `ListActionTypes` reports
/// each custom action type under, the type declaring no name of its own
/// ([`CODEPIPELINE_CUSTOM_ACTION_TYPE`]).
///
/// Because the identity IS the pair, a declaration that re-points either side of it names a
/// DIFFERENT resource: the deployment replaces the one the stack has
/// ([`super::provisioning::is_same_resource`]) and the new `Ref` names the new pair, and the
/// record the stack's own deletion acts on carries exactly the pair to unbind. Every part is
/// a create-only property of its type for that same reason
/// ([`replacement_properties`]).
///
/// A type whose primary identifier is composite for ANOTHER reason is here too: the three
/// Amplify UI Builder entities are identified by the app, the environment and the id their own
/// resource shape names ([`AMPLIFYUIBUILDER_COMPONENT`]), and the last of those is the one
/// Amplify UI Builder MINTS. So the row says what a client names one of these resources by
/// ([`crate::state::primary_identifier`]), while [`composite_identity`] answers `None` for one
/// — the declaration carries no `Id` to join — and the identity is built from the entity that
/// service answered instead ([`amplifyuibuilder_identity`]). The four WAFv2 ENTITY types are
/// the same case ([`WAFV2_ENTITY_IDENTITY`]): each is addressed by its name, the `Id` AWS WAF
/// assigned and the scope it lives in.
pub fn composite_identity_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        WAFV2_WEB_ACL_ASSOCIATION => &["ResourceArn", "WebACLArn"],
        EKS_ACCESS_ENTRY => &["ClusterName", "PrincipalArn"],
        COGNITO_IDENTITY_POOL_PRINCIPAL_TAG => &["IdentityPoolId", "IdentityProviderName"],
        ACMPCA_PERMISSION => &["CertificateAuthorityArn", "Principal"],
        GLUE_TABLE_OPTIMIZER => &["CatalogId", "DatabaseName", "TableName", "Type"],
        // An `AWS::LakeFormation::DataCellsFilter` is
        // `<TableCatalogId>|<DatabaseName>|<TableName>|<Name>`: a filter is a resource OF a
        // catalog table, which `CreateDataCellsFilter`, `GetDataCellsFilter` and
        // `DeleteDataCellsFilter` each address by those four together, and Lake Formation
        // mints no identifier for one.
        crate::state::provisioner::lake_formation_data_cells_filters::LAKEFORMATION_DATA_CELLS_FILTER => {
            crate::state::provisioner::lake_formation_data_cells_filters::DATA_CELLS_FILTER_ADDRESS
        }
        // An `AWS::EC2::ClientVpnAuthorizationRule` is `<ClientVpnEndpointId>|<TargetNetworkCidr>`:
        // Amazon EC2 mints no identifier for an authorization rule, every call on one naming
        // it by the endpoint it is on and the network it grants access to, and the type
        // declares no name of its own.
        crate::state::provisioner::client_vpn::EC2_CLIENT_VPN_AUTHORIZATION_RULE => {
            crate::state::provisioner::client_vpn::EC2_CLIENT_VPN_AUTHORIZATION_RULE_IDENTITY
        }
        CODEPIPELINE_CUSTOM_ACTION_TYPE => CODEPIPELINE_ACTION_TYPE_IDENTITY,
        S3TABLES_NAMESPACE => S3TABLES_NAMESPACE_IDENTITY,
        // An `AWS::EMR::StudioSessionMapping` is `<StudioId>|<IdentityType>|<IdentityName>`,
        // the triple `CreateStudioSessionMapping`, `GetStudioSessionMapping` and
        // `DeleteStudioSessionMapping` each address one mapping by — Amazon EMR mints no
        // identifier for one, and the type declares no name of its own.
        crate::state::provisioner::emr_resources::EMR_STUDIO_SESSION_MAPPING => {
            crate::state::provisioner::emr_resources::EMR_STUDIO_SESSION_MAPPING_IDENTITY
        }
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_FORM | AMPLIFYUIBUILDER_THEME => {
            AMPLIFYUIBUILDER_ATTRIBUTES
        }
        // The two AWS AppConfig ROLLOUT types are in that same position: a hosted configuration
        // version is identified by the application, the configuration profile and the
        // `VersionNumber` AppConfig minted, and a deployment by the application, the environment
        // and the `DeploymentNumber` it allocated — the three labels each of those APIs addresses
        // one resource by. The last part of each is that service's to mint, so
        // [`composite_identity`] answers `None` for one (the declaration carries no number to
        // join) and the identity is built from what the create answered instead
        // ([`appconfig_rollout_identity`]).
        APPCONFIG_HOSTED_CONFIGURATION_VERSION => &[
            "ApplicationId",
            "ConfigurationProfileId",
            APPCONFIG_VERSION_NUMBER_ATTRIBUTE,
        ],
        APPCONFIG_DEPLOYMENT => &["ApplicationId", "EnvironmentId", "DeploymentNumber"],
        // An Application Signals INSTRUMENTATION CONFIG is in that same position: a
        // configuration is addressed by the instrumentation type, the service, the environment,
        // the signal type and the `LocationHash` that service computed — the five values
        // `GetInstrumentationConfiguration` and `DeleteInstrumentationConfiguration` each take
        // to name one — and the type declares no name of its own. The last part is that
        // service's to compute, so [`composite_identity`] answers `None` for one (the
        // declaration carries no hash to join) and the identity is built from what the create
        // answered instead ([`instrumentation_config_identity`]).
        APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG => {
            APPLICATION_SIGNALS_INSTRUMENTATION_CONFIG_IDENTITY
        }
        // The three Amazon ECS types whose resource is not a named entity of its own but a
        // BINDING, each identified by the values that service's own API addresses one by: the
        // capacity-provider configuration of a cluster by that cluster (there is one per
        // cluster, and `PutClusterCapacityProviders` takes nothing else to address it), the
        // primary task set of a service by the cluster and the service whose primary task set
        // it is, and a TASK SET by the cluster, the service and the `id` Amazon ECS minted —
        // the last of which that service mints, so [`composite_identity`] answers `None` for
        // one (the declaration carries no `Id` to join) and the identity is built from what
        // the create answered instead
        // ([`crate::state::provisioner::ecs_task_sets::ecs_task_set_identity`]).
        crate::state::provisioner::ecs_cluster_capacity_providers::ECS_CLUSTER_CAPACITY_PROVIDER_ASSOCIATIONS => {
            crate::state::provisioner::ecs_cluster_capacity_providers::ECS_CLUSTER_CAPACITY_PROVIDER_ASSOCIATIONS_IDENTITY
        }
        crate::state::provisioner::ecs_task_sets::ECS_PRIMARY_TASK_SET => {
            crate::state::provisioner::ecs_task_sets::ECS_PRIMARY_TASK_SET_IDENTITY
        }
        crate::state::provisioner::ecs_task_sets::ECS_TASK_SET => {
            crate::state::provisioner::ecs_task_sets::ECS_TASK_SET_IDENTITY
        }
        // The two MEMBERS of a SageMaker AI Studio domain: a user profile is
        // `<UserProfileName>|<DomainId>` and a space `<DomainId>|<SpaceName>`, the pair
        // `CreateUserProfile`/`CreateSpace`, their reads and their deletes each take to address
        // one — SageMaker mints no identifier for either, and the member's own name identifies
        // it only within the domain that holds it, so two domains may hold the same name. Each
        // type's own order is the one its `Ref` reports the pair in, which is the order its
        // schema states the two parts of its primary identifier in: the profile's own name
        // leads, the space's domain does.
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_USER_PROFILE => {
            &["UserProfileName", "DomainId"]
        }
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_SPACE => {
            &["DomainId", "SpaceName"]
        }
        // The two Amazon Lex V2 resources that live INSIDE a bot, each identified by the pair
        // its own schema declares as its primary identifier: a version by the bot and the
        // number Amazon Lex assigned it, an alias by its own id and the bot it is an alias of.
        // The half that service assigns is no part of any declaration, so
        // [`composite_identity`] answers `None` for one and the identity is built from what
        // the create answered instead
        // ([`crate::state::provisioner::lex_bots::bot_version_identity`]).
        lex_bots::LEX_BOT_VERSION => &["BotId", lex_bots::LEX_BOT_VERSION_ATTRIBUTE],
        lex_bots::LEX_BOT_ALIAS => &["BotAliasId", "BotId"],
        // The four WAFv2 ENTITY types are in that position too: a web ACL, a rule group, an IP
        // set and a regex pattern set are each addressed by the triple their own schema declares
        // as the primary identifier — the name, the `Id` WAF assigned and the scope the entity
        // lives in ([`WAFV2_ENTITY_IDENTITY`]) — and the middle part is that service's to mint,
        // so [`composite_identity`] answers `None` for one (the declaration carries no `Id` to
        // join) and the identity is built from what the create answered instead
        // ([`wafv2_entity_identity`]).
        WAFV2_WEB_ACL | WAFV2_RULE_GROUP | WAFV2_IP_SET | WAFV2_REGEX_PATTERN_SET => {
            WAFV2_ENTITY_IDENTITY
        }
        _ => &[],
    }
}

/// The physical resource id a type's composite primary identifier makes of one RESOLVED
/// property document ([`composite_identity_properties`]).
///
/// `None` for a type that has no such identifier, and for a declaration that leaves one of
/// its parts out or states it as something other than a scalar: that declaration names no
/// pair at all, so the resource keeps the identity the naming rules computed and the owning
/// service's own validation of the missing member is what judges it.
pub fn composite_identity(resource_type: &str, properties: &Value) -> Option<String> {
    // The two Amazon EC2 bindings whose identifier is a composition of their parts rather
    // than the plain join of a fixed property list.
    match resource_type {
        EC2_ROUTE => return ec2_route_identity(properties),
        EC2_VPC_GATEWAY_ATTACHMENT => return ec2_gateway_attachment_identity(properties),
        _ => {}
    }
    let parts = composite_identity_properties(resource_type);
    if parts.is_empty() {
        return None;
    }
    let properties = properties.as_object()?;
    let values: Option<Vec<String>> = parts
        .iter()
        .map(|part| properties.get(*part).and_then(scalar_text))
        .collect();
    Some(values?.join(COMPOSITE_IDENTITY_SEPARATOR))
}

/// `AWS::EC2::Route`: ONE route of one route table. "Ref returns the ID of the resource",
/// which for this type is the pair that identifies a route — Amazon EC2 mints no identifier
/// for one, every call on a route addressing it by the table it is in and the destination it
/// matches on ([`ec2_route_identity`]).
pub const EC2_ROUTE: &str = "AWS::EC2::Route";

/// `AWS::EC2::VPCGatewayAttachment`: the attachment of ONE gateway to ONE VPC. "Ref returns
/// the name of the VPC gateway attachment", which is that pair for the same reason
/// ([`ec2_gateway_attachment_identity`]).
pub const EC2_VPC_GATEWAY_ATTACHMENT: &str = "AWS::EC2::VPCGatewayAttachment";

/// The properties of `AWS::EC2::Route` its identity and its one published attribute are read
/// off: the table the route is in, and the three a route may state its ONE destination in.
pub(super) const EC2_ROUTE_IDENTITY_MEMBERS: &[&str] = &[
    "RouteTableId",
    "DestinationCidrBlock",
    "DestinationIpv6CidrBlock",
    "DestinationPrefixListId",
];

/// The properties of `AWS::EC2::VPCGatewayAttachment` its identity is read off: the VPC, and
/// the two an attachment may name its ONE gateway in.
pub(super) const EC2_ATTACHMENT_IDENTITY_MEMBERS: &[&str] =
    &["VpcId", "InternetGatewayId", "VpnGatewayId"];

/// The ONE destination an `AWS::EC2::Route` declaration matches on, which is also the type's
/// one published attribute: "CidrBlock — the CIDR block used for the destination match".
///
/// A route matches on exactly one of the three destination properties, so the one it states is
/// the destination; `None` for a declaration that states none, which identifies no route at
/// all and is what the owning service's own validation refuses.
pub fn ec2_route_destination(properties: &Value) -> Option<String> {
    EC2_ROUTE_IDENTITY_MEMBERS
        .iter()
        .skip(1)
        .find_map(|member| properties.get(*member).and_then(scalar_text))
}

/// The physical resource id of an `AWS::EC2::Route`: `<RouteTableId>|<destination>`, the pair
/// every Amazon EC2 call on a route addresses one by, joined the way CloudFormation joins the
/// parts of a composite identifier ([`COMPOSITE_IDENTITY_SEPARATOR`]).
///
/// `None` for a declaration that does not state both halves: such a declaration names no route,
/// so the resource keeps the identity the naming rules computed and that service's own refusal
/// of the create is what judges it.
pub fn ec2_route_identity(properties: &Value) -> Option<String> {
    let route_table_id = properties.get("RouteTableId").and_then(scalar_text)?;
    let destination = ec2_route_destination(properties)?;
    Some(format!(
        "{route_table_id}{COMPOSITE_IDENTITY_SEPARATOR}{destination}"
    ))
}

/// The KIND of gateway a `AWS::EC2::VPCGatewayAttachment` puts on its VPC, as the physical
/// resource id of the attachment names it: an internet gateway and a virtual private gateway
/// are attached by calls of their own, and the attachment's identifier says which of the two
/// it is.
const EC2_INTERNET_GATEWAY_ATTACHMENT: &str = "IGW";
const EC2_VPN_GATEWAY_ATTACHMENT: &str = "VGW";

/// The physical resource id of an `AWS::EC2::VPCGatewayAttachment`: the kind of gateway
/// attached and the VPC it is attached to (`IGW|vpc-…`), the pair both detaches address the
/// attachment by, joined the way every other composite identifier is.
///
/// `None` for a declaration that names no VPC, or that names neither gateway or both: one
/// attachment attaches ONE gateway, so such a declaration names no attachment and the owning
/// service's own refusal is what judges it.
pub fn ec2_gateway_attachment_identity(properties: &Value) -> Option<String> {
    let vpc_id = properties.get("VpcId").and_then(scalar_text)?;
    let internet_gateway = properties
        .get("InternetGatewayId")
        .and_then(scalar_text)
        .is_some();
    let vpn_gateway = properties
        .get("VpnGatewayId")
        .and_then(scalar_text)
        .is_some();
    let kind = match (internet_gateway, vpn_gateway) {
        (true, false) => EC2_INTERNET_GATEWAY_ATTACHMENT,
        (false, true) => EC2_VPN_GATEWAY_ATTACHMENT,
        _ => return None,
    };
    Some(format!("{kind}{COMPOSITE_IDENTITY_SEPARATOR}{vpc_id}"))
}

/// The members `names` of one declaration, each EVALUATED: the document the identity rules
/// above read, built where the declaration is still the template's own text.
///
/// A member that cannot be evaluated — a reference to a resource this scope does not carry —
/// is left out, exactly as [`resolve_properties`] leaves it out of the document the owning
/// service is asked with.
pub(super) fn resolved_members(
    properties: &Value,
    names: &[&str],
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Value {
    let mut resolved = serde_json::Map::new();
    for name in names {
        if let Some(value) = properties
            .get(*name)
            .and_then(|declared| text(declared, scope, 0, budget))
        {
            resolved.insert((*name).to_string(), Value::String(value));
        }
    }
    Value::Object(resolved)
}

/// Give a resource the physical id its type's composite primary identifier makes it, where
/// its type has one.
///
/// A composite identity is a function of the resource's RESOLVED properties — the other
/// resources the binding names resolve to their own identifiers first — so this is applied
/// where those are filled in ([`resolve_properties`]) rather than where the naming rules run,
/// and AGAIN once a part naming a resource whose identity its owning service mints has been
/// re-pointed at the identifier that service really assigned
/// ([`crate::state::provisioning::repointing`]).
/// A type with no composite identifier is left exactly as it is.
pub(in crate::state) fn adopt_composite_identity(resource: &mut StoredResource) {
    let Some(identity) = composite_identity(&resource.resource_type, &resource.properties) else {
        return;
    };
    adopt_identity(resource, identity);
}

/// The properties that SCOPE a resource type's physical name: the parent resource its
/// owning service addresses one of these resources INSIDE, so that the name identifies one
/// resource only together with them; the empty list for a type whose name identifies a
/// resource on its own.
///
/// A physical name identifies one resource — which is why a second resource cannot be
/// created under a name a stack already holds ([`crate::state::physical_name`]) — but for a
/// CHILD resource that name is unique only within its parent: every call of the owning
/// service that addresses one takes the parent beside the name, so two declarations stating
/// the same name under DIFFERENT parents describe two resources, and AWS creates both.
///
/// Source for each row: the members the owning API's own calls take to address one resource
/// of the type.
///
/// * a log stream lives IN a log group (`CreateLogStream`, `DeleteLogStream` and
///   `PutLogEvents` each take `logGroupName` beside `logStreamName`, and
///   `DescribeLogStreams` lists the streams of one group);
/// * a metric filter and a subscription filter are each put ON a log group
///   (`PutMetricFilter`/`DeleteMetricFilter` and
///   `PutSubscriptionFilter`/`DeleteSubscriptionFilter` take `logGroupName` beside
///   `filterName`, and the two describes list the filters of one group);
/// * a stage belongs to ONE REST API (`GetStage` and `DeleteStage` take `restApiId` beside
///   `stageName`);
/// * a stage of an HTTP or WebSocket API belongs to ONE of those APIs the same way
///   (API Gateway v2's `GetStage` and `DeleteStage` take `ApiId` beside `StageName`, both
///   `@httpLabel`s of `/v2/apis/{ApiId}/stages/{StageName}`), and "Ref returns the stage
///   name" for that type too — so two apis of one stack each carrying the `$default` stage
///   every auto-deployed HTTP api is served on are two stages, and AWS creates both;
/// * a model belongs to ONE REST API (`GetModel` and `DeleteModel` take `restApiId` beside
///   `modelName`, and a method's `requestModels` names one by that name);
/// * a lifecycle hook and a scheduled action each belong to ONE Auto Scaling group
///   (`PutLifecycleHook` and `DeleteLifecycleHook` take `AutoScalingGroupName` beside
///   `LifecycleHookName`, `PutScheduledUpdateGroupAction` and `DeleteScheduledAction` take it
///   beside `ScheduledActionName`, and each type's `Ref` returns the child's own name) — so
///   two groups of one stack each carrying a hook of one name are two hooks, and AWS creates
///   both.
///
/// A type whose physical id its owning service ASSIGNS needs no row: that identifier is
/// unique by construction, so two resources of the type never resolve to one name.
pub fn name_scope_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        crate::state::provisioning::logs_resources::LOGS_LOG_STREAM
        | crate::state::provisioning::logs_resources::LOGS_METRIC_FILTER
        | crate::state::provisioning::logs_resources::LOGS_SUBSCRIPTION_FILTER => {
            &["LogGroupName"]
        }
        "AWS::ApiGateway::Stage" => &["RestApiId"],
        APIGATEWAY_MODEL => &["RestApiId"],
        APIGATEWAYV2_STAGE => &["ApiId"],
        crate::state::provisioner::auto_scaling_group_children::AUTOSCALING_LIFECYCLE_HOOK
        | crate::state::provisioner::auto_scaling_group_children::AUTOSCALING_SCHEDULED_ACTION => {
            &["AutoScalingGroupName"]
        }
        // An archive rule's name identifies it WITHIN one analyzer: every call of IAM Access
        // Analyzer on a rule binds the analyzer as its own URI label beside the rule's name, so
        // two analyzers each carrying a rule of one name carry two rules.
        access_analyzer::ACCESSANALYZER_ARCHIVE_RULE => {
            &[access_analyzer::ANALYZER_NAME_PROPERTY]
        }
        _ => &[],
    }
}

/// The values one resource's declaration states for the properties that scope its name
/// ([`name_scope_properties`]), in that order.
///
/// A property the declaration does not state, or states as something other than a scalar,
/// contributes the empty string: two declarations that both leave the parent out are in the
/// same (unnamed) scope, which is what comparing them says.
pub fn name_scope(resource_type: &str, properties: &Value) -> Vec<String> {
    name_scope_properties(resource_type)
        .iter()
        .map(|property| {
            properties
                .get(*property)
                .and_then(scalar_text)
                .unwrap_or_default()
        })
        .collect()
}

/// The three AWS Backup resource types a stack creates in that service (the AWS Backup
/// model: `cloudFormationName: Backup`).
///
/// A VAULT is keyed by the name the template states: "Ref returns the backup vault name",
/// which is the `@httpLabel` `CreateBackupVault`, `DescribeBackupVault` and
/// `DeleteBackupVault` all address one by, so its physical id is that name
/// ([`naming_rules`] holds a generated one to the name form that service publishes).
///
/// A PLAN's and a SELECTION's identifiers are AWS Backup's OWN to mint — `Ref` of a plan
/// returns the `BackupPlanId` `CreateBackupPlan` answered with, and `CreateBackupSelection`
/// answers the `SelectionId` it assigned, neither create taking an identifier — so what a
/// stack reports for one is the id that service answered with, which is what
/// `GetBackupPlan`, `ListBackupSelections` and `GetBackupSelection` resolve. That is what
/// [`identity_is_service_assigned`] names both types for.
pub const BACKUP_VAULT: &str = "AWS::Backup::BackupVault";
pub const BACKUP_PLAN: &str = "AWS::Backup::BackupPlan";
pub const BACKUP_SELECTION: &str = "AWS::Backup::BackupSelection";

/// AWS Backup's ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`backup`) — the service segment of a backup vault ARN.
pub const BACKUP_ARN_NAMESPACE: &str = "backup";

/// An Amazon Managed Grafana workspace's resource type (that service's model:
/// `cloudFormationName: Grafana`). This is a type `super::provisioning` creates in that
/// service itself, and `Ref` of the type returns the workspace id — a `^g-[0-9a-f]{10}$`
/// value only Amazon Managed Grafana mints — so the id a stack's output reports for it is
/// the one that service answered with.
pub const GRAFANA_WORKSPACE: &str = "AWS::Grafana::Workspace";

/// The three AWS Glue Data Catalog and crawler resource types a stack declares beside its
/// job definitions (that service's model: `cloudFormationName: Glue`). Each is a type
/// `super::provisioning` creates in AWS Glue itself, and each one's `Ref` returns the NAME
/// the declaration states — a database's and a table's inside the input structure the
/// owning API takes it in ([`declared_name_path`]), a crawler's as its own `Name` property
/// — which is the identifier every AWS Glue operation on one of them addresses it by, so
/// none of the three has a service-assigned identity.
pub const GLUE_DATABASE: &str = "AWS::Glue::Database";
pub const GLUE_TABLE: &str = "AWS::Glue::Table";
pub const GLUE_CRAWLER: &str = "AWS::Glue::Crawler";

/// The OPTIMIZER of one of those tables (the same `cloudFormationName: Glue`). This is a
/// type `super::provisioner` creates in AWS Glue itself, and it declares no name of its own:
/// an optimizer is a resource OF a table, which `CreateTableOptimizer`,
/// `GetTableOptimizer` and `DeleteTableOptimizer` each address by the catalog, the database,
/// the table and the optimizer's own type together — so the four properties that name it ARE
/// its identity ([`composite_identity_properties`]), and the physical id of one is those
/// values joined, which is what `Ref` of the type returns.
pub const GLUE_TABLE_OPTIMIZER: &str = "AWS::Glue::TableOptimizer";

/// An Amazon GuardDuty detector's resource type (the GuardDuty model:
/// `cloudFormationName: GuardDuty`). This is a type `super::provisioning` creates in
/// GuardDuty itself, and "Ref returns the detector's id" — `CreateDetector` takes no
/// identifier and MINTS one — so the id a stack's output reports for it is the one that
/// service answered with.
pub const GUARDDUTY_DETECTOR: &str = "AWS::GuardDuty::Detector";

/// An Amazon GuardDuty trusted IP list's resource type (the same
/// `cloudFormationName: GuardDuty`). This is a type `super::provisioning` creates in
/// GuardDuty itself, and `Ref` returns the IPSet's id — `CreateIPSet` takes no identifier
/// and MINTS one, which is the `IpSetId` `GetIPSet`, `UpdateIPSet` and `DeleteIPSet`
/// address the list by — so the id a stack reports for the resource is the one that
/// service answered with. That is what [`identity_is_service_assigned`] names it for.
pub const GUARDDUTY_IP_SET: &str = "AWS::GuardDuty::IPSet";

/// The two Amazon RDS Proxy resource types a stack creates in that service (the Amazon RDS
/// model: `cloudFormationName: RDS`).
///
/// `AWS::RDS::DBProxy`'s `Ref` returns the `DBProxyName` the declaration states, which is
/// the identifier every Amazon RDS call on a proxy takes — so it has no service-assigned
/// identity. `AWS::RDS::DBProxyTargetGroup`'s does: a proxy comes up with the one target
/// group it has, and that type's `Ref` returns the `TargetGroupArn` Amazon RDS minted for
/// it, which no naming rule could compute — which is what
/// [`identity_is_service_assigned`] names it for.
pub const RDS_DB_PROXY: &str = "AWS::RDS::DBProxy";
pub const RDS_DB_PROXY_TARGET_GROUP: &str = "AWS::RDS::DBProxyTargetGroup";

/// An AWS X-Ray sampling rule's resource type (the X-Ray model:
/// `cloudFormationName: XRay`). This is a type `super::provisioning` creates in X-Ray
/// itself, and the type's identity is the `RuleARN` `CreateSamplingRule` MINTS — a
/// declaration states the rule's name, never its ARN, and that ARN is what every X-Ray
/// call on a rule can address it by — so the identifier a stack reports for the resource
/// is the one that service answered with ([`identity_is_service_assigned`] names it for
/// exactly that).
pub const XRAY_SAMPLING_RULE: &str = "AWS::XRay::SamplingRule";

/// An AWS X-Ray GROUP's resource type (the same `cloudFormationName: XRay`). This is a type
/// `super::provisioner` creates in X-Ray itself, and the type's identity is the `GroupARN`
/// `CreateGroup` MINTS: "Ref returns the group ARN", whose resource path carries a value
/// that service draws per group beside the declared name, so no rule here could compute it
/// and the identifier a stack reports for the resource is the one that service answered with
/// ([`identity_is_service_assigned`] names it for exactly that). The group's NAME is still
/// CloudFormation's — that is what the create is made under, read off the type's own
/// `GroupName` property by the naming conventions.
pub const XRAY_GROUP: &str = "AWS::XRay::Group";

/// A KMS key's resource type (the KMS model: `cloudFormationName: KMS`). This is a type
/// `super::provisioning` creates in KMS itself, and "Ref returns the key ID" — `CreateKey`
/// takes no identifier and MINTS the key id — so the id a stack reports for the resource,
/// and the ARN built from it, are the ones KMS answered with and the ones
/// `kms DescribeKey` (and every service that resolves a `KmsMasterKeyId`) resolves.
pub const KMS_KEY: &str = "AWS::KMS::Key";

/// A KMS alias's resource type (the same `cloudFormationName: KMS`). This is a type
/// `super::provisioning` creates in KMS itself, and "Ref returns the alias name" — the
/// `AliasName` the declaration states, which is what `CreateAlias` creates the alias under
/// and what every operation taking a `KeyId` resolves to the key the alias points at. The
/// type publishes no `Fn::GetAtt` attribute, so the name is the whole of what a stack
/// reports for one, and no naming rule computes it: an alias KMS would refuse the name of
/// is the refusal that service answers, which is what the deployment reports.
pub const KMS_ALIAS: &str = "AWS::KMS::Alias";

/// KMS's ARN namespace, which is its model's own `awsService.arnNamespace` (`kms`) — the
/// service segment of a key ARN.
pub const KMS_ARN_NAMESPACE: &str = "kms";

/// The three Route 53 Resolver resource types a stack creates in that service (the
/// Route 53 Resolver model: `cloudFormationName: Route53Resolver`).
///
/// Each one's identifier is Route 53 Resolver's OWN to mint — `Ref` of an endpoint returns
/// its `rslvr-out-…`/`rslvr-in-…` id, of a rule its `rslvr-rr-…` id and of a rule
/// association its `rslvr-rrassoc-…` id, and none of the three creates takes an identifier
/// — so the id a stack's output reports for one is the id that service answered with, which
/// is what `GetResolverEndpoint`, `GetResolverRule` and `GetResolverRuleAssociation`
/// resolve.
pub const ROUTE53RESOLVER_ENDPOINT: &str = "AWS::Route53Resolver::ResolverEndpoint";
pub const ROUTE53RESOLVER_RULE: &str = "AWS::Route53Resolver::ResolverRule";
pub const ROUTE53RESOLVER_RULE_ASSOCIATION: &str =
    "AWS::Route53Resolver::ResolverRuleAssociation";

/// The three Route 53 Resolver DNS Firewall resource types a stack creates in that service
/// (the same `cloudFormationName: Route53Resolver`).
///
/// Each one's identifier is Route 53 Resolver's OWN to mint, exactly as for the three types
/// above: `Ref` of a domain list returns its `rslvr-fdl-…` id, of a rule group its
/// `rslvr-frg-…` id and of a rule group association its `rslvr-frgassoc-…` id, and none of
/// the three calls that make them takes an identifier — a `Name` such a type declares is only
/// "a name that lets you identify [it], to manage and use it". So the id a stack's output
/// reports for one is the id that service answered with, which is what
/// `GetFirewallDomainList`, `GetFirewallRuleGroup` and `GetFirewallRuleGroupAssociation`
/// resolve.
pub const ROUTE53RESOLVER_FIREWALL_DOMAIN_LIST: &str =
    "AWS::Route53Resolver::FirewallDomainList";
pub const ROUTE53RESOLVER_FIREWALL_RULE_GROUP: &str = "AWS::Route53Resolver::FirewallRuleGroup";
pub const ROUTE53RESOLVER_FIREWALL_RULE_GROUP_ASSOCIATION: &str =
    "AWS::Route53Resolver::FirewallRuleGroupAssociation";

/// The two Amazon Route 53 resource types a stack creates in that service (the Route 53
/// model: `cloudFormationName: Route53`).
///
/// A hosted zone's identifier is Route 53's OWN to mint: "Ref returns the hosted zone ID",
/// `CreateHostedZone` takes no identifier and answers the `Z…` id it assigned, and that id
/// — not the zone's DNS name, which is only what the zone is CALLED — is what every
/// operation on the zone takes (`GetHostedZone$Id`, `TestDNSAnswer$HostedZoneId` and a
/// record set's own `HostedZoneId`, each a `ResourceId` of at most 32 characters). A record
/// set's identifier is CloudFormation's: "Ref returns the record name", which is the `Name`
/// its declaration states.
pub const ROUTE53_HOSTED_ZONE: &str = "AWS::Route53::HostedZone";
pub const ROUTE53_RECORD_SET: &str = "AWS::Route53::RecordSet";

/// A GROUP of record sets, which is the same declaration several times over: "a complex type
/// that contains one `RecordSet` element for each record set that you want to create".
///
/// The group is no resource of Route 53's — that service holds record sets, not groups — so
/// its identifier is the one CloudFormation's naming rules give it, and the record sets it
/// declares are applied to the zone the GROUP names in one change batch.
pub const ROUTE53_RECORD_SET_GROUP: &str = "AWS::Route53::RecordSetGroup";

/// A health check of Amazon Route 53's, the third type a stack creates in that service.
///
/// Its identifier is Route 53's own to mint the same way a hosted zone's is: "Ref returns
/// the health check ID", `CreateHealthCheck` takes no identifier and answers the id it
/// assigned, and that id — not any name, since the type declares none — is what every
/// operation on the health check takes (`GetHealthCheck$HealthCheckId`,
/// `UpdateHealthCheck$HealthCheckId` and a record set's own `HealthCheckId`).
pub const ROUTE53_HEALTH_CHECK: &str = "AWS::Route53::HealthCheck";

/// The three further Amazon Route 53 resource types a stack creates in that service: the
/// CIDR collection an IP-based record set routes through, the reusable delegation set a
/// zone can be created on, and the traffic policy a routing configuration is held as.
///
/// Each one's identifier is Route 53's OWN to mint, as a hosted zone's and a health
/// check's are. `CreateCidrCollection` takes the collection's name and answers the UUID it
/// assigned, and every call on the collection takes that UUID
/// (`ChangeCidrCollection$Id`, `ListCidrBlocks$CollectionId`, and a record set's own
/// `CidrRoutingConfig.CollectionId`). `CreateReusableDelegationSet` takes no identifier at
/// all and answers the `N…` id it assigned, which is what `GetReusableDelegationSet` and a
/// zone's own `DelegationSetId` take. `CreateTrafficPolicy` takes the policy's name and
/// document and answers the id and the version it assigned, which is what
/// `GetTrafficPolicy` and a traffic policy instance's own `TrafficPolicyId` take.
pub const ROUTE53_CIDR_COLLECTION: &str = "AWS::Route53::CidrCollection";
pub const ROUTE53_DELEGATION_SET: &str = "AWS::Route53::DelegationSet";
pub const ROUTE53_TRAFFIC_POLICY: &str = "AWS::Route53::TrafficPolicy";

/// The first character of a reusable delegation set id, and how many follow it.
///
/// Route 53 publishes one as `N` followed by uppercase alphanumerics (`N1PA6795SAMPLE` in
/// `CreateReusableDelegationSet`'s own examples), inside the 32-character bound its
/// `ResourceId` shape fixes — the same shape a hosted zone id takes under a different first
/// character. What [`return_values`] computes is a STAND-IN of that shape, replaced by the
/// id Route 53 minted once `super::provisioning` has created the set.
pub(super) const DELEGATION_SET_ID_PREFIX: &str = "N";

/// What the digest behind that stand-in is salted with — see [`HOSTED_ZONE_ID_SALT`].
pub(super) const DELEGATION_SET_ID_SALT: &str = "route53-delegation-set";

/// The `Version` the one traffic policy an `AWS::Route53::TrafficPolicy` creates carries:
/// "for a new traffic policy, the value of `Version` is always 1" (`TrafficPolicy$Version`).
/// A further version is a `CreateTrafficPolicyVersion`, which no property of this type asks
/// for.
pub const FIRST_TRAFFIC_POLICY_VERSION: u32 = 1;

/// The `RecordType` an `AWS::Route53::TrafficPolicy`'s declared `Document` states, which is
/// the `Type` that type publishes: "the DNS type of the resource record sets that Amazon
/// Route 53 creates when you use a traffic policy to create a traffic policy instance"
/// (`TrafficPolicy$Type`), read OFF the document because the create carries no type at all.
///
/// The property is a JSON document, which a template may state as the document itself or as
/// the text of one, so both forms are read. A declaration carrying neither states no record
/// type, and the attribute is then absent rather than guessed — the resource's own create is
/// what refuses such a declaration.
fn declared_record_type(properties: &Value) -> Option<String> {
    let declared = properties.get("Document")?;
    let parsed = match declared {
        Value::String(text) => ::serde_json::from_str::<Value>(text).ok()?,
        document => document.clone(),
    };
    parsed.get("RecordType").and_then(scalar_text)
}

/// Route 53's ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`route53`) — the service segment of a CIDR collection's ARN.
pub const ROUTE53_ARN_NAMESPACE: &str = "route53";

/// The ARN of the reusable delegation set `id`, which is what that type's `Ref` returns.
///
/// Route 53 is a GLOBAL service and its entities belong to no account's namespace in an ARN:
/// a delegation set's ARN carries neither a Region nor an account id
/// (`arn:aws:route53:::delegationset/<id>`), the same form a hosted zone's takes
/// (`arn:aws:route53:::hostedzone/<id>`). The resource path is the entity and the bare id
/// Route 53 minted.
pub fn delegation_set_arn(id: &str) -> String {
    route53_entity_arn("delegationset", id)
}

/// The ARN of the traffic policy `id`, which is what that type's `Ref` returns — the same
/// account-less form a delegation set's takes ([`delegation_set_arn`]).
pub fn traffic_policy_arn(id: &str) -> String {
    route53_entity_arn("trafficpolicy", id)
}

/// The ARN of one Route 53 entity, as that service's own resource ARNs are written: no
/// Region, no account, and the entity and its bare id as the resource path.
fn route53_entity_arn(entity: &str, id: &str) -> String {
    format!("arn:{PARTITION}:{ROUTE53_ARN_NAMESPACE}:::{entity}/{id}")
}

/// The entity segment of a CIDR collection's ARN, as Route 53 writes it
/// (`arn:aws:route53:::cidrcollection/<id>`).
pub(crate) const ROUTE53_CIDR_COLLECTION_ENTITY: &str = "cidrcollection";

/// The reusable delegation set id an ARN of that form names: its resource path's last
/// segment, which is the bare `ResourceId` every operation on the set takes.
///
/// A value carrying no path at all is already that id, which is what a template stating one
/// literally writes. The same reading gives a traffic policy's id out of its own ARN, since
/// both are written the same way.
pub fn route53_id_in(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// The first character of a hosted zone id, and how many follow it.
///
/// Route 53 publishes a hosted zone id as `Z` followed by uppercase alphanumerics
/// (`Z1D633PJN98FT9` in `ChangeResourceRecordSets`' own examples), inside the 32-character
/// bound its `ResourceId` shape fixes. What [`return_values`] computes is a STAND-IN of that
/// shape: this is also the type `super::provisioning` creates in Route 53 itself, which
/// answers with the id IT minted — and THAT id is what the stack reports for a zone Route 53
/// created.
pub(super) const HOSTED_ZONE_ID_PREFIX: &str = "Z";
pub(super) const HOSTED_ZONE_ID_DIGITS: usize = 14;

/// What the digest behind that stand-in is salted with: the resource TYPE, so the digits are
/// this type's own and cannot collide with another arm's for a resource of the same name
/// ([`derived_digits`] — "the `salt` keeps two identifiers OF ONE resource … from being the
/// same digits").
pub(super) const HOSTED_ZONE_ID_SALT: &str = "route53-hosted-zone";

/// An AWS Certificate Manager ACME endpoint's resource type (the ACM model:
/// `cloudFormationName: CertificateManager`). This is a type `super::provisioning`
/// creates in ACM itself, and the identifier the provider reports for it is the
/// `AcmeEndpointArn` ACM minted — an ARN `CreateAcmeEndpoint` ASSIGNS, which no naming
/// rule could compute — so the identifier a stack reports for the resource is the value
/// `acm DescribeAcmeEndpoint` resolves.
pub const ACM_ACME_ENDPOINT: &str = "AWS::CertificateManager::AcmeEndpoint";

/// An AWS Certificate Manager certificate's resource type (the same model's
/// `cloudFormationName: CertificateManager`). This is a type `super::provisioning` creates
/// in ACM itself, and the identifier the provider reports for it is the `CertificateArn`
/// ACM minted — an ARN `RequestCertificate` ASSIGNS, which no naming rule could compute —
/// so the identifier a stack reports for the resource is the value `acm DescribeCertificate`
/// resolves ("Ref returns the certificate ARN").
pub const ACM_CERTIFICATE: &str = "AWS::CertificateManager::Certificate";

/// An Amazon Web Services Private CA certificate authority's resource type (the acm-pca
/// model: `cloudFormationName: ACMPCA`). This is a type `super::provisioning` creates in
/// that service itself, and the identifier the provider reports for it is the
/// `CertificateAuthorityArn` acm-pca minted — an ARN `CreateCertificateAuthority` ASSIGNS,
/// which no naming rule could compute — so the identifier a stack reports and exports for
/// the resource is the one `acm-pca DescribeCertificateAuthority` resolves ("Ref returns
/// the Amazon Resource Name (ARN) of the private CA").
pub const ACMPCA_CERTIFICATE_AUTHORITY: &str = "AWS::ACMPCA::CertificateAuthority";

/// The certificate an `AWS::ACMPCA::Certificate` has a private CA issue (the same acm-pca
/// model). `IssueCertificate` ASSIGNS its `CertificateArn` — "Ref returns the ARN of the
/// issued certificate" — so the identifier a stack reports for the resource is the one
/// `acm-pca GetCertificate` serves, and the certificate BODY it issued is this type's other
/// published return value (`Fn::GetAtt Certificate`).
pub const ACMPCA_CERTIFICATE: &str = "AWS::ACMPCA::Certificate";

/// The activation of a private CA with a certificate issued for it (the same acm-pca
/// model): the type whose deployment imports that certificate into the CA and puts the CA
/// in the status the declaration asks for. Its identity IS the CA it activates — that is
/// the one resource the activation exists about — and its published return value is the
/// complete chain the CA then presents (`Fn::GetAtt CompleteCertificateChain`).
pub const ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION: &str =
    "AWS::ACMPCA::CertificateAuthorityActivation";

/// The GRANT a private CA makes to one service principal (the same acm-pca model): the type
/// whose deployment is `CreatePermission` and whose removal is `DeletePermission`.
///
/// It declares no entity of its own and it carries no `Fn::GetAtt` value: a grant belongs to
/// the `(CertificateAuthorityArn, Principal)` pair every call of that service on one is
/// addressed by — `DeletePermission` takes exactly those two, and `ListPermissions` reports a
/// `Permission` under them — so that pair is this type's primary identifier and its physical
/// resource id ([`composite_identity_properties`]), which is what `Ref` returns.
pub const ACMPCA_PERMISSION: &str = "AWS::ACMPCA::Permission";

/// The ARN namespace AWS Step Functions mints its resources under: that model's
/// `awsService.arnNamespace`, which is `states` rather than the `StepFunctions` its
/// resource types are spelled with.
pub const STEP_FUNCTIONS_ARN_NAMESPACE: &str = "states";

/// An AWS Step Functions activity's resource type (the Step Functions model:
/// `cloudFormationName: StepFunctions`, `arnNamespace: states`). This is a type
/// `super::provisioning` creates in Step Functions itself, and "Ref returns the ARN of the
/// activity", so the handle a stack publishes for one is the ARN `sfn DescribeActivity`
/// answers with.
pub const STEP_FUNCTIONS_ACTIVITY: &str = "AWS::StepFunctions::Activity";

/// An AWS Step Functions state machine's resource type. Also created in that service, and
/// "Ref returns the ARN of the state machine" — an ARN that is a total function of the
/// Region, the account and the machine's name, so the handle a stack publishes for one is
/// the ARN `sfn DescribeStateMachine` answers with.
pub const STEP_FUNCTIONS_STATE_MACHINE: &str = "AWS::StepFunctions::StateMachine";

/// An AWS Secrets Manager secret's resource type (the Secrets Manager model:
/// `cloudFormationName: SecretsManager`, `arnNamespace: secretsmanager`). This is a type
/// `super::provisioning` creates in Secrets Manager itself, and "Ref returns the ARN of the
/// secret" — an ARN whose six trailing characters that service draws per secret, which is
/// why [`identity_is_service_assigned`] names the type: the handle a stack publishes for one
/// is the ARN `secretsmanager describe-secret` answers with, not a value a naming rule here
/// could compute.
pub const SECRETSMANAGER_SECRET: &str = "AWS::SecretsManager::Secret";

/// The rotation an AWS Secrets Manager secret is configured with, as a resource type of its
/// own. This is a type `super::provisioning` puts in force in Secrets Manager itself, and
/// "Ref returns the ARN of the secret" — the ARN of the secret the declaration's `SecretId`
/// names, which `RotateSecret` answers and whose six trailing characters that service drew
/// per secret. That is why [`identity_is_service_assigned`] names the type: the identifier
/// the resource is known by is one no naming rule here can compute, so a re-deploy carries
/// the one in force forward and the declaration is applied to the rotation that exists
/// rather than read as a different resource.
pub const SECRETSMANAGER_ROTATION_SCHEDULE: &str = "AWS::SecretsManager::RotationSchedule";

/// The resource-based policy an AWS Secrets Manager secret carries, as a resource type of
/// its own. This is a type `super::provisioning` attaches in Secrets Manager itself, and the
/// resource is the policy of the secret the declaration's `SecretId` names — so the
/// identifier it is known by is that secret's ARN, which `PutResourcePolicy` answers and
/// whose six trailing characters that service drew per secret. That is why
/// [`identity_is_service_assigned`] names the type: the identifier in force is one no naming
/// rule here can compute, so a re-deploy carries the one in force forward and the
/// declaration is applied to the policy that exists rather than read as a different
/// resource.
pub const SECRETSMANAGER_RESOURCE_POLICY: &str = "AWS::SecretsManager::ResourcePolicy";

/// A Systems Manager Parameter Store parameter's resource type (the SSM model:
/// `cloudFormationName: SSM`). This is a type `super::provisioning` creates in Systems
/// Manager itself, and "Ref returns the name of the parameter", so the parameter a stack
/// declares is the one `ssm GetParameter` answers with.
pub const SSM_PARAMETER: &str = "AWS::SSM::Parameter";

/// A State Manager association's resource type. This is a type `super::provisioning` creates
/// in Systems Manager itself, and "Ref returns the association ID" — the identifier that
/// service MINTS for the association, which is why [`identity_is_service_assigned`] names the
/// type: `CreateAssociation` takes no identifier for the association (the `Name` a declaration
/// states names the DOCUMENT it applies), every call on one takes that id, and the id is the
/// 8-4-4-4-12 hexadecimal the SSM model's `AssociationId` fixes
/// (`smithy_query(query="shape", shape="AssociationId")`), which no naming rule here could
/// compute.
pub const SSM_ASSOCIATION: &str = "AWS::SSM::Association";

/// The published return value an `AWS::SSM::Association` carries its identifier under
/// ("Fn::GetAtt … AssociationId: the association ID"), which is also the member Systems
/// Manager's own `AssociationDescription` reports it as.
pub const SSM_ASSOCIATION_ID_ATTRIBUTE: &str = "AssociationId";

/// The published return value an `AWS::SSM::PatchBaseline` carries its identifier under
/// ("Fn::GetAtt … Id: the ID of the patch baseline"), which is also what its `Ref` returns —
/// the `pb-…` id `CreatePatchBaseline` MINTS, which is why [`identity_is_service_assigned`]
/// names that type: the call takes no identifier for the baseline (a declaration states the
/// name it is called and the patches it approves), every Systems Manager call on one takes
/// that id, and no naming rule here could compute it.
pub const SSM_PATCH_BASELINE_ID_ATTRIBUTE: &str = "Id";

/// A Resource Groups resource group's resource type (that model:
/// `cloudFormationName: ResourceGroups`, `arnNamespace: resource-groups`). This is a type
/// `super::provisioning` creates in Resource Groups itself, and "Ref returns the group name",
/// so the group a stack declares is the one `resource-groups GetGroup` answers with.
pub const RESOURCE_GROUPS_GROUP: &str = "AWS::ResourceGroups::Group";

/// The ARN namespace Resource Groups mints its groups under (that model's
/// `awsService.arnNamespace`): the HYPHENATED `resource-groups`, which is the only namespace
/// token the modelled group-ARN form admits (`GroupArnV2`'s pattern,
/// `^arn:aws(-[a-z]+)*:resource-groups:…:group/…$`) — and is NOT what lower-casing the type's
/// `ResourceGroups` namespace segment gives.
pub const RESOURCE_GROUPS_ARN_NAMESPACE: &str = "resource-groups";

/// An EventBridge Scheduler schedule group's resource type (that model:
/// `cloudFormationName: Scheduler`, `arnNamespace: scheduler`). This is a type
/// `super::provisioning` creates in EventBridge Scheduler itself, and "Ref returns the
/// schedule group name", so the group a stack declares is the one
/// `scheduler GetScheduleGroup` answers with.
pub const SCHEDULER_SCHEDULE_GROUP: &str = "AWS::Scheduler::ScheduleGroup";

/// An EventBridge Scheduler schedule's resource type. Also created in that service, and
/// "Ref returns the schedule name", so the schedule a stack declares is one
/// `scheduler ListSchedules` reports under its group.
pub const SCHEDULER_SCHEDULE: &str = "AWS::Scheduler::Schedule";

/// The ARN namespace EventBridge Scheduler mints its resources under (that model's
/// `awsService.arnNamespace`).
pub const SCHEDULER_ARN_NAMESPACE: &str = "scheduler";

/// The schedule group a schedule declaring none lands in: `GetSchedule$GroupName` — "the
/// name of the schedule group associated with this schedule. If you omit this, the default
/// schedule group is used" — whose name is `default`, and the group whose name a schedule's
/// ARN still carries a segment for (`schedule/<group>/<name>`).
pub const DEFAULT_SCHEDULE_GROUP: &str = "default";

/// How many characters the id Amazon GuardDuty mints for a detector carries: 32 lowercase
/// hexadecimal, the form that service's own `mint_id` produces (`DETECTOR_ID_LEN` in
/// `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs`).
pub(super) const DETECTOR_ID_HEX_DIGITS: usize = 32;

/// The salt the stand-in detector id is derived under (see [`derived_digits`]), so it
/// cannot collide with another identifier derived for the same resource.
pub(super) const DETECTOR_ID_SALT: &str = "guardduty/detector-id";

/// The salt the stand-in IPSet id is derived under (see [`derived_digits`]), so the
/// identifier computed for a trusted IP list cannot collide with the one computed for a
/// detector of the same name. The WIDTH is [`DETECTOR_ID_HEX_DIGITS`]: GuardDuty mints an
/// `IpSetId` with the same `mint_id` it mints a detector id with.
pub(super) const IP_SET_ID_SALT: &str = "guardduty/ip-set-id";

/// The form of the identifier Directory Service mints for a directory, from that model's own
/// `DirectoryId` (`^d-[0-9a-f]{10}$`): the prefix, and the ten lowercase hexadecimal
/// characters after it.
pub(super) const DIRECTORY_ID_PREFIX: &str = "d-";
pub(super) const DIRECTORY_ID_HEX_DIGITS: usize = 10;

/// The salt the stand-in directory id is derived under (see [`derived_digits`]), so it cannot
/// collide with another identifier derived for the same resource.
pub(super) const DIRECTORY_ID_SALT: &str = "directoryservice/directory-id";

/// The prefix of an experiment-template id, as FIS mints one (`EXT` + a base-36 suffix;
/// FIS's own `TEMPLATE_ID_PREFIX`).
pub(super) const FIS_TEMPLATE_ID_PREFIX: &str = "EXT";

/// How many characters follow [`FIS_TEMPLATE_ID_PREFIX`] in an experiment-template id:
/// FIS's own `ID_SUFFIX_LEN`, the width of the base-36 suffix every id it mints carries.
pub(super) const FIS_TEMPLATE_ID_DIGITS: usize = 12;

/// The regional host API Gateway serves a custom domain name on
/// (`DomainNameConfiguration$ApiGatewayDomainName`): the
/// `d-{id}.execute-api.{region}.amazonaws.com` form a caller aliases their own DNS record
/// to, with the id derived here for the reason [`derived_digits`] gives and replaced by
/// the one API Gateway v2 really assigned once the domain is created there.
pub(super) fn apigatewayv2_regional_domain_name(name: &str, region: &str) -> String {
    format!(
        "d-{}.execute-api.{region}.{URL_SUFFIX}",
        api_gateway_assigned_id(APIGATEWAYV2_DOMAIN_LABEL_SALT, name)
    )
}

/// The revision a task definition registered for a family the account does not hold yet
/// carries: "the first task definition that you registered to a family is given a revision
/// of 1".
pub(super) const FIRST_TASK_DEFINITION_REVISION: u32 = 1;

/// The revision a job definition registered under a name the account holds none under yet
/// carries: AWS Batch allocates "the revision of the job definition" one past the highest it
/// already holds for the name, so a first registration is revision 1.
pub(super) const FIRST_JOB_DEFINITION_REVISION: u32 = 1;

/// How many characters the id Elastic Load Balancing assigns a load balancer, a target
/// group or a listener carries: 16 lowercase hexadecimal, the form that service's own
/// `mint_id` produces (`.../app/my-load-balancer/50dc6c495c0c9188`).
pub(super) const ELB_ID_HEX_DIGITS: usize = 16;

/// The salts those ids are derived under (see [`service_assigned_label`]) — one PER KIND,
/// because Elastic Load Balancing keeps the two name spaces apart (a load balancer and a
/// target group may carry the same name) while the ids it assigns them are distinct.
pub(super) const ELB_LOAD_BALANCER_ID_SALT: &str = "elasticloadbalancing/loadbalancer-id";
pub(super) const ELB_TARGET_GROUP_ID_SALT: &str = "elasticloadbalancing/targetgroup-id";

/// The `Scheme` value of an INTERNAL load balancer, whose DNS name carries the
/// `internal-` prefix.
pub(super) const ELB_SCHEME_INTERNAL: &str = "internal";

/// The segment a load balancer's ARN carries for its `Type`: "`app` for an Application
/// Load Balancer, `net` for a Network Load Balancer, `gwy` for a Gateway Load Balancer",
/// with `application` the modelled default for the property.
pub(super) fn load_balancer_type_code(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> &'static str {
    match properties
        .get("Type")
        .and_then(|value| text(value, scope, 0, budget))
        .as_deref()
    {
        Some("network") => "net",
        Some("gateway") => "gwy",
        _ => "app",
    }
}

/// Fill in each resource's evaluated `Properties`, against the whole set of resources the
/// template declares.
///
/// This is the request the service that owns a resource is asked to create it with
/// ([`super::provisioning`](crate::state::provisioning)), so every intrinsic function in
/// the properties is evaluated here: a property naming another resource (a stream's
/// `RoleARN` from an `Fn::GetAtt`) carries that resource's value, and one that cannot be
/// evaluated is DROPPED rather than sent as a guess — which is also what CloudFormation's
/// own `AWS::NoValue` means ("removes the corresponding resource property").
///
/// `resources` is what [`provision`] produced for `template`. A declaration whose
/// `Condition` excluded it has no record there, so each record is matched to its
/// declaration by LOGICAL ID rather than by position.
///
/// A property the type's schema marks READ-ONLY is left out of that document
/// ([`super::without_read_only`]): it is a value the owning service computes rather than a
/// member of any call, so a declaration carrying one is deployed as though it did not — and
/// what every reader of the resource then reports for it is that service's own value.
pub fn resolve_properties(
    template: &Template,
    resources: &mut [StoredResource],
    scope: &Scope<'_>,
    budget: &mut Budget,
) {
    for resource in resources.iter_mut() {
        let Some(declared) = template
            .resources
            .iter()
            .find(|declared| declared.logical_id == resource.logical_id)
        else {
            continue;
        };
        resource.properties = super::without_read_only(
            &resource.resource_type,
            resolved_value(&declared.properties, scope, 0, budget).unwrap_or(Value::Null),
        );
        // A type whose primary identifier is COMPOSITE takes its physical id from that
        // resolved document: this is the first point at which the resources the binding
        // names have resolved to the identifiers they really carry.
        adopt_composite_identity(resource);
    }
}

/// One template value with every intrinsic function it carries evaluated, preserving the
/// objects and arrays around them; `None` for a value that cannot be evaluated at all.
///
/// The one entry point for evaluating a value OUTSIDE a deployment: comparing what two
/// revisions of a template declare for one property has to compare the values each would
/// really deploy, since a property that reads `!Ref Size` is a different value under a
/// different parameter value while the template text is the same.
pub fn resolve(value: &Value, scope: &Scope<'_>, budget: &mut Budget) -> Option<Value> {
    resolved_value(value, scope, 0, budget)
}

/// What CloudFormation says about an `Fn::GetAtt` naming an attribute the resource it names
/// does not publish: "Requested attribute <attribute> does not exist in schema for <resource
/// type>". Fixed text around the attribute the template asked for and the type whose schema
/// does not declare it.
///
/// The TYPE rather than the logical id, because the attributes a resource answers are a
/// property of its type's schema — the same schema the registry publishes for it
/// ([`crate::state::resource_schema`]) — so what the author has to change is the attribute
/// name, and what says why is the type that does not have it. This is the reason the
/// CONSUMING resource's create fails, which is where CloudFormation reports it: the read is
/// resolved as that resource is deployed, so the diagnosis reaches the caller as that
/// resource's `ResourceStatusReason` rather than as a refusal of the submitted template.
pub fn unsupported_attribute_reason(resource_type: &str, attribute: &str) -> String {
    format!("Requested attribute {attribute} does not exist in schema for {resource_type}")
}

/// Every resource of `template` whose declaration reads an attribute the resource it names
/// does not publish, as `(the resource named, the attribute read)` keyed by the resource whose
/// declaration reads it.
///
/// `Fn::GetAtt` "returns the value of an attribute from a resource in the template", and the
/// attributes a resource publishes are the ones its record carries — for a CUSTOM RESOURCE
/// that is the `Data` its provider answered with. So this is only asked once the records are
/// settled: a resource whose attributes are not published YET reads as publishing none, and
/// a reader of this answers for the resources whose attributes are final.
///
/// What the reader REPORTS for such a read is not this module's to say: CloudFormation words
/// it one way for an attribute a type does not publish ([`unsupported_attribute_reason`]) and
/// another for a member a custom resource's own provider left out of its answer
/// ([`crate::state::provisioning::custom_resource::vendor_response_missing_attribute`]), and
/// which it is depends on the resource NAMED — so both names travel to the caller that knows
/// the type.
///
/// Only an attribute of a resource `scope` KNOWS is judged: a `GetAtt` naming a logical id
/// the scope does not carry is a reference to nothing rather than to an attribute nothing
/// publishes, which is a different error and not this one.
pub fn unresolved_attributes(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> BTreeMap<String, (String, String)> {
    let mut unresolved = BTreeMap::new();
    for declared in &template.resources {
        if let Some(read) = missing_attribute(&declared.properties, scope, 0, budget) {
            unresolved.insert(declared.logical_id.clone(), read);
        }
    }
    unresolved
}

/// The properties of each resource of `template` whose stated value READS an attribute that
/// is not published YET, keyed by logical id.
///
/// Such a property has a value — the one the resource it names will publish — and the
/// deployment resolves it before the resource carrying it is created
/// ([`crate::state::StoredStack::resolve_deferred_declarations`], driven by
/// [`crate::state::provisioning::deferred`]). What it does NOT have is a value yet: the
/// attributes a resource publishes are the ones its record carries, and a resource whose own
/// creation is still in flight — a custom resource, whose attributes are the `Data` its
/// provider answers with, or a resource whose owning service fills them in as it brings it up
/// — publishes none of them at the instant the template is deployed. So the resolved document
/// a deployment holds for it carries no such member at all, and anything that judges that
/// document against the type's contract has to know which members are missing from it only
/// because their value has not landed ([`crate::state::property_validation`]).
///
/// Judged by the SAME relation [`unresolved_attributes`] reads — an attribute the record of
/// the resource it is read off does not carry — over a walk of the same shape and the same
/// budget. The difference is only WHEN each is asked: this one before the deployment starts,
/// where an unpublished attribute is one still to come, and that one once the records are
/// settled, where it is one the resource does not have.
pub fn deferred_property_reads(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut deferred: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for declared in &template.resources {
        let Value::Object(members) = &declared.properties else {
            continue;
        };
        let names: BTreeSet<String> = members
            .iter()
            .filter(|(_, value)| reads_an_unpublished_attribute(value, scope, 0, budget))
            .map(|(name, _)| name.clone())
            .collect();
        if !names.is_empty() {
            deferred.insert(declared.logical_id.clone(), names);
        }
    }
    deferred
}

/// Whether `value` READS an attribute that is not published yet, in either spelling an
/// attribute is read in: an `Fn::GetAtt`, and the `${Name.Attribute}` of an `Fn::Sub` source,
/// "which is an `Fn::GetAtt` by another spelling"
/// ([`super::substitution_names`] reads the variables of one).
///
/// Walked over the same document shape every other evaluation walks, and charged to the same
/// budget, so a declaration cannot drive this deeper or longer than its own evaluation.
fn reads_an_unpublished_attribute(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> bool {
    if depth > MAX_INTRINSIC_DEPTH || budget.spend(1).is_none() {
        return false;
    }
    match value {
        Value::Object(members) => {
            if let Some(argument) = members.get("Fn::GetAtt").filter(|_| members.len() == 1) {
                if let Some((logical_id, attribute)) = named_attribute(argument) {
                    if is_unpublished(scope, &logical_id, &attribute) {
                        return true;
                    }
                }
            }
            if let Some(argument) = members.get(FN_SUB).filter(|_| members.len() == 1) {
                if substituted_missing_attribute(argument, scope).is_some() {
                    return true;
                }
            }
            members
                .values()
                .any(|member| reads_an_unpublished_attribute(member, scope, depth + 1, budget))
        }
        Value::Array(items) => items
            .iter()
            .any(|item| reads_an_unpublished_attribute(item, scope, depth + 1, budget)),
        _ => false,
    }
}

/// Whether the resource `logical_id` of `scope` carries no attribute `attribute` — `false`
/// for a logical id the scope does not know at all, which is a reference to nothing rather
/// than to an attribute nothing publishes.
fn is_unpublished(scope: &Scope<'_>, logical_id: &str, attribute: &str) -> bool {
    scope
        .resources
        .get(logical_id)
        .is_some_and(|resource| !publishes_attribute(&resource.attributes, attribute))
}

/// The FIRST attribute read of `value` naming an attribute the resource it names does not
/// publish, as `(logical id, attribute)`.
///
/// Both spellings an attribute is read in, exactly as [`reads_an_unpublished_attribute`]
/// counts them: an `Fn::GetAtt`, and the `${Name.Attribute}` of an `Fn::Sub` source, "which
/// is an `Fn::GetAtt` by another spelling". A read the deferral withholds a resource FOR has
/// to be a read this can report once the records are settled, or the resource is created with
/// the member that read left out instead of failing on it.
///
/// Walked over the same document shape every other evaluation walks, and charged to the same
/// budget, so a declaration cannot drive this deeper or longer than its own evaluation.
fn missing_attribute(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<(String, String)> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    match value {
        Value::Object(members) => {
            if let Some(argument) = members.get("Fn::GetAtt").filter(|_| members.len() == 1) {
                if let Some((logical_id, attribute)) = named_attribute(argument) {
                    if is_unpublished(scope, &logical_id, &attribute) {
                        return Some((logical_id, attribute));
                    }
                }
            }
            if let Some(argument) = members.get(FN_SUB).filter(|_| members.len() == 1) {
                if let Some(read) = substituted_missing_attribute(argument, scope) {
                    return Some(read);
                }
            }
            members
                .values()
                .find_map(|member| missing_attribute(member, scope, depth + 1, budget))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| missing_attribute(item, scope, depth + 1, budget)),
        _ => None,
    }
}

/// The FIRST `${Name.Attribute}` variable of one `Fn::Sub` argument naming an attribute the
/// resource it names does not publish — the `Fn::Sub` half of [`missing_attribute`], read off
/// the source string the same way [`reads_an_unpublished_attribute`] reads it.
fn substituted_missing_attribute(
    argument: &Value,
    scope: &Scope<'_>,
) -> Option<(String, String)> {
    let source = match argument {
        Value::String(source) => Some(source.as_str()),
        Value::Array(arguments) => arguments.first().and_then(Value::as_str),
        _ => None,
    }?;
    super::substitution_names(source).into_iter().find_map(|(name, path)| {
        let attribute = path
            .as_deref()
            .and_then(|path| path.split_once('.'))
            .map(|(_, attribute)| attribute.to_string())?;
        is_unpublished(scope, &name, &attribute).then_some((name, attribute))
    })
}

/// The resource and attribute an `Fn::GetAtt` argument NAMES, for the two forms that spell
/// both out: `["<logical id>", "<attribute>", …]` and the short form's `<logical id>.<attribute>`.
///
/// `None` where either half is itself an expression — such a `GetAtt` names whatever that
/// expression resolves to, which is not something to judge against one resource's attributes.
fn named_attribute(argument: &Value) -> Option<(String, String)> {
    match argument {
        Value::Array(parts) if parts.len() >= 2 => {
            let logical_id = parts[0].as_str()?.to_string();
            let mut path = Vec::with_capacity(parts.len() - 1);
            for part in &parts[1..] {
                path.push(part.as_str()?.to_string());
            }
            Some((logical_id, path.join(".")))
        }
        Value::String(dotted) => dotted
            .split_once('.')
            .map(|(logical_id, attribute)| (logical_id.to_string(), attribute.to_string())),
        _ => None,
    }
}

/// One template value with every intrinsic function it carries evaluated, preserving the
/// objects and arrays around them; `None` for a value that cannot be evaluated at all.
pub(super) fn resolved_value(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Value> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    match value {
        Value::Object(members) => {
            // An intrinsic function is a single-member object whose name is the
            // function's; anything else is a property map.
            if let Some((name, argument)) = sole_intrinsic(members) {
                // `Fn::If` "returns one value if the specified condition evaluates to true
                // and another value if the specified condition evaluates to false", and
                // EITHER value may be a structure rather than a scalar — a whole property
                // map, a list of policy statements, a nested list — so the branch the
                // condition chooses ([`super::chosen_branch`]) is resolved HERE, as a value,
                // keeping the objects and arrays inside it. Resolving it as an expression
                // instead would yield no value for every branch that is not text, which
                // drops the property the condition chose.
                if name == FN_IF {
                    let chosen = super::conditions::chosen_branch(argument, scope, depth, budget)?;
                    return resolved_value(chosen, scope, depth + 1, budget);
                }
                // Through the SAME guard the output path uses: the value's size is held
                // to `MAX_RESOLVED_TEXT_BYTES` and its bytes are charged against the
                // deployment's budget, so evaluating a property cannot amplify what
                // evaluating an output cannot.
                let resolved = intrinsic(name, argument, scope, depth, budget)?;
                return match bounded(resolved, budget)? {
                    Resolved::Text(text) => Some(Value::String(text)),
                    Resolved::List(items) => {
                        Some(Value::Array(items.into_iter().map(Value::String).collect()))
                    }
                };
            }
            Some(Value::Object(
                members
                    .iter()
                    .filter_map(|(key, member)| {
                        Some((
                            key.clone(),
                            resolved_value(member, scope, depth + 1, budget)?,
                        ))
                    })
                    .collect(),
            ))
        }
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            Some(Value::Array(
                items
                    .iter()
                    .filter_map(|item| resolved_value(item, scope, depth + 1, budget))
                    .collect(),
            ))
        }
        scalar => {
            budget.spend_text(scalar_text(scalar).map_or(1, |text| text.len()))?;
            Some(scalar.clone())
        }
    }
}

/// The intrinsic function a single-member object names, if it names one: `Ref`, or one of
/// the `Fn::`-prefixed functions.
pub(super) fn sole_intrinsic(members: &Map<String, Value>) -> Option<(&str, &Value)> {
    let mut entries = members.iter();
    let (name, argument) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    (name == "Ref" || name.starts_with("Fn::")).then_some((name.as_str(), argument))
}

/// How many hexadecimal characters the service-assigned label of an Amazon RDS endpoint
/// carries: an RDS endpoint's middle label is 12 characters
/// (`mydb.cluster-c7g8h9i0j1k2.us-east-1.rds.amazonaws.com`).
pub(super) const RDS_ENDPOINT_LABEL_HEX_DIGITS: usize = 12;

/// The salt an RDS endpoint label is derived under (see [`service_assigned_label`]).
pub(super) const RDS_ENDPOINT_LABEL_SALT: &str = "rds/endpoint";

/// The salt a Secrets Manager secret ARN's suffix is derived under, so a secret's ARN
/// suffix and any endpoint label of the same name are different digits.
pub(super) const SECRET_ARN_SUFFIX_SALT: &str = "secretsmanager/secret";

/// How many characters an Amazon RDS immutable resource id carries after its prefix:
/// `DbClusterResourceId`/`DbiResourceId` are published as `cluster-`/`db-` plus 26
/// characters.
pub(super) const RDS_RESOURCE_ID_DIGITS: usize = 26;

/// The `Fn::GetAtt` attribute an `AWS::RDS::DBCluster` and an `AWS::RDS::DBInstance` each
/// publish for the secret Amazon RDS generates and manages the master user password in, from
/// those two types' own "Return values" sections. The dotted name is a member of the
/// resource's read-only `MasterUserSecret` property, exactly as `Endpoint.Address` is a
/// member of its `Endpoint`.
///
/// The VALUE is one only Amazon RDS can produce — it mints the secret in Amazon Web Services
/// Secrets Manager when a declaration turns `ManageMasterUserPassword` on — so the arm below
/// computes a stand-in for it and the provisioning arm re-points it at what that service
/// answered (`crate::state::provisioner::push_master_user_secret_attribute`).
pub(crate) const MASTER_USER_SECRET_ARN_ATTRIBUTE: &str = "MasterUserSecret.SecretArn";

/// How many characters the service-assigned suffix of a Secrets Manager secret ARN
/// carries: "Secrets Manager automatically adds a hyphen and six random characters after
/// the secret name at the end of the ARN".
pub(super) const SECRET_ARN_SUFFIX_CHARACTERS: usize = 6;

/// How many hexadecimal characters an Amazon EC2 resource id carries after its prefix: the
/// current (long) id form is 17 (`sg-0123456789abcdef0`).
pub(super) const EC2_ID_HEX_DIGITS: usize = 17;

/// The Amazon EC2 VPC-network resource types whose published `Ref` return value is an EC2
/// resource IDENTIFIER rather than a name.
///
/// Amazon EC2 takes no name on any of these creates: it ASSIGNS the resource an identifier
/// of its own published form (`vpc-`, `subnet-`, `sg-`, `igw-`, … followed by
/// [`EC2_ID_HEX_DIGITS`] hexadecimal characters). A declaration of one of these types
/// therefore names the resource nowhere, and CloudFormation's
/// `<stack name>-<logical id>-<unique id>` name is no identifier any EC2 API would take for
/// it — which is what each type's own "Return values" section says: "Ref returns the ID of
/// the VPC", "Ref returns the subnet ID", "Ref returns the ID of the security group", "Ref
/// returns the ID of the internet gateway", and so on.
///
/// [`ec2_network_identity`] is what gives a resource of one of them an identifier of that
/// form, so a property of another resource that references it (a Resolver endpoint's
/// `IpAddresses[].SubnetId`, a DB subnet group's `SubnetIds`) carries a value the owning
/// service's own API accepts instead of a name it refuses.
///
/// For the three types this emulator really CREATES in Amazon EC2 — [`EC2_VPC`],
/// [`EC2_SUBNET`] and [`EC2_SECURITY_GROUP`] ([`super::provisioner`]) — that computed value
/// is only the STAND-IN a declaration is resolved against before any of those resources
/// exists: [`identity_is_service_assigned`] names all three, so the identifier in force is
/// the one EC2 answered its create with. For the rest it is still the identifier the stack
/// reports, because nothing here holds such a resource yet.
pub const EC2_VPC: &str = "AWS::EC2::VPC";
pub const EC2_SUBNET: &str = "AWS::EC2::Subnet";
pub const EC2_SECURITY_GROUP: &str = "AWS::EC2::SecurityGroup";
pub const EC2_INTERNET_GATEWAY: &str = "AWS::EC2::InternetGateway";
pub const EC2_ROUTE_TABLE: &str = "AWS::EC2::RouteTable";
pub const EC2_NETWORK_ACL: &str = "AWS::EC2::NetworkAcl";
pub const EC2_NETWORK_INTERFACE: &str = "AWS::EC2::NetworkInterface";
pub const EC2_NAT_GATEWAY: &str = "AWS::EC2::NatGateway";
pub const EC2_VPC_ENDPOINT: &str = "AWS::EC2::VPCEndpoint";
pub const EC2_DHCP_OPTIONS: &str = "AWS::EC2::DHCPOptions";
pub const EC2_VPN_GATEWAY: &str = "AWS::EC2::VPNGateway";

/// `AWS::EC2::SubnetRouteTableAssociation`: the association that puts one route table on one
/// subnet. Not a `<prefix>-` resource of its own in the sense the types above are — the
/// association's identity is the `rtbassoc-…` id `AssociateRouteTable` answers — which is why
/// [`identity_is_service_assigned`] names it: a declaration states the subnet and the table,
/// never that id.
pub const EC2_SUBNET_ROUTE_TABLE_ASSOCIATION: &str = "AWS::EC2::SubnetRouteTableAssociation";

/// `AWS::EC2::EIP`: an Elastic IP address of the account. Not a VPC-network type whose
/// identity is an Amazon EC2 `<prefix>-` id — "Ref returns the Elastic IP address" — but its
/// `AllocationId` return value is one, which is why it is read here.
pub const EC2_EIP: &str = "AWS::EC2::EIP";

/// `AWS::EC2::SecurityGroupIngress` and `AWS::EC2::SecurityGroupEgress`: ONE rule of a security
/// group, declared as a resource of its own rather than inline on the group
/// ([`EC2_SECURITY_GROUP`]'s `SecurityGroupIngress` / `SecurityGroupEgress` properties). The
/// resource IS the rule, and its identity is the `sgr-…` id Amazon EC2 mints for the rule the
/// authorize call adds — which is why [`identity_is_service_assigned`] names both types: a
/// declaration states the group and the permission, never that id.
pub const EC2_SECURITY_GROUP_INGRESS: &str = "AWS::EC2::SecurityGroupIngress";
pub const EC2_SECURITY_GROUP_EGRESS: &str = "AWS::EC2::SecurityGroupEgress";

/// `AWS::EC2::Volume`: an Amazon EBS volume. It is not a VPC-network type, but its identity
/// is read exactly the same way — `CreateVolume` takes no name, Amazon EC2 assigns the
/// `vol-…` identifier and "Ref returns the volume ID" — so a declaration of one carries an
/// identifier of that form rather than a generated name, and
/// [`identity_is_service_assigned`] names the type because the identifier in force is the
/// one that service answered its create with.
pub const EC2_VOLUME: &str = "AWS::EC2::Volume";

/// `AWS::EC2::LaunchTemplate`: a launch template of Amazon EC2. Its identity is read the
/// same way the VPC-network types' are: `CreateLaunchTemplate` takes the template's NAME and
/// assigns the `lt-…` identifier itself, and that type's "Return values" publishes "Ref
/// returns the launch template ID" beside the `LaunchTemplateId`, `LatestVersionNumber` and
/// `DefaultVersionNumber` attributes — so a declaration of one carries an identifier of that
/// form rather than the name, and [`identity_is_service_assigned`] names the type because
/// the identifier in force is the one EC2 answered the create with.
pub const EC2_LAUNCH_TEMPLATE: &str = "AWS::EC2::LaunchTemplate";

/// The version a launch template's FIRST version carries, which is the one
/// `CreateLaunchTemplate` creates: version numbering starts at 1 and "you can't specify,
/// change, or replace the numbering of launch template versions"
/// (`CreateLaunchTemplateVersion`). It is the stand-in both version attributes carry before
/// the create — the deployment re-points them at the numbers EC2 answered
/// ([`super::provisioning::repoint_references`]).
pub const FIRST_LAUNCH_TEMPLATE_VERSION: &str = "1";

/// `AWS::AutoScaling::AutoScalingGroup`: an Auto Scaling group of Amazon EC2 Auto Scaling.
/// "Ref returns the name of the Auto Scaling group", which is the name the declaration states
/// or the one CloudFormation generated for it — the physical id the naming rules already
/// compute — so the type carries no arm in [`return_values`] and is not in
/// [`identity_is_service_assigned`]: the group is CREATED under that name
/// ([`super::provisioner`]).
pub const AUTOSCALING_GROUP: &str = "AWS::AutoScaling::AutoScalingGroup";

/// `AWS::EC2::Instance`: an Amazon EC2 instance. Its identity is read the same way again —
/// `RunInstances` takes no name, Amazon EC2 assigns the `i-…` identifier and "Ref returns the
/// ID of the instance" — so a declaration of one carries an identifier of that form rather
/// than a generated name, and [`identity_is_service_assigned`] names the type because the
/// identifier in force is the one that service answered the launch with
/// (`crate::state::provisioning`'s instance provider).
pub const EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// `AWS::EC2::SpotFleet`: a Spot Fleet request of Amazon EC2. Its identity is read the same way
/// again — `RequestSpotFleet` takes no identifier, Amazon EC2 assigns the `sfr-…` identifier and
/// "Ref returns the Spot Fleet request ID" — so a declaration of one carries an identifier of
/// that form rather than a generated name, and [`identity_is_service_assigned`] names the type
/// because the identifier in force is the one that service answered the request with
/// (`crate::state::provisioning`'s Spot Fleet provider). Its one `Fn::GetAtt` attribute ("Id:
/// the ID of the Spot Fleet") is that same identifier.
pub const EC2_SPOT_FLEET: &str = "AWS::EC2::SpotFleet";

/// The identifier Amazon EC2 assigns a declared resource of one of the VPC-network types
/// above: `<prefix>-` plus [`EC2_ID_HEX_DIGITS`] lowercase hexadecimal characters, derived
/// from the resource's name ([`generated_service_id`]) so it is stable across reads and
/// across a re-deployment of the same declaration.
///
/// `identifier_attribute` is the `Fn::GetAtt` attribute that type publishes FOR that
/// identifier (`VpcId`, `SubnetId`, `GroupId`, `Id`, …), recorded with the same value — so
/// the id a `Ref` answers and the id that attribute answers cannot disagree. A published
/// attribute that is the PROPERTY the template declared (a VPC's `CidrBlock`, a subnet's
/// `AvailabilityZone` and `VpcId`) is attached from the declaration itself
/// ([`attach_attributes`]), and one that is ANOTHER EC2 resource's identifier (a VPC's
/// `DefaultSecurityGroup` and `DefaultNetworkAcl`) is left out: nothing here holds that
/// resource, and reporting an id for one that does not exist would be worse than reporting
/// none.
pub(super) fn ec2_network_identity(
    prefix: &str,
    identifier_attribute: &str,
    name: &str,
    attributes: &mut BTreeMap<String, String>,
) -> String {
    let id = generated_service_id(prefix, name, EC2_ID_HEX_DIGITS);
    attributes.insert(identifier_attribute.to_string(), id.clone());
    id
}

/// The `AWS::ECS::Service` resource type: a service running on an ECS cluster.
///
/// `pub(crate)` because the type's own NAME is read outside this module as well as in it: the
/// provisioner that CREATES the service in Amazon ECS keys its catalogue entry by it
/// ([`crate::state::provisioner::ecs_services`]).
pub(crate) const ECS_SERVICE: &str = "AWS::ECS::Service";

/// The two AWS Lambda types whose published identifier is the FUNCTION's ARN with a
/// qualifier: a published version's carries the version number, an alias's the alias name.
/// Both are therefore resolved once the function the declaration names has an identity,
/// which is what [`identity_depends_on_another_resource`] names them for.
pub(super) const LAMBDA_VERSION: &str = "AWS::Lambda::Version";
pub(super) const LAMBDA_ALIAS: &str = "AWS::Lambda::Alias";

/// The ARN of the function a version's or an alias's declaration names, from its
/// `FunctionName` property: "the name or ARN of the Lambda function".
///
/// The property states either, and an alias and a version are addressed by both — so the
/// value is used as the ARN where it already is one, and the function's ARN is built from
/// the name where it is a name, which is what a `Ref` to a function of the same template
/// resolves to ("Ref returns the function name"). `None` for a declaration that names no
/// function, or names one from something this module cannot resolve.
fn lambda_function_arn(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
    region: &str,
) -> Option<String> {
    let reference = properties
        .get("FunctionName")
        .and_then(|value| text(value, scope, 0, budget))
        .filter(|reference| !reference.is_empty())?;
    if reference.starts_with("arn:") {
        return Some(reference);
    }
    Some(::emulator_server::emulator_arn(
        "lambda",
        region,
        &format!("function:{reference}"),
    ))
}

/// An Elastic Load Balancing v2 listener's and listener rule's resource types. Each one's
/// published identifier NESTS inside its parent's ARN, which is what [`nested_elb_arn`]
/// builds and why [`identity_depends_on_another_resource`] names them.
pub const ELB_LISTENER: &str = "AWS::ElasticLoadBalancingV2::Listener";
pub const ELB_LISTENER_RULE: &str = "AWS::ElasticLoadBalancingV2::ListenerRule";

/// The same service's TRUST STORE: the set of CA certificates an HTTPS listener
/// authenticates client certificates against, whose own ARN carries the identifier that
/// service mints for it (`truststore/<name>/<id>`) and is what the type's `Ref` returns.
pub const ELB_TRUST_STORE: &str = "AWS::ElasticLoadBalancingV2::TrustStore";

/// The salts a listener's, a rule's and a trust store's assigned ids are derived under,
/// beside the load balancer's and the target group's (see [`service_assigned_label`]).
pub(super) const ELB_LISTENER_ID_SALT: &str = "elasticloadbalancing/listener-id";
pub(super) const ELB_LISTENER_RULE_ID_SALT: &str = "elasticloadbalancing/listener-rule-id";
pub(super) const ELB_TRUST_STORE_ID_SALT: &str = "elasticloadbalancing/truststore-id";

/// The ARN resource segment a trust store's own ARN carries ahead of its name and the id
/// that service assigned it (`truststore/<name>/<id>`).
pub(super) const ELB_TRUST_STORE_SEGMENT: &str = "truststore";

/// The ARN resource segment of each Elastic Load Balancing v2 resource kind, which is what
/// a nested ARN replaces: a listener's is `listener/app/<lb>/<lb id>/<listener id>` and a
/// rule's `listener-rule/app/<lb>/<lb id>/<listener id>/<rule id>`.
pub(super) const ELB_LOAD_BALANCER_SEGMENT: &str = "loadbalancer";
pub(super) const ELB_LISTENER_SEGMENT: &str = "listener";
pub(super) const ELB_LISTENER_RULE_SEGMENT: &str = "listener-rule";

/// The cluster an ECS service belongs to when its declaration names none. Every ECS
/// `cluster` member documents it: "If you do not specify a cluster, the default cluster is
/// assumed", and that cluster is named `default`.
pub(super) const ECS_DEFAULT_CLUSTER: &str = "default";

/// The ARN of a resource Elastic Load Balancing nests inside another: `parent`'s ARN with
/// `segment` in place of `parent_segment` and `id` appended, which is the form a
/// listener's ARN takes inside its load balancer's and a rule's inside its listener's.
///
/// `None` where `parent` is not an Elastic Load Balancing ARN of the expected kind — the
/// declaration names no parent, or names one this emulator could not resolve — so the
/// nested resource keeps the identity CloudFormation's naming rules gave it rather than
/// one built on a value that is not an ARN.
pub(super) fn nested_elb_arn(
    parent: &str,
    parent_segment: &str,
    segment: &str,
    id: &str,
) -> Option<String> {
    let marker = format!(":{parent_segment}/");
    if !parent.starts_with(&format!("arn:{PARTITION}:elasticloadbalancing:"))
        || !parent.contains(&marker)
    {
        return None;
    }
    Some(format!(
        "{}/{id}",
        parent.replacen(&marker, &format!(":{segment}/"), 1)
    ))
}

/// The cluster NAME an ECS resource's `Cluster` property names: every such member takes
/// "the short name or full Amazon Resource Name (ARN) of the cluster", and a cluster ARN's
/// resource portion is `cluster/<name>`.
pub(super) fn ecs_cluster_name(reference: &str) -> String {
    match reference.rsplit_once(":cluster/") {
        Some((_, name)) => name.to_string(),
        None => reference.to_string(),
    }
}

/// The vector bucket an `AWS::S3Vectors::Index` declaration holds its index in: the
/// `VectorBucketName` it names, or the bucket its `VectorBucketArn` names.
///
/// Those are the two ways Amazon S3 Vectors' own API addresses a vector bucket — "you can
/// either use both the vector bucket name and the index name, or you can use the vector
/// index ARN" is how its reads put it, and `CreateIndex` takes `vectorBucketName` or
/// `vectorBucketArn` — so a declaration spelling either names the same bucket.
///
/// `None` where the declaration names neither, or names one this module could not evaluate:
/// there is then no bucket to nest the index's ARN inside.
pub(super) fn declared_vector_bucket(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Option<String> {
    let named = properties
        .get("VectorBucketName")
        .and_then(|value| text(value, scope, 0, budget))
        .filter(|name| !name.is_empty());
    if named.is_some() {
        return named;
    }
    properties
        .get("VectorBucketArn")
        .and_then(|value| text(value, scope, 0, budget))
        .as_deref()
        .and_then(vector_bucket_of_arn)
}

/// The bucket name an Amazon S3 Vectors vector-bucket ARN carries:
/// `arn:aws:s3vectors:<region>:<account>:bucket/<name>` names `<name>`.
///
/// `None` for anything that is not one — the resource portion past the fifth `:` has to be
/// exactly one `bucket/<name>` segment, which is what that model's `VectorBucketArn`
/// pattern fixes — so an index's ARN is never nested inside a value that is not a vector
/// bucket's.
pub(super) fn vector_bucket_of_arn(arn: &str) -> Option<String> {
    let bucket = arn
        .splitn(6, ':')
        .nth(5)?
        .strip_prefix(S3VECTORS_BUCKET_SEGMENT)?;
    (!bucket.is_empty() && !bucket.contains('/')).then(|| bucket.to_string())
}

/// A service-assigned identifier of the canonical UUID form, derived from `name` under
/// `domain`.
///
/// Some resource types report an identifier their owning service generates as a UUID (a
/// CloudWatch RUM app monitor's `Id`). The 16 bytes come from a SHA-256 digest of the
/// pair, with byte 6's high nibble set to `4` (the version) and byte 8's two high bits to
/// `10` (the RFC 4122 variant), exactly the layout a version-4 generator writes. It is a
/// function of the resource's own name — itself derived from the stack id and the logical
/// id — so the id is stable across reads and across a later update of the same stack, and
/// two resources of one stack never collide.
pub(super) fn generated_uuid(domain: &str, name: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

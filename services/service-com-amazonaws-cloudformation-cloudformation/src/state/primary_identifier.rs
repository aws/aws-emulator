// SPDX-License-Identifier: CC0-1.0

//! The PRIMARY IDENTIFIER of a resource type: the properties whose values identify one
//! resource of that type.
//!
//! A resource type's primary identifier is what a client has to supply to name an EXISTING
//! resource of that type — the `ResourceIdentifier` entries a `ResourceToImport` carries
//! ("the key is the name of one of the primary identifiers for the resource … the value is
//! the value of that primary identifier"). What an IMPORT SUMMARY reports for a type is the
//! neighbouring question, and [`super::import_identifiers`] owns it: that answer adds the
//! ALTERNATIVE identifiers a type publishes beside its primary one and reports nothing at all
//! for a type that identifies no resource outside the stack, while this module always answers,
//! because every resource this emulator has READ is identified by something.
//!
//! It is decided from the TYPE alone, since a summary is given for a template whose
//! resources nothing has created yet. The reading is the one
//! [`super::resource_scans::primary_identifier`] gives a resource it has READ, in the same
//! order of preference:
//!
//! 1. the parts of a COMPOSITE primary identifier, for a type whose resource IS a binding
//!    between others ([`super::template::composite_identity_properties`]);
//! 2. the published return value that carries the resource's IDENTITY, for a type whose
//!    `Ref` returns something other than the resource's name ([`identifying_return_value`]);
//! 3. otherwise the property the type takes its NAME from
//!    ([`super::template::name_property_candidates`]), because a physical id that IS the
//!    resource's name is the value of that property.
//!
//! Step 3 always answers, so every resource type has a primary identifier to report: a type
//! whose name property is a convention of the type's own name is identified by that
//! property, which is how the majority of CloudFormation types are identified.

/// The properties whose values identify one resource of `resource_type`, in the order a
/// summary reports them.
pub fn properties(resource_type: &str) -> Vec<String> {
    let recorded = recorded_identity(resource_type);
    if !recorded.is_empty() {
        return recorded;
    }
    super::template::name_property_candidates(resource_type)
        .into_iter()
        .take(1)
        .collect()
}

/// The properties this emulator RECORDS as a type's identity, which is what the resource
/// provider schema the registry serves declares as its `primaryIdentifier`: the parts of a
/// COMPOSITE identifier ([`super::template::composite_identity_properties`]), or the published
/// return value that carries the resource's identity ([`identifying_return_value`]); the EMPTY
/// list for a type whose identity is its own NAME, which is a convention rather than a record.
///
/// The ONE reading of that question, so the three answers computed from it cannot drift apart:
/// the identity a resource this emulator has READ is reported under ([`properties`]), the
/// `primaryIdentifier` a type's schema declares ([`super::resource_schema`]) and the identifier
/// set an IMPORT takes ([`super::import_identifiers`]) all start here.
pub(crate) fn recorded_identity(resource_type: &str) -> Vec<String> {
    let composite = super::template::composite_identity_properties(resource_type);
    if !composite.is_empty() {
        return composite.iter().map(|part| (*part).to_string()).collect();
    }
    identifying_return_value(resource_type)
        .map(|published| vec![published.to_string()])
        .unwrap_or_default()
}

/// The published return value a resource of `resource_type` is identified BY, for a type
/// whose `Ref` returns an ARN, a URL or an identifier the owning service assigns rather than
/// the resource's own name; `None` for a type whose identity is that name.
///
/// Source: each type's documented "Return values" section — the same sentences
/// [`super::template`] computes a resource's identity and attributes from ("Ref returns the
/// topic ARN" for `AWS::SNS::Topic`, "Ref returns the queue URL" for `AWS::SQS::Queue`, "Ref
/// returns the key ID" for `AWS::KMS::Key`) — read here for WHICH published member carries
/// that value, which is the member a client names the resource by.
///
/// Visible to the crate because it is a RECORDED per-type answer, which is what the resource
/// provider schema the registry serves declares as the type's `primaryIdentifier`
/// ([`super::resource_schema`]); the name convention below it is not.
pub(crate) fn identifying_return_value(resource_type: &str) -> Option<&'static str> {
    use super::template::*;

    let identifier = match resource_type {
        // "Ref returns the topic ARN"; the queue URL; the activity ARN.
        SNS_TOPIC => "TopicArn",
        SQS_QUEUE => "QueueUrl",
        STEP_FUNCTIONS_ACTIVITY => "Arn",
        // The CodeArtifact types, each identified by the ARN its create answers.
        CODEARTIFACT_DOMAIN | CODEARTIFACT_REPOSITORY => "Arn",
        // A package group of such a domain is identified by the ARN CodeArtifact minted for it
        // too ("Ref returns the Amazon Resource Name (ARN) of the package group"); its own
        // `Pattern` is what the group matches and what addresses it beside its domain, not a
        // name the type declares.
        crate::state::provisioner::codeartifact_package_groups::CODEARTIFACT_PACKAGE_GROUP => {
            "Arn"
        }
        // An Amazon Athena CAPACITY RESERVATION is identified by its own ARN, which is that
        // resource type's primary identifier and what its `Ref` returns; the reservation's
        // `Name` is what Athena's own calls take, and it is the ARN's last segment.
        crate::state::provisioning::athena_resources::ATHENA_CAPACITY_RESERVATION => "Arn",
        // The Amazon S3 Tables table bucket, identified by its own ARN — the `tableBucketARN`
        // every operation of that service on a table bucket takes.
        S3TABLES_TABLE_BUCKET => S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE,
        // The Amazon S3 Vectors types, each identified by its own ARN.
        S3VECTORS_VECTOR_BUCKET => "VectorBucketArn",
        S3VECTORS_INDEX => "IndexArn",
        // A Billing and Cost Management Dashboards dashboard, and a scheduled report on one,
        // are each identified by the ARN that service minted for the resource ("Ref returns
        // the Arn") — the handle every call on either takes; the type's own `Name` is only
        // what the resource is called, and two accounts' reports may share one.
        BCM_DASHBOARD | BCM_SCHEDULED_REPORT => "Arn",
        // Each Amazon VPC Lattice RESOURCE-ACCESS resource is identified by the ARN that
        // service minted for it ("Ref returns the Arn"), which is the handle every VPC Lattice
        // call on one takes; a gateway's or a configuration's own `Name` is only what it is
        // called, and an association has no name at all. (The service network and the access
        // log subscription are in the same position, under their own arm above.)
        VPCLATTICE_RESOURCE_GATEWAY
        | VPCLATTICE_RESOURCE_CONFIGURATION
        | VPCLATTICE_SERVICE_NETWORK_RESOURCE_ASSOCIATION
        | VPCLATTICE_SERVICE_NETWORK_VPC_ASSOCIATION => VPCLATTICE_ARN_ATTRIBUTE,
        // An Amazon Bedrock SESSION, identified by the ARN the Bedrock Agent runtime built
        // around the uuid it minted ("Ref returns the session ARN") — one of the two forms
        // every call on a session takes as its `sessionIdentifier`; the type declares no name a
        // client could name one by.
        BEDROCK_SESSION => BEDROCK_SESSION_ARN_ATTRIBUTE,
        // An APPLICATION INFERENCE PROFILE, identified by the ARN Amazon Bedrock built around
        // the id it minted ("Ref returns the inference profile identifier") — one of the two
        // forms every call on a profile takes as its `inferenceProfileIdentifier`; its own
        // `InferenceProfileName` is only what the profile is called, and no call of that
        // service addresses one by it.
        BEDROCK_APPLICATION_INFERENCE_PROFILE => BEDROCK_INFERENCE_PROFILE_ARN_ATTRIBUTE,
        // The three Amazon Bedrock DATA AUTOMATION resources, each identified by the ARN that
        // service minted for it ("Ref returns the ARN") — the handle every call of that
        // build-time plane on the resource takes. Each type's own name is only what the
        // resource is called, and no call of that service addresses one by it.
        BEDROCK_BLUEPRINT => BLUEPRINT_ARN_ATTRIBUTE,
        BEDROCK_DATA_AUTOMATION_PROJECT => PROJECT_ARN_ATTRIBUTE,
        BEDROCK_DATA_AUTOMATION_LIBRARY => LIBRARY_ARN_ATTRIBUTE,
        // An Amazon Braket spending limit, identified by the ARN that service minted for it
        // ("Ref returns the spending limit ARN"), which is the handle every spending-limit
        // API takes — the type declares no name a client could name one by.
        BRAKET_SPENDING_LIMIT => "SpendingLimitArn",
        // The four AWS End User Messaging SMS and Voice types whose identity that service
        // assigns, each identified by the identifier its create minted — the handle every
        // later call of that service on the resource takes. None of them declares a name a
        // client could name one by. (Its configuration set and opt-out list are identified
        // by their own names, and a resource policy by the ARN it is attached to, which is a
        // declared property rather than a published return value.)
        SMSVOICE_PROTECT_CONFIGURATION => SMSVOICE_PROTECT_CONFIGURATION_ID_ATTRIBUTE,
        SMSVOICE_RCS_AGENT => SMSVOICE_RCS_AGENT_ID_ATTRIBUTE,
        SMSVOICE_REGISTRATION => SMSVOICE_REGISTRATION_ID_ATTRIBUTE,
        SMSVOICE_REGISTRATION_ATTACHMENT => SMSVOICE_REGISTRATION_ATTACHMENT_ID_ATTRIBUTE,
        // An Amazon CloudWatch investigations investigation group, identified by the ARN that
        // service minted for it ("Ref returns the Arn"), which is the `identifier` every
        // investigation-group API takes; its own `Name` is only what the group is called, and
        // no ARN a template states could be it.
        AIOPS_INVESTIGATION_GROUP => "Arn",
        // An Amazon GameLift Servers build and a Realtime script, each identified by the id
        // that service minted for it ("Ref returns the build ID" / "the script ID") — the
        // `BuildId`/`ScriptId` every call on one takes; each type's own `Name` is only what
        // the resource is called, and "build names do not need to be unique".
        GAMELIFT_BUILD => GAMELIFT_BUILD_ID_ATTRIBUTE,
        GAMELIFT_SCRIPT => GAMELIFT_ID_ATTRIBUTE,
        // An Amazon Lex V2 BOT is identified by the `Id` that service minted for it ("Ref
        // returns the bot Id") — the `botId` every bot-addressed call takes; its `Name` is only
        // what the bot is called. (A version and an alias are identified by the PAIR each of
        // their schemas declares, which is a composite row of
        // [`super::template::composite_identity_properties`] rather than a published value.)
        crate::state::provisioner::lex_bots::LEX_BOT => "Id",
        // An AWS HealthLake FHIR data store is identified by the `DatastoreId` that service        // minted for it ("Ref returns the DatastoreId") — the handle every data-store API
        // takes; its `DatastoreName` is only what the store is called, and the type does not
        // require one at all. A DATA TRANSFORMATION PROFILE is identified by the ARN built
        // around the id that service minted ("Ref returns the profile ARN"), which is the
        // handle its tagging surface addresses one by.
        HEALTHLAKE_FHIR_DATASTORE => "DatastoreId",
        HEALTHLAKE_DATA_TRANSFORMATION_PROFILE => "ProfileArn",
        // The two Amazon VPC Lattice types, each identified by the ARN that service minted for
        // the resource ("Ref returns the ARN") — one of the two forms every call of that
        // service on the resource takes as its identifier. A network's own `Name` is only what
        // it is called, and a subscription declares no name at all.
        VPCLATTICE_SERVICE_NETWORK | VPCLATTICE_ACCESS_LOG_SUBSCRIPTION => "Arn",
        // An Application Signals service level objective is identified by the ARN that type's
        // `Ref` returns — the one every SLO operation takes as its `Id`. A GROUPING
        // CONFIGURATION is identified by the ACCOUNT whose configuration it is: there is one per
        // account and Region, its put takes no identifier at all and answers a record carrying
        // no account member, so the account the call was signed for is what names one. (An
        // INSTRUMENTATION CONFIG is not here: its primary identifier is the COMPOSITE
        // [`super::template::composite_identity_properties`] records, which the step above
        // answers.)
        APPLICATION_SIGNALS_SERVICE_LEVEL_OBJECTIVE => "Arn",
        APPLICATION_SIGNALS_GROUPING_CONFIGURATION => APPLICATION_SIGNALS_ACCOUNT_ID_ATTRIBUTE,
        // The private Amazon ECR registry's own signing configuration, identified by the
        // REGISTRY whose configuration it is: a registry holds one, neither the put nor the
        // delete takes an identifier at all, and `registryId` is the one member the calls that
        // report one answer it against.
        ECR_SIGNING_CONFIGURATION => ECR_REGISTRY_ID_ATTRIBUTE,
        // An Application Insights application, identified by the ARN that type's `Ref` returns —
        // the `ApplicationARN` a client names one by — rather than by a name of its own, which
        // the type does not declare.
        APPLICATION_INSIGHTS_APPLICATION => APPLICATION_INSIGHTS_APPLICATION_ARN_ATTRIBUTE,
        // A Clean Rooms ML training dataset, identified by the ARN that service minted for it
        // ("Ref returns the training dataset ARN"), which is the `trainingDatasetArn` every
        // call on one takes; the type's own `Name` is only what the dataset is called.
        CLEANROOMSML_TRAINING_DATASET => CLEANROOMSML_TRAINING_DATASET_ARN_ATTRIBUTE,
        // A Quick Setup configuration manager, identified by the ARN that service minted for
        // it ("Ref returns the ManagerArn"), which is the `ManagerArn` every call on one
        // takes; the type's own `Name` is only what the manager is called, and two of them
        // may share one.
        SSM_QUICKSETUP_CONFIGURATION_MANAGER => {
            SSM_QUICKSETUP_CONFIGURATION_MANAGER_ARN_ATTRIBUTE
        }
        // A CloudHSM cluster, identified by the ARN built around the `ClusterId` that service
        // minted ("Ref returns the cluster ARN"), which the type publishes as its `Arn`; the
        // type declares no name a client could name one by.
        CLOUDHSM_CLUSTER => CLOUDHSM_CLUSTER_ARN_ATTRIBUTE,
        // A Backup Gateway hypervisor, identified by the ARN that service minted for it ("Ref
        // returns the hypervisor ARN") — the `ServerArn` every call on a hypervisor takes; the
        // type's own `Name` is only what the hypervisor is called.
        BACKUP_GATEWAY_HYPERVISOR => "HypervisorArn",
        // An IAM Access Analyzer analyzer, identified by the ARN that service records for it
        // ("Ref returns the ARN of the analyzer"), and an ARCHIVE RULE of one by the identity
        // built around that ARN and the rule's own name. Either type's own name property is
        // what the resource is CALLED inside that service; the published identity is the ARN,
        // which is what a client naming an existing resource of the type supplies.
        access_analyzer::ACCESSANALYZER_ANALYZER
        | access_analyzer::ACCESSANALYZER_ARCHIVE_RULE => {
            crate::state::provisioner::access_analyzer::ACCESS_ANALYZER_ARN_ATTRIBUTE
        }
        // An IoT Core Device Advisor suite definition, identified by the id that service minted
        // for it ("Ref returns the suite definition id") — the `suiteDefinitionId` every call on
        // one takes; the suite's own name lives inside its configuration and two accounts' suites
        // may share one.
        IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION => {
            IOT_CORE_DEVICE_ADVISOR_SUITE_DEFINITION_ID_ATTRIBUTE
        }
        // A Recycle Bin retention rule, identified by the ARN that service built around the
        // identifier it minted ("Ref returns the rule ARN") — the type declares no name a
        // client could name one by.
        RBIN_RULE => "Arn",
        // A Direct Connect gateway and a resiliency group are each identified by the
        // identifier AWS Direct Connect minted for them — the `directConnectGatewayId` and the
        // `resiliencyGroupId` every call of that service on the resource takes — which is the
        // value inside the ARN that type's `Ref` returns. The resource's own name is only what
        // it is called, and two of them may share one.
        DIRECT_CONNECT_GATEWAY => "DirectConnectGatewayId",
        DIRECT_CONNECT_RESILIENCY_GROUP => "ResiliencyGroupId",
        // Each AWS Network Security Manager entity, identified by the ARN that service built
        // around the id it minted — the `…Identifier` every call on the entity takes, which is
        // what its ARN member's own `resourceIdentifier` declares. The entity's own name is
        // only what it is called.
        NETWORK_SECURITY_MANAGER_RULE => "RuleArn",
        NETWORK_SECURITY_MANAGER_TEMPLATE => "TemplateArn",
        NETWORK_SECURITY_MANAGER_POLICY => "PolicyArn",
        NETWORK_SECURITY_MANAGER_SCOPE => "ScopeArn",
        // A Neptune Analytics graph, identified by the id that service minted for it ("Ref
        // returns the graph id"), which is the `graphIdentifier` every call on a graph takes,
        // and a SNAPSHOT of one by the id it minted for the snapshot, which is the
        // `snapshotIdentifier` every call on one takes.
        NEPTUNE_GRAPH_GRAPH => "GraphId",
        NEPTUNE_GRAPH_GRAPH_SNAPSHOT => "Id",
        // A Simple AD directory, identified by the `DirectoryId` Directory Service minted for
        // it ("Ref returns the directory ID") — the handle every operation on a directory
        // takes, which this type publishes as a return value of its own. (An
        // `AWS::DirectoryService::MicrosoftAD` needs no row: it publishes no return value
        // carrying that id, and this answer can only name one the type publishes.)
        DIRECTORY_SERVICE_SIMPLE_AD => "DirectoryId",
        // An AWS Amplify app, identified by the ARN that service built around the id it minted
        // ("Ref returns the ARN of the app"), which is the type's primary identifier. (The
        // three Amplify UI Builder entities need no row: each is identified by the app, the
        // environment and the id its own resource shape names, which step 1 above answers from
        // [`super::template::composite_identity_properties`].)
        AMPLIFY_APP => "Arn",
        // An AWS Amplify BRANCH, identified by the ARN that service built around the app's own
        // ("Ref returns the ARN of the branch"), which is the type's primary identifier.
        AMPLIFY_BRANCH => "Arn",
        // The CloudFront types, each identified by the id that service assigns.
        CLOUDFRONT_DISTRIBUTION
        | CLOUDFRONT_ORIGIN_ACCESS_CONTROL
        | CLOUDFRONT_ORIGIN_ACCESS_IDENTITY
        | CLOUDFRONT_CACHE_POLICY
        | CLOUDFRONT_ORIGIN_REQUEST_POLICY
        | CLOUDFRONT_RESPONSE_HEADERS_POLICY
        | CLOUDFRONT_PUBLIC_KEY
        | CLOUDFRONT_KEY_GROUP
        | CLOUDFRONT_CONNECTION_GROUP
        // A VPC origin is identified by the `vo_`-prefixed id that service mints too.
        | CLOUDFRONT_VPC_ORIGIN => "Id",
        // A key value store is identified by the ARN its own data plane addresses it by, a
        // connection function by the ARN CloudFront built around the id it minted, and a
        // function by the ARN a cache behaviour names it by. A real-time log configuration is
        // in that same position: the ARN CloudFront minted for it is what a cache behaviour's
        // `RealtimeLogConfigArn` names one by and what its own `Ref` returns.
        CLOUDFRONT_KEY_VALUE_STORE | CLOUDFRONT_REALTIME_LOG_CONFIG => "Arn",
        CLOUDFRONT_CONNECTION_FUNCTION => "ConnectionFunctionArn",
        CLOUDFRONT_FUNCTION => "FunctionARN",
        // The API Gateway types, each identified by the id that service assigns. An api and
        // an api mapping publish their ids under the member their own control plane names
        // them by.
        APIGATEWAY_REST_API => "RestApiId",
        APIGATEWAY_RESOURCE => "ResourceId",
        APIGATEWAY_DEPLOYMENT => "DeploymentId",
        // The four types that govern access to a REST API: a key, an authorizer and a plan
        // are each identified by the id their own create minted, and an association by the
        // id of the key it put in the plan.
        APIGATEWAY_API_KEY => "APIKeyId",
        APIGATEWAY_AUTHORIZER => "AuthorizerId",
        // A method's request validator, identified by the id its own create minted. (A
        // model needs no row: it is identified by the `Name` it is keyed under, which step 3
        // above answers from the conventional name property.)
        APIGATEWAY_REQUEST_VALIDATOR => "RequestValidatorId",
        // A CLIENT CERTIFICATE, identified by the `ClientCertificateId` API Gateway minted —
        // "Ref returns the client certificate ID", the handle every call on one takes. The
        // type declares no name a client could name one by.
        APIGATEWAY_CLIENT_CERTIFICATE => "ClientCertificateId",
        APIGATEWAY_USAGE_PLAN | APIGATEWAY_USAGE_PLAN_KEY => "Id",
        // The account settings of the REST control plane, identified by the id the resource
        // publishes as `Id`: the settings are a singleton, so the type declares no name a
        // client could name one by.
        APIGATEWAY_ACCOUNT => "Id",
        APIGATEWAYV2_API => "ApiId",
        APIGATEWAYV2_API_MAPPING => "ApiMappingId",
        // A ROUTING RULE of a custom domain name, identified by the ARN API Gateway v2 built
        // for it — "Ref returns the routing rule ARN", the value that carries both halves
        // every call on a rule takes (the domain name and the rule's own id).
        APIGATEWAYV2_ROUTING_RULE => crate::state::provisioner::ROUTING_RULE_ARN_ATTRIBUTE,
        // The AWS Transfer Family types, each identified by the ARN its create answers.
        TRANSFER_SERVER => "Arn",
        // The Amazon EKS association, the Batch environments, queues and job definitions, and
        // a task definition: each one's `Ref` is the ARN its create answered.
        EKS_POD_IDENTITY_ASSOCIATION => "AssociationArn",
        // A State Manager association, identified by the `AssociationId` Systems Manager
        // minted — "Ref returns the association ID", the handle every call on one takes.
        SSM_ASSOCIATION => SSM_ASSOCIATION_ID_ATTRIBUTE,
        // A patch baseline, identified by the `BaselineId` Systems Manager minted — "Ref
        // returns the patch baseline ID", which that type publishes as its `Id`.
        crate::state::provisioning::ssm_patch_baseline::SSM_PATCH_BASELINE => {
            crate::state::template::SSM_PATCH_BASELINE_ID_ATTRIBUTE
        }
        BATCH_COMPUTE_ENVIRONMENT => "ComputeEnvironmentArn",
        BATCH_JOB_QUEUE => "JobQueueArn",
        BATCH_JOB_DEFINITION => "JobDefinitionArn",
        BATCH_SERVICE_ENVIRONMENT => "ServiceEnvironmentArn",
        // The same service's SHARING plane: a scheduling policy is identified by the ARN its
        // create answered — the `arn` every call on a policy takes, and what a job queue names
        // one by — and a quota share by the ARN that service composed under its queue, the one
        // identifier every call on a share takes.
        BATCH_SCHEDULING_POLICY => crate::state::provisioner::SCHEDULING_POLICY_ARN_ATTRIBUTE,
        BATCH_QUOTA_SHARE => crate::state::provisioner::QUOTA_SHARE_ARN_ATTRIBUTE,
        ECS_TASK_DEFINITION => "TaskDefinitionArn",
        // The three Amazon EFS types, each identified by the identifier that service minted:
        // the `fs-` id every call naming a file system takes, the `fsap-` id of an access
        // point, and the `fsmt-` id a mount target publishes as its `Id`.
        crate::state::provisioning::efs_resources::EFS_FILE_SYSTEM => {
            crate::state::provisioning::efs_resources::FILE_SYSTEM_ID_ATTRIBUTE
        }
        crate::state::provisioning::efs_resources::EFS_ACCESS_POINT => {
            crate::state::provisioning::efs_resources::ACCESS_POINT_ID_ATTRIBUTE
        }
        crate::state::provisioning::efs_resources::EFS_MOUNT_TARGET => {
            crate::state::provisioning::efs_resources::ID_ATTRIBUTE
        }
        // A SageMaker AI Studio DOMAIN, identified by the `d-…` `DomainId` that service minted
        // ("Ref returns the domain ID") — the handle every call on the domain and on a member of
        // it takes; the domain's own `DomainName` is only what it is called. (Its two MEMBER
        // types are identified by the PAIR each of their APIs addresses one by, which
        // [`super::template::composite_identity_properties`] records.)
        crate::state::provisioning::sagemaker_studio::SAGEMAKER_DOMAIN => "DomainId",
        // The Elastic Load Balancing v2 types: a load balancer, a target group, a listener,
        // a listener rule and a trust store are each identified by their own ARN.
        ELB_LOAD_BALANCER => "LoadBalancerArn",
        ELB_TARGET_GROUP => "TargetGroupArn",
        "AWS::ElasticLoadBalancingV2::Listener" => "ListenerArn",
        "AWS::ElasticLoadBalancingV2::ListenerRule" => "RuleArn",
        crate::state::template::ELB_TRUST_STORE => "TrustStoreArn",
        // An Amazon ECS service and cluster: "Ref returns the Amazon Resource Name (ARN)".
        "AWS::ECS::Service" | ECS_CLUSTER => "Arn",
        // The Amazon EC2 network types, each identified by the id Amazon EC2 assigns — under
        // the member that type publishes it as.
        EC2_VPC => "VpcId",
        EC2_SUBNET => "SubnetId",
        EC2_INTERNET_GATEWAY => "InternetGatewayId",
        EC2_ROUTE_TABLE => "RouteTableId",
        EC2_NAT_GATEWAY => "NatGatewayId",
        EC2_DHCP_OPTIONS => "DhcpOptionsId",
        EC2_VPN_GATEWAY => "VPNGatewayId",
        EC2_VOLUME => "VolumeId",
        EC2_SECURITY_GROUP | EC2_NETWORK_ACL | EC2_NETWORK_INTERFACE | EC2_VPC_ENDPOINT => "Id",
        // The Amazon Bedrock AgentCore types, each identified by the id its create mints.
        "AWS::BedrockAgentCore::Memory" => "MemoryId",
        "AWS::BedrockAgentCore::Runtime" => "AgentRuntimeId",
        "AWS::BedrockAgentCore::CodeInterpreter" => "CodeInterpreterId",
        "AWS::BedrockAgentCore::Browser" => "BrowserId",
        "AWS::BedrockAgentCore::Gateway" => "GatewayId",
        // A gateway TARGET is identified by the PAIR every call on one binds: the gateway
        // and the target id, which is the id this type publishes.
        crate::state::provisioner::BEDROCK_AGENTCORE_GATEWAY_TARGET => {
            crate::state::provisioner::bedrock_agentcore_gateways::GATEWAY_TARGET_ID_ATTRIBUTE
        }
        // A secret is identified by its ARN, which that type publishes as `Id`.
        "AWS::SecretsManager::Secret" => "Id",
        // The types whose own service assigns an opaque id: an experiment template, a
        // GuardDuty detector and IP set, a hosted zone, a health check, a KMS key, an
        // Amazon MQ broker and either kind of Amazon MSK cluster.
        FIS_EXPERIMENT_TEMPLATE | GUARDDUTY_DETECTOR | GUARDDUTY_IP_SET | ROUTE53_HOSTED_ZONE => {
            "Id"
        }
        ROUTE53_HEALTH_CHECK => "HealthCheckId",
        KMS_KEY => "KeyId",
        MQ_BROKER => "Id",
        // A configuration of the same service, identified by the `Id` Amazon MQ minted — the
        // `ConfigurationId` label `DescribeConfiguration`, `UpdateConfiguration` and
        // `DeleteConfiguration` each take, which this type publishes as a return value of its
        // own; the configuration's `Name` is only what it is called.
        MQ_CONFIGURATION => "Id",
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => "Arn",
        // A configuration of the same service, identified by the ARN its create answered —
        // the `Arn` label `DescribeConfiguration`, `UpdateConfiguration` and
        // `DeleteConfiguration` each take; the type's own `Name` is only what the
        // configuration is called.
        MSK_CONFIGURATION => "Arn",
        // A private CA and a certificate it issued are each identified by the ARN that
        // service minted, and an activation by the ARN of the CA it activates.
        ACMPCA_CERTIFICATE_AUTHORITY | ACMPCA_CERTIFICATE => "Arn",
        ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION => "CertificateAuthorityArn",
        _ => return None,
    };
    Some(identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every type this emulator provisions has a primary identifier to report, and every
    /// part of it is a named property.
    #[test]
    fn every_resource_type_has_a_primary_identifier() {
        for resource_type in super::super::template::IMPLEMENTED_RESOURCE_TYPES {
            let identifiers = properties(resource_type);
            assert!(
                !identifiers.is_empty(),
                "case {resource_type:?}: expected an identifier property, got {identifiers:?}"
            );
            assert!(
                identifiers.iter().all(|property| !property.is_empty()),
                "case {resource_type:?}: expected named properties, got {identifiers:?}"
            );
        }
    }

    /// A type whose primary identifier is a PUBLISHED return value names it by the member
    /// that type publishes, so the catalogue here and the return-value catalogue in
    /// [`super::super::template::published_attribute_names`] cannot drift apart.
    ///
    /// The exceptions are the types whose identifier member is not among the values
    /// `Fn::GetAtt` reads off them: an api and a load balancer are identified by an id and an
    /// ARN their own control plane names them by and that the type publishes no return value
    /// for, a security group, an Amazon MQ broker and a GuardDuty-style `Id` likewise, an
    /// activation is identified by the ARN of the CA it activates (a PROPERTY of the
    /// declaration), and an Amazon ECS service by the ARN the uniform convention gives it
    /// rather than by a published value.
    #[test]
    fn a_published_identifier_is_one_of_the_types_published_return_values() {
        const NOT_PUBLISHED: &[&str] = &[
            "AWS::ApiGatewayV2::Api",
            "AWS::ElasticLoadBalancingV2::LoadBalancer",
            "AWS::EC2::SecurityGroup",
            "AWS::AmazonMQ::Broker",
            "AWS::ACMPCA::CertificateAuthorityActivation",
            "AWS::ECS::Service",
        ];
        for resource_type in super::super::template::IMPLEMENTED_RESOURCE_TYPES {
            let Some(identifier) = identifying_return_value(resource_type) else {
                continue;
            };
            if NOT_PUBLISHED.contains(resource_type) {
                continue;
            }
            let published = super::super::template::published_attribute_names(resource_type);
            assert!(
                published.contains(&identifier),
                "case {resource_type:?}: expected the identifier {identifier:?} to be one of \
                 the type's published return values {published:?}"
            );
        }
    }

    /// A type whose resource IS a binding is identified by the parts of that binding, which
    /// is more than one property — and so is a type whose identifier is composite for another
    /// reason: each WAFv2 ENTITY type is identified by its name, the id AWS WAF assigned and
    /// the scope it lives in.
    #[test]
    fn a_binding_is_identified_by_every_part_of_its_composite_identifier() {
        for resource_type in [
            "AWS::WAFv2::WebACLAssociation",
            "AWS::EKS::AccessEntry",
            "AWS::WAFv2::WebACL",
            "AWS::WAFv2::RuleGroup",
            "AWS::WAFv2::IPSet",
            "AWS::WAFv2::RegexPatternSet",
        ] {
            let identifiers = properties(resource_type);
            assert_eq!(
                identifiers,
                super::super::template::composite_identity_properties(resource_type)
                    .iter()
                    .map(|part| (*part).to_string())
                    .collect::<Vec<String>>(),
                "case {resource_type:?}: expected the parts of the composite identifier, \
                 got {identifiers:?}"
            );
        }
    }

    /// A type this emulator holds no catalogue entry for is identified by the name property
    /// CloudFormation's convention gives it, rather than by nothing at all.
    #[test]
    fn an_uncatalogued_type_is_identified_by_its_conventional_name_property() {
        let identifiers = properties("AWS::NotAService::NotAType");
        assert_eq!(
            identifiers,
            vec!["NotATypeName".to_string()],
            "case \"a type no catalogue entry carries\": expected the conventional name \
             property, got {identifiers:?}"
        );
    }
}

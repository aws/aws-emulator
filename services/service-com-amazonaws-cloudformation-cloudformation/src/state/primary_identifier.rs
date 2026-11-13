// SPDX-License-Identifier: CC0-1.0

//! The PRIMARY IDENTIFIER of a resource type: the properties whose values identify one
//! resource of that type.
//!
//! A resource type's primary identifier is what a client has to supply to name an EXISTING
//! resource of that type — the `ResourceIdentifier` entries a `ResourceToImport` carries
//! ("the key is the name of one of the primary identifiers for the resource … the value is
//! the value of that primary identifier") — and it is therefore what `GetTemplateSummary`
//! reports as `ResourceIdentifierSummary$ResourceIdentifiers`: "the properties you can
//! provide during the import to identify the target resources. For example, BucketName is a
//! possible identifier property for an AWS::S3::Bucket resource."
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
    let composite = super::template::composite_identity_properties(resource_type);
    if !composite.is_empty() {
        return composite.iter().map(|part| (*part).to_string()).collect();
    }
    if let Some(published) = identifying_return_value(resource_type) {
        return vec![published.to_string()];
    }
    super::template::name_property_candidates(resource_type)
        .into_iter()
        .take(1)
        .collect()
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
fn identifying_return_value(resource_type: &str) -> Option<&'static str> {
    use super::template::*;

    let identifier = match resource_type {
        // "Ref returns the topic ARN"; the queue URL; the activity ARN.
        SNS_TOPIC => "TopicArn",
        SQS_QUEUE => "QueueUrl",
        STEP_FUNCTIONS_ACTIVITY => "Arn",
        // The CodeArtifact types, each identified by the ARN its create answers.
        CODEARTIFACT_DOMAIN | CODEARTIFACT_REPOSITORY => "Arn",
        // The Amazon S3 Tables table bucket, identified by its own ARN — the `tableBucketARN`
        // every operation of that service on a table bucket takes.
        S3TABLES_TABLE_BUCKET => S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE,
        // The Amazon S3 Vectors types, each identified by its own ARN.
        S3VECTORS_VECTOR_BUCKET => "VectorBucketArn",
        S3VECTORS_INDEX => "IndexArn",
        // An Amazon Braket spending limit, identified by the ARN that service minted for it
        // ("Ref returns the spending limit ARN"), which is the handle every spending-limit
        // API takes — the type declares no name a client could name one by.
        BRAKET_SPENDING_LIMIT => "SpendingLimitArn",
        // An AWS Amplify app, identified by the ARN that service built around the id it minted
        // ("Ref returns the ARN of the app"), which is the type's primary identifier. (The
        // three Amplify UI Builder entities need no row: each is identified by the app, the
        // environment and the id its own resource shape names, which step 1 above answers from
        // [`super::template::composite_identity_properties`].)
        AMPLIFY_APP => "Arn",
        // The CloudFront types, each identified by the id that service assigns.
        CLOUDFRONT_DISTRIBUTION
        | CLOUDFRONT_ORIGIN_ACCESS_CONTROL
        | CLOUDFRONT_ORIGIN_ACCESS_IDENTITY
        | CLOUDFRONT_CACHE_POLICY
        | CLOUDFRONT_ORIGIN_REQUEST_POLICY
        | CLOUDFRONT_RESPONSE_HEADERS_POLICY => "Id",
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
        APIGATEWAY_USAGE_PLAN | APIGATEWAY_USAGE_PLAN_KEY => "Id",
        // The account settings of the REST control plane, identified by the id the resource
        // publishes as `Id`: the settings are a singleton, so the type declares no name a
        // client could name one by.
        APIGATEWAY_ACCOUNT => "Id",
        APIGATEWAYV2_API => "ApiId",
        APIGATEWAYV2_API_MAPPING => "ApiMappingId",
        // The AWS Transfer Family types, each identified by the ARN its create answers.
        TRANSFER_SERVER => "Arn",
        // The Amazon EKS association, the Batch environments and queues, and a task
        // definition: each one's `Ref` is the ARN its create answered.
        EKS_POD_IDENTITY_ASSOCIATION => "AssociationArn",
        // A State Manager association, identified by the `AssociationId` Systems Manager
        // minted — "Ref returns the association ID", the handle every call on one takes.
        SSM_ASSOCIATION => SSM_ASSOCIATION_ID_ATTRIBUTE,
        BATCH_COMPUTE_ENVIRONMENT => "ComputeEnvironmentArn",
        BATCH_JOB_QUEUE => "JobQueueArn",
        BATCH_SERVICE_ENVIRONMENT => "ServiceEnvironmentArn",
        ECS_TASK_DEFINITION => "TaskDefinitionArn",
        // The Elastic Load Balancing v2 types: a load balancer, a target group, a listener
        // and a listener rule are each identified by their own ARN.
        ELB_LOAD_BALANCER => "LoadBalancerArn",
        ELB_TARGET_GROUP => "TargetGroupArn",
        "AWS::ElasticLoadBalancingV2::Listener" => "ListenerArn",
        "AWS::ElasticLoadBalancingV2::ListenerRule" => "RuleArn",
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
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => "Arn",
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
    /// is more than one property.
    #[test]
    fn a_binding_is_identified_by_every_part_of_its_composite_identifier() {
        for resource_type in ["AWS::WAFv2::WebACLAssociation", "AWS::EKS::AccessEntry"] {
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

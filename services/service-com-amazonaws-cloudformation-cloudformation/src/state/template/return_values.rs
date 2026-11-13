// SPDX-License-Identifier: CC0-1.0

use super::*;

// What a template's outputs and its intrinsic functions are evaluated against, split out so
// this file holds the per-type return values alone. Re-exported, so the one path every caller
// of `Scope` and of an evaluation uses is unchanged.
mod outputs;
pub use outputs::*;

/// What `Ref` returns for a resource of this type, and what `Fn::GetAtt` reads off it.
///
/// Source: the "Return values" section the AWS CloudFormation resource and property type
/// reference publishes for each type — `Ref` returns the topic ARN for
/// `AWS::SNS::Topic`, the queue URL for `AWS::SQS::Queue`, the bucket name for
/// `AWS::S3::Bucket`, and so on, with `Fn::GetAtt` reading the listed attributes. A type
/// this catalogue does not carry gets its physical NAME as its physical resource id,
/// which is what the majority of types return, and no attributes: `Fn::GetAtt` against it
/// yields no value rather than a guessed one.
///
/// An attribute whose documented return value is a LIST (an
/// `As2ServiceManagedEgressIpAddresses`, say) is not carried either: the attribute map is
/// string-valued, and reporting one element of a list as the whole of it would be worse
/// than reporting no value.
///
/// The whole declared resource is in scope, not just its type and name, because a few
/// published return values are functions of the resource's own properties — a DB
/// endpoint's port is the one the template declared, or the declared engine's default —
/// and those properties are evaluated through the template's intrinsics, which is what the
/// `scope` and the `budget` are for.
pub(super) fn return_values(
    declared: &DeclaredResource,
    name: &str,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> (String, BTreeMap<String, String>) {
    let region = scope.stack.region;
    let resource_type = declared.resource_type.as_str();
    let properties = &declared.properties;
    let mut attributes = BTreeMap::new();
    match resource_type {
        // "Ref returns the report group name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in CodeBuild itself, which then answers with the
        // report group's own ARN — the same ARN this computes, since both are built from
        // the one account id this emulator mints and the group's name, which is how
        // CodeBuild keys a report group (`report_group_arn` in its own state module).
        CODEBUILD_REPORT_GROUP => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    CODEBUILD_ARN_NAMESPACE,
                    region,
                    &format!("report-group/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the project name"; Fn::GetAtt: Arn. The same position the report
        // group above is in: `super::provisioning` creates the project in CodeBuild, which
        // answers with the project's own ARN — the same ARN this computes, since both are
        // built from the one account id this emulator mints and the project's name, which
        // is how CodeBuild keys a project (`project_arn` in its own state module).
        CODEBUILD_PROJECT => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    CODEBUILD_ARN_NAMESPACE,
                    region,
                    &format!("project/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the topic ARN"; Fn::GetAtt: TopicArn, TopicName. This is the type
        // `super::provisioning` creates in Amazon SNS itself, which then answers with the
        // topic's own ARN — the same ARN this computes, since both are built from the one
        // account id this emulator mints and the topic's name, which is how SNS keys a
        // topic (`topic_arn` in its own state module).
        SNS_TOPIC => {
            let arn = ::emulator_server::emulator_arn("sns", region, name);
            attributes.insert("TopicArn".to_string(), arn.clone());
            attributes.insert("TopicName".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the queue URL"; Fn::GetAtt: Arn, QueueName, QueueUrl. This is the
        // type `super::provisioning` creates in Amazon SQS itself, which then reports the
        // queue's own url back over the `QueueUrl` attribute — the same url this computes,
        // since both are built from the one account id and the queue's name.
        SQS_QUEUE => {
            let url = format!(
                "https://sqs.{region}.{URL_SUFFIX}/{}/{name}",
                ::emulator_server::emulator_account_id()
            );
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("sqs", region, name),
            );
            attributes.insert("QueueName".to_string(), name.to_string());
            attributes.insert("QueueUrl".to_string(), url.clone());
            (url, attributes)
        }
        // "Ref returns the ARN of the activity"; Fn::GetAtt: Arn, Name. This is the type
        // `super::provisioning` creates in AWS Step Functions itself, which then answers
        // with the activity's own ARN — the same ARN this computes, since both are built
        // from the one account id this emulator mints and the activity's name, which is how
        // Step Functions keys an activity (`activity_arn` in its own state module). The ARN
        // namespace is that model's `arnNamespace`, `states`, not its CloudFormation name.
        STEP_FUNCTIONS_ACTIVITY => {
            let arn = ::emulator_server::emulator_arn(
                STEP_FUNCTIONS_ARN_NAMESPACE,
                region,
                &format!("activity:{name}"),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("Name".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the ARN of the state machine"; Fn::GetAtt: Arn, Name. This is the
        // type `super::provisioning` creates in AWS Step Functions itself, which then answers
        // with the machine's own ARN — the same ARN this computes, since both are built from
        // the one account id this emulator mints and the machine's name, which is how Step
        // Functions keys a state machine (`state_machine_arn` in its own state module). The
        // name is the declared `StateMachineName` where the template states one, as it is for
        // every type whose name is a property, and the generated one otherwise.
        STEP_FUNCTIONS_STATE_MACHINE => {
            let declared = properties
                .get("StateMachineName")
                .and_then(|value| text(value, scope, 0, budget))
                .filter(|declared| !declared.is_empty());
            let machine = declared.unwrap_or_else(|| name.to_string());
            let arn = ::emulator_server::emulator_arn(
                STEP_FUNCTIONS_ARN_NAMESPACE,
                region,
                &format!("stateMachine:{machine}"),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("Name".to_string(), machine);
            (arn, attributes)
        }
        // The three Amazon SageMaker AI types `super::provisioning` creates in SageMaker
        // itself ([`crate::state::provisioner::sagemaker_resources`]). Each publishes ONE
        // `Fn::GetAtt` attribute, which is the resource's own NAME — `ModelName` ("the name
        // of the model"), `NotebookInstanceName` ("the name of the notebook instance") and
        // `NotebookInstanceLifecycleConfigName` ("the name of the lifecycle configuration")
        // — so the attribute is the name CloudFormation decided and the create made the
        // resource under.
        //
        // That name is this resource's physical id too, which is the position
        // [`SAGEMAKER_MODEL_PACKAGE_GROUP`] is in and for the same reason: the name is what
        // every SageMaker operation addresses one of these resources by (`DescribeModel` and
        // `DeleteModel` take a `ModelName`, `DescribeNotebookInstance` and
        // `CreatePresignedNotebookInstanceUrl` a `NotebookInstanceName`, and a notebook
        // instance's `LifecycleConfigName` names a configuration by name), so the identifier
        // a client can resolve is preferred over an ARN minted from a pattern. The ARN each
        // create answers is not computed here, no type of the three publishing a return
        // value for it.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL => {
            attributes.insert("ModelName".to_string(), name.to_string());
            (name.to_string(), attributes)
        }
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE => {
            attributes.insert("NotebookInstanceName".to_string(), name.to_string());
            (name.to_string(), attributes)
        }
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            attributes.insert(
                "NotebookInstanceLifecycleConfigName".to_string(),
                name.to_string(),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the user pool ID"; Fn::GetAtt: Arn
        // ("arn:aws:cognito-idp:us-east-1:123412341234:userpool/us-east-1_123412341"),
        // ProviderName ("The provider name of the Amazon Cognito user pool") and
        // ProviderURL ("The URL of the provider of the Amazon Cognito user pool"). The
        // provider name is the pool's endpoint host and its identity
        // (`cognito-idp.<region>.amazonaws.com/<user pool id>`) — the name an identity
        // pool's `CognitoIdentityProviders`, a principal-tag mapping's
        // `IdentityProviderName` and a `Logins` entry all name the pool by — and the
        // provider URL is that same host and identity as an `https` URL, which is the
        // issuer an OpenID relying party reads. Both are built from the resource's OWN
        // identity, so whatever a `Ref` to the pool answers, the provider the pool
        // publishes names that same pool; making that identity the id the owning service
        // mints is the business of the open issue on service-assigned physical ids
        // (`20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never`).
        // "Ref returns the Amazon Resource Name (ARN) of the domain"; Fn::GetAtt: Arn,
        // Name, Owner. This is the type `super::provisioning` creates in CodeArtifact
        // itself, which then answers with the domain's own ARN and owning account — the same
        // values this computes, since both are built from the one account id this emulator
        // mints and the domain's name, which is how CodeArtifact keys a domain (`domain_arn`
        // in that service's own state module).
        CODEARTIFACT_DOMAIN => {
            let arn = ::emulator_server::emulator_arn(
                CODEARTIFACT_ARN_NAMESPACE,
                region,
                &format!("domain/{name}"),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("Name".to_string(), name.to_string());
            attributes.insert(
                "Owner".to_string(),
                ::emulator_server::emulator_account_id().to_string(),
            );
            (arn, attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the repository"; Fn::GetAtt: Arn,
        // DomainOwner, Name. A repository ARN NESTS inside the domain that holds it
        // (`repository/<domain>/<name>`), so it is built on the domain the declaration's
        // `DomainName` names — a declaration that names none leaves the repository with
        // CloudFormation's own generated identity rather than an ARN built on a domain
        // nothing named, exactly as an index of no vector bucket does.
        CODEARTIFACT_REPOSITORY => {
            let domain = properties
                .get("DomainName")
                .and_then(|value| text(value, scope, 0, budget));
            attributes.insert(
                "DomainOwner".to_string(),
                ::emulator_server::emulator_account_id().to_string(),
            );
            attributes.insert("Name".to_string(), name.to_string());
            match domain {
                Some(domain) => {
                    let arn = ::emulator_server::emulator_arn(
                        CODEARTIFACT_ARN_NAMESPACE,
                        region,
                        &format!("repository/{domain}/{name}"),
                    );
                    attributes.insert("Arn".to_string(), arn.clone());
                    (arn, attributes)
                }
                None => (name.to_string(), attributes),
            }
        }
        // "Ref returns the user pool ID"; Fn::GetAtt: Arn, ProviderName, ProviderURL. Each of
        // the three is read off the IDENTITY in force ([`user_pool_attributes`]): the name
        // computed here until the pool's own create has answered, and the id Amazon Cognito
        // minted from then on ([`crate::state::provisioning::cognito_user_pool`] re-points
        // them, exactly as the physical id itself is re-pointed).
        COGNITO_USER_POOL => {
            attributes.extend(user_pool_attributes(region, name));
            (name.to_string(), attributes)
        }
        // "Ref returns the client ID"; Fn::GetAtt: ClientSecret — the secret Amazon Cognito
        // generated for the client, which only `CreateUserPoolClient`'s own answer carries,
        // so what is computed here is the stand-in a declaration is resolved against until
        // that create has answered ([`service_assigned_document`]).
        COGNITO_USER_POOL_CLIENT => {
            attributes.insert(
                "ClientSecret".to_string(),
                service_assigned_document(name, "ClientSecret"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the bucket name"; Fn::GetAtt: Arn, DomainName,
        // DualStackDomainName, RegionalDomainName, WebsiteURL — each one a DNS name S3
        // serves the bucket under, in the form that type's "Return values" section
        // publishes an example of: `DomainName`
        // "mystack-mybucket-kdwwxmddtr2g.s3.amazonaws.com" (the IPv4 DNS name),
        // `DualStackDomainName`
        // "mystack-mybucket-kdwwxmddtr2g.s3.dualstack.us-east-2.amazonaws.com" (the IPv6
        // one), `RegionalDomainName`
        // "mystack-mybucket-kdwwxmddtr2g.s3.us-east-2.amazonaws.com" and `WebsiteURL`
        // "http://mystack-mybucket-kdwwxmddtr2g.s3-website-us-east-2.amazonaws.com" (the
        // bucket's website endpoint). An Amazon S3 bucket ARN carries neither a region nor
        // an account id (`arn:aws:s3:::<bucket>`).
        S3_BUCKET => {
            attributes.insert("Arn".to_string(), format!("arn:{PARTITION}:s3:::{name}"));
            attributes.insert("DomainName".to_string(), format!("{name}.s3.{URL_SUFFIX}"));
            attributes.insert(
                "DualStackDomainName".to_string(),
                format!("{name}.s3.dualstack.{region}.{URL_SUFFIX}"),
            );
            attributes.insert(
                "RegionalDomainName".to_string(),
                format!("{name}.s3.{region}.{URL_SUFFIX}"),
            );
            attributes.insert(
                "WebsiteURL".to_string(),
                format!("http://{name}.s3-website-{region}.{URL_SUFFIX}"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the table bucket ARN": this type's primary identifier is its
        // `TableBucketARN`, which is also its one published `Fn::GetAtt` attribute — the
        // handle every S3 Tables operation on a table bucket takes
        // (`GetTableBucket$tableBucketARN`, an `@httpLabel` whose pattern is
        // `…:bucket/[a-z0-9_-]{3,63}`), so the identifier this type publishes is an ARN
        // rather than the declared name the sibling `AWS::S3::Bucket` publishes. It is the
        // type `super::provisioning` creates in Amazon S3 Tables itself, which then answers
        // with the bucket's own ARN — the same ARN this computes, since both are built from
        // the one account id this emulator mints and the bucket's name, which is how S3
        // Tables keys a table bucket (`table_bucket_arn` in that service's own state
        // module).
        S3TABLES_TABLE_BUCKET => {
            let arn = ::emulator_server::emulator_arn(
                S3TABLES_ARN_NAMESPACE,
                region,
                &format!("{S3TABLES_BUCKET_SEGMENT}{name}"),
            );
            attributes.insert(S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE.to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the vector bucket ARN": this type's primary identifier is its
        // `VectorBucketArn`, which is also its one published `Fn::GetAtt` attribute. It is
        // the type `super::provisioning` creates in Amazon S3 Vectors itself, which then
        // answers with the bucket's own ARN — the same ARN this computes, since both are
        // built from the one account id this emulator mints and the bucket's name, which is
        // how S3 Vectors keys a vector bucket (`bucket_arn` in that service's own state
        // module).
        S3VECTORS_VECTOR_BUCKET => {
            let arn = ::emulator_server::emulator_arn(
                S3VECTORS_ARN_NAMESPACE,
                region,
                &format!("{S3VECTORS_BUCKET_SEGMENT}{name}"),
            );
            attributes.insert("VectorBucketArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the vector index ARN", and an index ARN NESTS inside the ARN of the
        // vector bucket that holds it (`bucket/<bucket>/index/<name>`), so it is resolved
        // once that bucket's identity is known ([`resolve_dependent_identities`]). The
        // declaration names the bucket the two ways S3 Vectors' own API addresses one — by
        // name or by ARN ([`declared_vector_bucket`]) — and one that names neither leaves
        // the index with CloudFormation's own generated identity rather than an ARN built
        // on a bucket nothing named.
        S3VECTORS_INDEX => {
            let bucket = declared_vector_bucket(properties, scope, budget);
            match bucket {
                Some(bucket) => {
                    let arn = ::emulator_server::emulator_arn(
                        S3VECTORS_ARN_NAMESPACE,
                        region,
                        &format!("{S3VECTORS_BUCKET_SEGMENT}{bucket}/index/{name}"),
                    );
                    attributes.insert("IndexArn".to_string(), arn.clone());
                    (arn, attributes)
                }
                None => (name.to_string(), attributes),
            }
        }
        // "Ref returns the distribution ID"; Fn::GetAtt: DomainName, Id. CloudFront
        // ASSIGNS both — the type has no name property for a template to set — so a
        // declaration reports identifiers of CLOUDFRONT's forms rather than
        // CloudFormation's `<stack>-<logical id>-<unique id>` name (the case
        // AWS::Transfer::Server and AWS::RUM::AppMonitor are in): an id is
        // [`cloudfront_assigned_id`] and the domain name
        // [`cloudfront_distribution_domain_name`].
        CLOUDFRONT_DISTRIBUTION => {
            let id = cloudfront_assigned_id(CLOUDFRONT_DISTRIBUTION_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            attributes.insert(
                "DomainName".to_string(),
                cloudfront_distribution_domain_name(name),
            );
            (id, attributes)
        }
        // "Ref returns the origin access control's unique identifier"; Fn::GetAtt: Id.
        // Service-assigned in the same form a distribution id takes.
        CLOUDFRONT_ORIGIN_ACCESS_CONTROL => {
            let id = cloudfront_assigned_id(CLOUDFRONT_ORIGIN_ACCESS_CONTROL_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the CloudFront origin access identity"; Fn::GetAtt: Id,
        // S3CanonicalUserId. The id is service-assigned in the form
        // `CloudFrontOriginAccessIdentity$Id` documents ("for example, E74FTE3AJFJ256A"),
        // and the canonical user id is the one S3 knows the identity by, in the
        // [`S3_CANONICAL_USER_ID_HEX_CHARS`] lowercase hex characters S3 writes one in.
        CLOUDFRONT_ORIGIN_ACCESS_IDENTITY => {
            let id = cloudfront_assigned_id(CLOUDFRONT_ORIGIN_ACCESS_IDENTITY_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            attributes.insert(
                "S3CanonicalUserId".to_string(),
                derived_digits(
                    CLOUDFRONT_S3_CANONICAL_USER_ID_SALT,
                    name,
                    S3_CANONICAL_USER_ID_HEX_CHARS,
                    Alphabet::Lower,
                ),
            );
            (id, attributes)
        }
        // "Ref returns the cache policy's unique identifier"; Fn::GetAtt: Id,
        // LastModifiedTime. CloudFront identifies each of its three policy kinds by a
        // UUID it mints, so that is what `Ref` and `Id` report. `LastModifiedTime` is NOT
        // carried: it is the instant the policy was last changed, which this module has no
        // value for, and an invented timestamp would be worse than no value.
        CLOUDFRONT_CACHE_POLICY | CLOUDFRONT_ORIGIN_REQUEST_POLICY
        | CLOUDFRONT_RESPONSE_HEADERS_POLICY => {
            let id = generated_uuid(resource_type, name);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the RestApi ID"; Fn::GetAtt: RestApiId, RootResourceId. API Gateway
        // ASSIGNS both ids — a template names the API but not its identifier — in the form
        // [`api_gateway_assigned_id`] describes, and the root resource is a resource of the
        // API, so it carries an id of its own rather than the API's.
        APIGATEWAY_REST_API => {
            let id = api_gateway_assigned_id(APIGATEWAY_REST_API_ID_SALT, name);
            attributes.insert("RestApiId".to_string(), id.clone());
            attributes.insert(
                "RootResourceId".to_string(),
                api_gateway_assigned_id(APIGATEWAY_ROOT_RESOURCE_ID_SALT, name),
            );
            (id, attributes)
        }
        // "Ref returns the resource ID"; Fn::GetAtt: ResourceId.
        APIGATEWAY_RESOURCE => {
            let id = api_gateway_assigned_id(APIGATEWAY_RESOURCE_ID_SALT, name);
            attributes.insert("ResourceId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the deployment ID"; Fn::GetAtt: DeploymentId.
        APIGATEWAY_DEPLOYMENT => {
            let id = api_gateway_assigned_id(APIGATEWAY_DEPLOYMENT_ID_SALT, name);
            attributes.insert("DeploymentId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the API key ID"; Fn::GetAtt: APIKeyId. API Gateway ASSIGNS the id —
        // a template names the key but not its identifier — so the stand-in is of the form
        // [`api_gateway_assigned_id`] describes and is REPLACED by the id `CreateApiKey`
        // really minted, which is what [`identity_is_service_assigned`] names this type
        // for.
        APIGATEWAY_API_KEY => {
            let id = api_gateway_assigned_id(APIGATEWAY_API_KEY_ID_SALT, name);
            attributes.insert("APIKeyId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the authorizer ID"; Fn::GetAtt: AuthorizerId. The same
        // service-assigned shape, replaced by the id `CreateAuthorizer` mints.
        APIGATEWAY_AUTHORIZER => {
            let id = api_gateway_assigned_id(APIGATEWAY_AUTHORIZER_ID_SALT, name);
            attributes.insert("AuthorizerId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the ID for the account"; Fn::GetAtt: Id. The account settings are a
        // SINGLETON of the account and Region — `UpdateAccount` takes no identifier and
        // answers none — so the id a stack publishes for the resource is CloudFormation's
        // own, of the shape API Gateway writes an identifier in, and
        // [`identity_is_service_assigned`] does NOT name this type.
        APIGATEWAY_ACCOUNT => {
            let id = api_gateway_assigned_id(APIGATEWAY_ACCOUNT_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the usage plan ID"; Fn::GetAtt: Id. The same service-assigned shape,
        // replaced by the id `CreateUsagePlan` mints.
        APIGATEWAY_USAGE_PLAN => {
            let id = api_gateway_assigned_id(APIGATEWAY_USAGE_PLAN_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the usage plan key ID"; Fn::GetAtt: Id. An association's id is the id
        // of the API KEY it put in the plan, which only API Gateway produces either, so the
        // stand-in takes the same shape and is replaced by what `CreateUsagePlanKey`
        // answered.
        APIGATEWAY_USAGE_PLAN_KEY => {
            let id = api_gateway_assigned_id(APIGATEWAY_USAGE_PLAN_KEY_ID_SALT, name);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the VPC link ID"; Fn::GetAtt: VpcLinkId. API Gateway ASSIGNS the id —
        // a template names the link but not its identifier — so the stand-in takes the same
        // shape and is REPLACED by the id `CreateVpcLink` really minted, which is what
        // [`identity_is_service_assigned`] names this type for.
        APIGATEWAY_VPC_LINK => {
            let id = api_gateway_assigned_id(APIGATEWAY_VPC_LINK_ID_SALT, name);
            attributes.insert("VpcLinkId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the API ID"; Fn::GetAtt: ApiEndpoint ("The default endpoint for an
        // API"). API Gateway v2 ASSIGNS the id — a template names the api but not its
        // identifier — so the id is of the form [`api_gateway_assigned_id`] describes and
        // the endpoint is the `{api-id}.execute-api.{region}.amazonaws.com` host
        // `Api$ApiEndpoint` documents, under the scheme the declared `ProtocolType`
        // speaks. Both are REPLACED by the ones that service really minted once the
        // resource is created there ([`super::provisioner`]), which is what
        // [`identity_is_service_assigned`] names this type for.
        APIGATEWAYV2_API => {
            let id = api_gateway_assigned_id(APIGATEWAYV2_API_ID_SALT, name);
            attributes.insert(
                "ApiEndpoint".to_string(),
                apigatewayv2_api_endpoint(&id, region, properties, scope, budget),
            );
            (id, attributes)
        }
        // "Ref returns the API mapping ID"; Fn::GetAtt: ApiMappingId. The same
        // service-assigned shape, replaced by the id `CreateApiMapping` mints.
        APIGATEWAYV2_API_MAPPING => {
            let id = api_gateway_assigned_id(APIGATEWAYV2_API_MAPPING_ID_SALT, name);
            attributes.insert("ApiMappingId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the domain name"; Fn::GetAtt: DomainNameArn, RegionalDomainName,
        // RegionalHostedZoneId. The physical id is the domain name the template states
        // (the type's own identifier, read through [`declared_name_property`]); the ARN is
        // the one API Gateway names a custom domain by (its own REST path,
        // `/domainnames/{name}`, under the `apigateway` ARN namespace that service's model
        // declares), and the regional domain name is the
        // `d-{id}.execute-api.{region}.amazonaws.com` host a caller aliases their DNS
        // record to. `RegionalHostedZoneId` is NOT carried: it names a Route 53 zone this
        // emulator neither owns nor can look up, and an invented zone id is one a caller
        // could put in a real alias record.
        APIGATEWAYV2_DOMAIN_NAME => {
            attributes.insert(
                "DomainNameArn".to_string(),
                ::emulator_server::emulator_arn(
                    APIGATEWAYV2_ARN_NAMESPACE,
                    region,
                    &format!("/domainnames/{name}"),
                ),
            );
            attributes.insert(
                "RegionalDomainName".to_string(),
                apigatewayv2_regional_domain_name(name, region),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the table name"; Fn::GetAtt: Arn.
        "AWS::DynamoDB::Table" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("dynamodb", region, &format!("table/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the function name"; Fn::GetAtt: Arn.
        "AWS::Lambda::Function" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("lambda", region, &format!("function:{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the ARN of the function version"; Fn::GetAtt: FunctionArn, Version.
        // Both carry the version NUMBER, which is AWS Lambda's to allocate — "each time you
        // publish a version, Lambda increments the version number" — so neither can be
        // computed from the template: what this gives the resource is the stand-in the
        // publish then replaces, under the identifier CloudFormation computed for it
        // ([`identity_is_service_assigned`] names the type, and
        // [`crate::state::provisioning`] re-points the id and the attribute together).
        LAMBDA_VERSION => {
            match lambda_function_arn(properties, scope, budget, region) {
                Some(function_arn) => {
                    let arn = format!("{function_arn}:{name}");
                    attributes.insert("FunctionArn".to_string(), arn.clone());
                    attributes.insert("Version".to_string(), name.to_string());
                    (arn, attributes)
                }
                // A declaration naming no function leaves the version with
                // CloudFormation's own generated identity rather than an ARN built on a
                // value that is not one.
                None => (name.to_string(), attributes),
            }
        }
        // "Ref returns the alias ARN"; Fn::GetAtt: AliasArn. An alias's ARN is the FUNCTION's
        // own qualified by the alias name ("arn:aws:lambda:us-west-2:123456789012:function:
        // my-function:BLUE"), so it is resolved once the function the declaration names has
        // an identity ([`resolve_dependent_identities`]) — and it is the ARN `CreateAlias`
        // answers, since both are built from the one account id this emulator mints, the
        // function's name and the alias's.
        LAMBDA_ALIAS => {
            match lambda_function_arn(properties, scope, budget, region) {
                Some(function_arn) => {
                    let arn = format!("{function_arn}:{name}");
                    attributes.insert("AliasArn".to_string(), arn.clone());
                    (arn, attributes)
                }
                None => (name.to_string(), attributes),
            }
        }
        // "Ref returns the log group name"; Fn::GetAtt: Arn. A CloudWatch Logs log-group
        // ARN ends in `:*` ("such as
        // arn:aws:logs:us-east-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*").
        "AWS::Logs::LogGroup" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "logs",
                    region,
                    &format!("log-group:{name}:*"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the server"; Fn::GetAtt: Arn,
        // ServerId ("the service-assigned ID of the server that is created") and the
        // list-valued As2ServiceManagedEgressIpAddresses, which is not carried. The
        // template names no server — AWS::Transfer::Server has no name property — so the
        // id is the service-assigned `s-` + 17 hexadecimal form.
        TRANSFER_SERVER => {
            let server_id = generated_service_id("s", name, TRANSFER_ID_HEX_DIGITS);
            let arn =
                ::emulator_server::emulator_arn("transfer", region, &format!("server/{server_id}"));
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("ServerId".to_string(), server_id);
            (arn, attributes)
        }
        // The same shape one type along: "Ref returns the Amazon Resource Name (ARN)";
        // Fn::GetAtt: Arn, WorkflowId. Transfer's `WorkflowId` is `w-` + 17 characters of
        // `[a-z0-9]` (model pattern `^w-([a-z0-9]{17})$`), which hexadecimal satisfies.
        "AWS::Transfer::Workflow" => {
            let workflow_id = generated_service_id("w", name, TRANSFER_ID_HEX_DIGITS);
            let arn = ::emulator_server::emulator_arn(
                "transfer",
                region,
                &format!("workflow/{workflow_id}"),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("WorkflowId".to_string(), workflow_id);
            (arn, attributes)
        }
        // "Ref returns the AWS::IAM::AccessKey resource's access key ID"; Fn::GetAtt:
        // SecretAccessKey — "the secret access key for the specified AWS::IAM::AccessKey
        // resource". IAM MINTS both: `CreateAccessKey` takes no identifier and answers the
        // key id and the secret it generated, and the secret "is returned only in response
        // to CreateAccessKey", so neither is a value these naming rules can compute. The
        // identifier a declaration reports until that create answers is the name
        // CloudFormation generated for the resource ([`identity_is_service_assigned`] names
        // the type, so [`super::provisioning::reconcile`] adopts the minted id over it), and
        // the secret is a stand-in that same walk re-points at what IAM answered — never an
        // invented credential.
        IAM_ACCESS_KEY => {
            attributes.insert(
                "SecretAccessKey".to_string(),
                service_assigned_document(name, "SecretAccessKey"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the role name"; Fn::GetAtt: Arn. IAM is a global service, so its
        // ARNs carry no region.
        "AWS::IAM::Role" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("iam", "", &format!("role/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the rule name"; Fn::GetAtt: Arn. An EventBridge rule ARN names the
        // bus the rule is bound to unless that is the account's `default` bus
        // (`arn:aws:events:<region>:<account>:rule/[<bus>/]<name>`), which is the form the
        // `events` service mints for the rule this stack provisions.
        EVENTS_RULE => {
            let bus = properties
                .get("EventBusName")
                .and_then(|value| text(value, scope, 0, budget))
                .map(|bus| event_bus_name(&bus))
                .filter(|bus| !bus.is_empty() && bus != DEFAULT_EVENT_BUS)
                .map(|bus| format!("{bus}/"))
                .unwrap_or_default();
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("events", region, &format!("rule/{bus}{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the name of the parameter"; Fn::GetAtt: Type, Value — the two
        // required properties of the declaration, which is what the parameter Systems
        // Manager holds carries.
        SSM_PARAMETER => {
            for attribute in ["Type", "Value"] {
                if let Some(declared) = properties
                    .get(attribute)
                    .and_then(|value| text(value, scope, 0, budget))
                {
                    attributes.insert(attribute.to_string(), declared);
                }
            }
            (name.to_string(), attributes)
        }
        // "Ref returns the group name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Resource Groups itself, which keys a group by its
        // name and mints the same ARN from the Region and that name (`group_arn` in its own
        // state module). The ARN namespace is that model's `arnNamespace`,
        // [`RESOURCE_GROUPS_ARN_NAMESPACE`], not the type's CloudFormation segment.
        RESOURCE_GROUPS_GROUP => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    RESOURCE_GROUPS_ARN_NAMESPACE,
                    region,
                    &format!("group/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the schedule group name"; Fn::GetAtt: Arn (and the group's State,
        // CreationDate and LastModificationDate, which are EventBridge Scheduler's own to
        // decide and are not computed here). This is the type `super::provisioning`
        // creates in that service, which keys a group by its name and mints the same ARN
        // from the Region and that name (`schedule_group_arn` in its own state module).
        SCHEDULER_SCHEDULE_GROUP => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    SCHEDULER_ARN_NAMESPACE,
                    region,
                    &format!("schedule-group/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the schedule name"; Fn::GetAtt: Arn. A schedule ARN nests the
        // schedule inside the GROUP that holds it (`schedule/<group>/<name>`), and a
        // declaration naming no group is in the account's `default` group — the same ARN
        // EventBridge Scheduler mints for the schedule this stack provisions
        // (`schedule_arn` in its own state module).
        SCHEDULER_SCHEDULE => {
            let group = properties
                .get("GroupName")
                .and_then(|value| text(value, scope, 0, budget))
                .filter(|group| !group.is_empty())
                .unwrap_or_else(|| DEFAULT_SCHEDULE_GROUP.to_string());
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    SCHEDULER_ARN_NAMESPACE,
                    region,
                    &format!("schedule/{group}/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the repository name"; Fn::GetAtt: Arn. Amazon ECR Public is a
        // partition-wide service, so the ARN carries no region — the same form the
        // `ecr-public` service mints for the repository this stack provisions, which is
        // what lets the two be compared.
        ECR_PUBLIC_REPOSITORY => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("ecr-public", "", &format!("repository/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the delivery stream name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Amazon Data Firehose itself, which then
        // reports the stream's own ARN back over this attribute.
        FIREHOSE_DELIVERY_STREAM => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "firehose",
                    region,
                    &format!("deliverystream/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the stream name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Amazon Kinesis Data Streams itself, which then
        // reports the stream's own ARN back over this attribute — the same ARN this
        // computes, since both are built from the one account id this emulator mints and
        // the stream's name, which is how that service keys a stream (`stream_arn` in its
        // own state module).
        KINESIS_STREAM => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("kinesis", region, &format!("stream/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the repository name"; Fn::GetAtt: Arn, RepositoryUri. The URI is
        // the registry host of the account and Region plus the repository's name
        // ("The format is `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name`"),
        // which is what a container image reference built from this stack's output pulls
        // from — and exactly what Amazon ECR's own `CreateRepository` answers.
        ECR_REPOSITORY => {
            // "A repository name must start with a letter and can only contain lowercase
            // letters, numbers, hyphens, underscores, and forward slashes"
            // (`RepositoryName`'s pattern is lower case only), which is the character set
            // [`naming_rules`] already holds a GENERATED name to; a name the template
            // declared is reported exactly as it wrote it.
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("ecr", region, &format!("repository/{name}")),
            );
            attributes.insert(
                "RepositoryUri".to_string(),
                format!(
                    "{}.dkr.ecr.{region}.{URL_SUFFIX}/{name}",
                    ::emulator_server::emulator_account_id()
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the cluster name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Amazon ECS itself, so the cluster a stack's
        // output names is one `ecs ListTasks` finds.
        ECS_CLUSTER => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("ecs", region, &format!("cluster/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the name of the cluster"; Fn::GetAtt: Arn, CertificateAuthorityData,
        // ClusterSecurityGroupId, EncryptionConfigKeyArn, Endpoint, OpenIdConnectIssuerUrl.
        // This is the type `super::provisioning` creates in Amazon EKS itself, so the
        // cluster a stack's output names is one `eks describe-cluster` finds — and every
        // attribute but the ARN is a value only that service can produce (the API server's
        // endpoint, the cluster CA, the OIDC issuer, the security group it made for the
        // control plane), so each stands in until the create reports what EKS really
        // answered.
        EKS_CLUSTER => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(EKS_ARN_NAMESPACE, region, &format!("cluster/{name}")),
            );
            for attribute in [
                "CertificateAuthorityData",
                "ClusterSecurityGroupId",
                "EncryptionConfigKeyArn",
                "Endpoint",
                "OpenIdConnectIssuerUrl",
            ] {
                attributes.insert(
                    attribute.to_string(),
                    service_assigned_document(name, attribute),
                );
            }
            (name.to_string(), attributes)
        }
        // "Ref returns the association ARN"; Fn::GetAtt: AssociationArn, AssociationId.
        // Amazon EKS ASSIGNS both — `CreatePodIdentityAssociation` takes the cluster, the
        // namespace, the service account and the role and answers the association it minted
        // — so a declaration reports a stand-in until `super::provisioning` has created the
        // association and reported the ARN and the id that service really assigned.
        EKS_POD_IDENTITY_ASSOCIATION => {
            let arn = ::emulator_server::emulator_arn(
                EKS_ARN_NAMESPACE,
                region,
                &format!("podidentityassociation/{name}"),
            );
            attributes.insert("AssociationArn".to_string(), arn.clone());
            attributes.insert(
                "AssociationId".to_string(),
                service_assigned_document(name, "AssociationId"),
            );
            (arn, attributes)
        }
        // "Ref returns the compute environment ARN"; Fn::GetAtt: ComputeEnvironmentArn.
        // This is the type `super::provisioning` creates in AWS Batch itself, so the
        // environment a stack's output names is one `batch describe-compute-environments`
        // finds — under the same ARN that service mints from the Region and the name
        // (`compute_environment_arn` in its own state module).
        BATCH_COMPUTE_ENVIRONMENT => {
            let arn = ::emulator_server::emulator_arn(
                BATCH_ARN_NAMESPACE,
                region,
                &format!("compute-environment/{name}"),
            );
            attributes.insert("ComputeEnvironmentArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the job queue ARN"; Fn::GetAtt: JobQueueArn. The same shape one
        // type along, on the ARN AWS Batch mints for a queue (`job_queue_arn`).
        BATCH_JOB_QUEUE => {
            let arn = ::emulator_server::emulator_arn(
                BATCH_ARN_NAMESPACE,
                region,
                &format!("job-queue/{name}"),
            );
            attributes.insert("JobQueueArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // And one more: "Ref returns the service environment ARN"; Fn::GetAtt:
        // ServiceEnvironmentArn (`service_environment_arn`).
        BATCH_SERVICE_ENVIRONMENT => {
            let arn = ::emulator_server::emulator_arn(
                BATCH_ARN_NAMESPACE,
                region,
                &format!("service-environment/{name}"),
            );
            attributes.insert("ServiceEnvironmentArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the task definition";
        // Fn::GetAtt: TaskDefinitionArn.
        //
        // A task definition is identified by its FAMILY and a revision, not by a name
        // property ("the family and revision (family:revision)"), so the family is the
        // declared `Family` — evaluated through the template's intrinsics, as every
        // declared name is — and the revision is the FIRST one, which is the revision the
        // registration a stack declaring a family makes carries. A later registration of
        // the same family is numbered by Amazon ECS, and nothing here re-reads that
        // number.
        ECS_TASK_DEFINITION => {
            let family = properties
                .get("Family")
                .and_then(|value| text(value, scope, 0, budget))
                .unwrap_or_else(|| name.to_string());
            let arn = ::emulator_server::emulator_arn(
                "ecs",
                region,
                &format!("task-definition/{family}:{FIRST_TASK_DEFINITION_REVISION}"),
            );
            attributes.insert("TaskDefinitionArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the ARN of the load balancer"; Fn::GetAtt: CanonicalHostedZoneID,
        // DNSName, LoadBalancerFullName, LoadBalancerName, SecurityGroups.
        //
        // This is the type `super::provisioning` creates in Elastic Load Balancing itself, so
        // the ARN a stack's output names is one that service really holds a load balancer
        // for, and the values only IT can produce stand in until its create has answered:
        // the hosted zone it publishes for the Region and the family, and the groups in front
        // of the load balancer — which for an Application Load Balancer a declaration names
        // none for are the default security group of its VPC. The ARN and the DNS name carry
        // an id that service ASSIGNS (`.../loadbalancer/app/<name>/<id>`,
        // `<name>-<id>.<region>.elb.amazonaws.com`, prefixed `internal-` for an internal
        // scheme), which is why those too are the create's to report and the values computed
        // here are only what a declaration is resolved against until then.
        ELB_LOAD_BALANCER => {
            let id = service_assigned_label(ELB_LOAD_BALANCER_ID_SALT, name, ELB_ID_HEX_DIGITS);
            let type_code = load_balancer_type_code(properties, scope, budget);
            let path = format!("{type_code}/{name}/{id}");
            let internal = properties
                .get("Scheme")
                .and_then(|value| text(value, scope, 0, budget))
                .as_deref()
                == Some(ELB_SCHEME_INTERNAL);
            let prefix = if internal { "internal-" } else { "" };
            attributes.insert(
                "CanonicalHostedZoneID".to_string(),
                service_assigned_document(name, "CanonicalHostedZoneID"),
            );
            attributes.insert(
                "DNSName".to_string(),
                format!("{prefix}{name}-{id}.{region}.elb.{URL_SUFFIX}"),
            );
            attributes.insert("LoadBalancerFullName".to_string(), path.clone());
            attributes.insert("LoadBalancerName".to_string(), name.to_string());
            attributes.insert(
                "SecurityGroups".to_string(),
                service_assigned_document(name, "SecurityGroups"),
            );
            (
                ::emulator_server::emulator_arn(
                    "elasticloadbalancing",
                    region,
                    &format!("loadbalancer/{path}"),
                ),
                attributes,
            )
        }
        // "Ref returns the ARN of the target group"; Fn::GetAtt: LoadBalancerArns,
        // TargetGroupArn, TargetGroupFullName, TargetGroupName. The full name is the ARN's own
        // resource path, `targetgroup/<name>/<id>`, with the same service-assigned id form,
        // and `LoadBalancerArns` — "the ARN of the load balancer that routes traffic to this
        // target group" — is a value only Elastic Load Balancing can produce, so it stands in
        // until that service has answered for the group `super::provisioning` creates there.
        ELB_TARGET_GROUP => {
            let id = service_assigned_label(ELB_TARGET_GROUP_ID_SALT, name, ELB_ID_HEX_DIGITS);
            let path = format!("targetgroup/{name}/{id}");
            let arn =
                ::emulator_server::emulator_arn("elasticloadbalancing", region, &path);
            attributes.insert(
                "LoadBalancerArns".to_string(),
                service_assigned_document(name, "LoadBalancerArns"),
            );
            attributes.insert("TargetGroupArn".to_string(), arn.clone());
            attributes.insert("TargetGroupFullName".to_string(), path);
            attributes.insert("TargetGroupName".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the DBClusterIdentifier"; Fn::GetAtt: DBClusterArn,
        // DBClusterResourceId, Endpoint.Address, Endpoint.Port, ReadEndpoint.Address
        // (MasterUserSecret.SecretArn is left out: it names a secret only RDS's own
        // managed-password integration creates, which this emulator does not have). The
        // endpoints are the published Aurora forms, `<identifier>.cluster-<label>` and
        // `<identifier>.cluster-ro-<label>` under the regional RDS suffix.
        "AWS::RDS::DBCluster" => {
            let label = service_assigned_label(
                RDS_ENDPOINT_LABEL_SALT,
                name,
                RDS_ENDPOINT_LABEL_HEX_DIGITS,
            );
            attributes.insert(
                "DBClusterArn".to_string(),
                ::emulator_server::emulator_arn("rds", region, &format!("cluster:{name}")),
            );
            attributes.insert(
                "DBClusterResourceId".to_string(),
                generated_upper_resource_id("cluster", name, RDS_RESOURCE_ID_DIGITS),
            );
            attributes.insert(
                "Endpoint.Address".to_string(),
                format!("{name}.cluster-{label}.{region}.rds.{URL_SUFFIX}"),
            );
            if let Some(port) = database_port(properties, scope, budget) {
                attributes.insert("Endpoint.Port".to_string(), port);
            }
            attributes.insert(
                "ReadEndpoint.Address".to_string(),
                format!("{name}.cluster-ro-{label}.{region}.rds.{URL_SUFFIX}"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the DBInstanceIdentifier"; Fn::GetAtt: DBInstanceArn, DbiResourceId,
        // Endpoint.Address, Endpoint.Port (Endpoint.HostedZoneId is a value AWS allocated
        // per Region — nothing computes it — so it is left out rather than invented).
        "AWS::RDS::DBInstance" => {
            let label = service_assigned_label(
                RDS_ENDPOINT_LABEL_SALT,
                name,
                RDS_ENDPOINT_LABEL_HEX_DIGITS,
            );
            attributes.insert(
                "DBInstanceArn".to_string(),
                ::emulator_server::emulator_arn("rds", region, &format!("db:{name}")),
            );
            attributes.insert(
                "DbiResourceId".to_string(),
                generated_upper_resource_id("db", name, RDS_RESOURCE_ID_DIGITS),
            );
            attributes.insert(
                "Endpoint.Address".to_string(),
                format!("{name}.{label}.{region}.rds.{URL_SUFFIX}"),
            );
            if let Some(port) = database_port(properties, scope, budget) {
                attributes.insert("Endpoint.Port".to_string(), port);
            }
            (name.to_string(), attributes)
        }
        // "Ref returns the ARN of the secret"; Fn::GetAtt: Id, which is that same ARN. A
        // Secrets Manager secret ARN ends in a service-assigned six-character suffix
        // (`arn:aws:secretsmanager:<region>:<account>:secret:<name>-<suffix>`), so the
        // resource id is of THAT form rather than the name.
        SECRETSMANAGER_SECRET => {
            let suffix =
                service_assigned_label(SECRET_ARN_SUFFIX_SALT, name, SECRET_ARN_SUFFIX_CHARACTERS);
            let arn = ::emulator_server::emulator_arn(
                "secretsmanager",
                region,
                &format!("secret:{name}-{suffix}"),
            );
            attributes.insert("Id".to_string(), arn.clone());
            (arn, attributes)
        }
        // The Amazon EC2 VPC-network types ([`EC2_VPC`] and its siblings), each of which
        // publishes "Ref returns the ID of …" and one `Fn::GetAtt` attribute that IS that
        // id: a VPC's `VpcId`, a subnet's `SubnetId`, a security group's `GroupId`, an
        // internet gateway's `InternetGatewayId`, and the plain `Id` the types that publish
        // no narrower name carry. The id is the `<prefix>-` + 17 hexadecimal form Amazon
        // EC2 assigns, never the name CloudFormation computed, because EC2 takes no name on
        // any of these creates ([`ec2_network_identity`]).
        EC2_VPC => {
            let id = ec2_network_identity("vpc", "VpcId", name, &mut attributes);
            (id, attributes)
        }
        EC2_SUBNET => {
            let id = ec2_network_identity("subnet", "SubnetId", name, &mut attributes);
            (id, attributes)
        }
        EC2_SECURITY_GROUP => {
            let id = ec2_network_identity("sg", "GroupId", name, &mut attributes);
            (id, attributes)
        }
        EC2_INTERNET_GATEWAY => {
            let id = ec2_network_identity("igw", "InternetGatewayId", name, &mut attributes);
            (id, attributes)
        }
        EC2_ROUTE_TABLE => {
            let id = ec2_network_identity("rtb", "RouteTableId", name, &mut attributes);
            (id, attributes)
        }
        // An Elastic IP address, whose published return values are the two halves of what
        // `AllocateAddress` answers: "Ref returns the Elastic IP address" and "AllocationId
        // — the ID that represents the allocation of the Elastic IP address". Only the
        // allocation id has a FORM a rule here can compute (Amazon EC2's own `eipalloc-` plus
        // 17 hexadecimal characters), which is the stand-in a NAT gateway of the same
        // template is resolved against before the address is allocated and which the
        // deployment then re-points at the allocation that service answered. The address
        // itself is this resource's physical id, so it is the identity's own stand-in rather
        // than an attribute computed beside it, and the create reports the address EC2
        // allocated ([`super::provisioning`]).
        EC2_EIP => {
            attributes.insert(
                "AllocationId".to_string(),
                generated_service_id("eipalloc", name, EC2_ID_HEX_DIGITS),
            );
            (name.to_string(), attributes)
        }
        EC2_NAT_GATEWAY => {
            let id = ec2_network_identity("nat", "NatGatewayId", name, &mut attributes);
            (id, attributes)
        }
        EC2_DHCP_OPTIONS => {
            let id = ec2_network_identity("dopt", "DhcpOptionsId", name, &mut attributes);
            (id, attributes)
        }
        EC2_VPN_GATEWAY => {
            let id = ec2_network_identity("vgw", "VPNGatewayId", name, &mut attributes);
            (id, attributes)
        }
        EC2_NETWORK_ACL => {
            let id = ec2_network_identity("acl", "Id", name, &mut attributes);
            (id, attributes)
        }
        EC2_NETWORK_INTERFACE => {
            let id = ec2_network_identity("eni", "Id", name, &mut attributes);
            (id, attributes)
        }
        EC2_VPC_ENDPOINT => {
            let id = ec2_network_identity("vpce", "Id", name, &mut attributes);
            (id, attributes)
        }
        // An Amazon EBS volume, whose identity Amazon EC2 assigns the same way: "Ref returns
        // the volume ID", and the type's one `Fn::GetAtt` attribute IS that id.
        EC2_VOLUME => {
            let id = ec2_network_identity("vol", "VolumeId", name, &mut attributes);
            (id, attributes)
        }
        // A LAUNCH TEMPLATE, whose identity Amazon EC2 assigns the same way: "Ref returns the
        // launch template ID" (the `lt-` + 17 hexadecimal form), and whose published
        // `Fn::GetAtt` attributes are that same id and the two VERSION numbers of the
        // template — the latest and the default, both 1 for the one version
        // `CreateLaunchTemplate` creates ([`FIRST_LAUNCH_TEMPLATE_VERSION`]). The create
        // re-points all three at what EC2 answered.
        EC2_LAUNCH_TEMPLATE => {
            let id = ec2_network_identity("lt", "LaunchTemplateId", name, &mut attributes);
            for attribute in ["DefaultVersionNumber", "LatestVersionNumber"] {
                attributes.insert(
                    attribute.to_string(),
                    FIRST_LAUNCH_TEMPLATE_VERSION.to_string(),
                );
            }
            (id, attributes)
        }
        // An INSTANCE, whose identity Amazon EC2 assigns the same way again — "Ref returns the
        // ID of the instance", the `i-` + 17 hexadecimal form — and whose published
        // `Fn::GetAtt` attributes are NONE of them that id: they are the zone the instance was
        // placed in and its four addresses and DNS names, every one of which only Amazon EC2
        // can produce. Each therefore starts EMPTY, which is both what a reference to a
        // resource that does not exist yet can resolve to and what AWS answers for an address
        // an instance was never assigned; the launch's own answer and the read that follows the
        // instance coming up are what fill them (`crate::state::provisioning`'s instance
        // provider).
        EC2_INSTANCE => {
            for attribute in published_attribute_names(EC2_INSTANCE) {
                attributes.insert((*attribute).to_string(), String::new());
            }
            (
                generated_service_id("i", name, EC2_ID_HEX_DIGITS),
                attributes,
            )
        }
        // "Ref returns the Amazon Resource Name (ARN)"; Fn::GetAtt: Name. A service ARN
        // carries the CLUSTER that hosts it (`service/<cluster>/<name>`), which the
        // declaration names by short name or by cluster ARN and which defaults to the
        // `default` cluster — so the cluster is resolved once the resource the declaration
        // names has an identity ([`resolve_dependent_identities`]). The `Arn`/`ServiceArn`
        // the convention adds is that same ARN, since it is the service's physical id.
        ECS_SERVICE => {
            let cluster = properties
                .get("Cluster")
                .and_then(|value| text(value, scope, 0, budget))
                .map(|reference| ecs_cluster_name(&reference))
                .unwrap_or_else(|| ECS_DEFAULT_CLUSTER.to_string());
            attributes.insert("Name".to_string(), name.to_string());
            (
                ::emulator_server::emulator_arn(
                    "ecs",
                    region,
                    &format!("service/{cluster}/{name}"),
                ),
                attributes,
            )
        }
        // "Ref returns the ARN of the listener"; Fn::GetAtt: ListenerArn. A listener's ARN
        // nests inside the ARN of the load balancer its declaration names
        // (`listener/<type>/<lb>/<lb id>/<listener id>`), so it is resolved once that load
        // balancer's identity is known ([`resolve_dependent_identities`]); a declaration
        // naming none leaves the listener with CloudFormation's own generated identity
        // rather than an ARN built on a value that is not one.
        ELB_LISTENER => {
            let listener_arn = properties
                .get("LoadBalancerArn")
                .and_then(|value| text(value, scope, 0, budget))
                .and_then(|parent| {
                    nested_elb_arn(
                        &parent,
                        ELB_LOAD_BALANCER_SEGMENT,
                        ELB_LISTENER_SEGMENT,
                        &service_assigned_label(ELB_LISTENER_ID_SALT, name, ELB_ID_HEX_DIGITS),
                    )
                });
            match listener_arn {
                Some(arn) => {
                    attributes.insert("ListenerArn".to_string(), arn.clone());
                    (arn, attributes)
                }
                None => (name.to_string(), attributes),
            }
        }
        // "Ref returns the rule ARN"; Fn::GetAtt: IsDefault, RuleArn. A rule's ARN nests
        // inside its listener's the same way. `IsDefault` is false for a rule a template
        // declares: a listener's default rule is created WITH the listener (it is that
        // listener's `DefaultActions`), never as a resource of its own.
        ELB_LISTENER_RULE => {
            let rule_arn = properties
                .get("ListenerArn")
                .and_then(|value| text(value, scope, 0, budget))
                .and_then(|parent| {
                    nested_elb_arn(
                        &parent,
                        ELB_LISTENER_SEGMENT,
                        ELB_LISTENER_RULE_SEGMENT,
                        &service_assigned_label(
                            ELB_LISTENER_RULE_ID_SALT,
                            name,
                            ELB_ID_HEX_DIGITS,
                        ),
                    )
                });
            attributes.insert("IsDefault".to_string(), false.to_string());
            match rule_arn {
                Some(arn) => {
                    attributes.insert("RuleArn".to_string(), arn.clone());
                    (arn, attributes)
                }
                None => (name.to_string(), attributes),
            }
        }
        // The Amazon Bedrock AgentCore types. Source for each one's identifiers: the
        // AgentCore control-plane model, whose `arnReference` traits name these very
        // resource types — `MemoryArn` references `AWS::BedrockAgentCore::Memory`,
        // `GatewayArn` `AWS::BedrockAgentCore::Gateway`, `AgentRuntimeArn`
        // `AWS::BedrockAgentCore::Runtime`, `WorkloadIdentityArnType`
        // `AWS::BedrockAgentCore::WorkloadIdentity`, and `CodeInterpreterArn` /
        // `BrowserArn` the two sandbox types — and whose create responses publish
        // the members a CloudFormation resource of that type returns. `Ref` returns the
        // resource's own identifier (the type's primary identifier) and `Fn::GetAtt` reads
        // the identifier and the ARN, each in the form the model's pattern fixes; the
        // members those responses carry that describe a resource's RUNNING state (a
        // status, a timestamp, a failure reason, a gateway's URL) are not carried, because
        // nothing here provisioned a resource to be in a state.
        //
        // "Ref returns the memory id"; Fn::GetAtt: MemoryId, MemoryArn.
        "AWS::BedrockAgentCore::Memory" => {
            let id = generated_service_id(
                &agentcore_label(name, memory_label_char, false, 'm'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("memory/{id}"),
            );
            attributes.insert("MemoryId".to_string(), id.clone());
            attributes.insert("MemoryArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the agent runtime id"; Fn::GetAtt: AgentRuntimeId,
        // AgentRuntimeArn, AgentRuntimeVersion. A create answers version `1`
        // (`AgentRuntimeVersion` is `^([1-9][0-9]{0,4})$`, and the first version of a
        // runtime is its first), which is a value of the declared resource rather than of
        // its running state.
        "AWS::BedrockAgentCore::Runtime" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'r'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("runtime/{id}"),
            );
            attributes.insert("AgentRuntimeId".to_string(), id.clone());
            attributes.insert("AgentRuntimeArn".to_string(), arn);
            attributes.insert(
                "AgentRuntimeVersion".to_string(),
                FIRST_AGENTCORE_RUNTIME_VERSION.to_string(),
            );
            (id, attributes)
        }
        // The two SANDBOX types, the same shape one resource along. `CodeInterpreterId`
        // and `BrowserId` are `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$` — the agent
        // runtime's label alphabet exactly, hence `runtime_label_char` — and their ARN
        // patterns admit `code-interpreter(-custom)?` / `browser(-custom)?`: the bare
        // segment is the service's built-in `aws.codeinterpreter.v1` /
        // `aws.browser.v1` sandbox, so a sandbox a template declares is a
        // customer-created one and carries the `-custom` segment.
        //
        // "Ref returns the code interpreter id"; Fn::GetAtt: CodeInterpreterId,
        // CodeInterpreterArn.
        "AWS::BedrockAgentCore::CodeInterpreter" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'c'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("code-interpreter-custom/{id}"),
            );
            attributes.insert("CodeInterpreterId".to_string(), id.clone());
            attributes.insert("CodeInterpreterArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the browser id"; Fn::GetAtt: BrowserId, BrowserArn.
        "AWS::BedrockAgentCore::Browser" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'b'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("browser-custom/{id}"),
            );
            attributes.insert("BrowserId".to_string(), id.clone());
            attributes.insert("BrowserArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the gateway id"; Fn::GetAtt: GatewayId, GatewayArn. A gateway
        // identifier is lower case only (`GatewayId`: `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`).
        "AWS::BedrockAgentCore::Gateway" => {
            let id = generated_service_id(
                &agentcore_label(name, gateway_label_char, true, 'g'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("gateway/{id}"),
            );
            attributes.insert("GatewayId".to_string(), id.clone());
            attributes.insert("GatewayArn".to_string(), arn);
            (id, attributes)
        }
        // Amazon Bedrock's own control plane: the guardrail an `AWS::Bedrock::Guardrail`
        // declares. "Ref returns the guardrail ARN" (the type's primary identifier);
        // Fn::GetAtt: GuardrailId, GuardrailArn, Version. Amazon Bedrock MINTS the id —
        // `CreateGuardrail` answers a `guardrailId` of `^[a-z0-9]+$`, which is what every
        // guardrail API takes as its `guardrailIdentifier` and which the guardrail's NAME
        // (`^[0-9a-zA-Z-_]+$`) is not — so the values here are STAND-INS of the forms that
        // service's own model fixes, and `super::provisioning` re-points them at what the
        // create really answered ([`super::provisioner::Provisioner::BedrockGuardrail`]).
        // The create opens the guardrail's `DRAFT` revision (`version` is `^DRAFT$`), which
        // is the version a freshly created guardrail carries.
        BEDROCK_GUARDRAIL => {
            let id = service_assigned_label(
                BEDROCK_GUARDRAIL_ID_SALT,
                name,
                BEDROCK_GUARDRAIL_ID_DIGITS,
            );
            let arn = ::emulator_server::emulator_arn(
                BEDROCK_ARN_NAMESPACE,
                region,
                &format!("guardrail/{id}"),
            );
            attributes.insert("GuardrailId".to_string(), id);
            attributes.insert("GuardrailArn".to_string(), arn.clone());
            attributes.insert(
                "Version".to_string(),
                BEDROCK_GUARDRAIL_DRAFT_VERSION.to_string(),
            );
            (arn, attributes)
        }
        // Amazon Bedrock Agent's build-time control plane: the knowledge base an
        // `AWS::Bedrock::KnowledgeBase` declares. "Ref returns the knowledge base ID" (the
        // type's primary identifier); Fn::GetAtt reads `KnowledgeBaseId`,
        // `KnowledgeBaseArn`, `Status`, `CreatedAt` and `UpdatedAt`. That service MINTS the
        // id — `CreateKnowledgeBase` answers a `knowledgeBaseId` of `^[0-9a-zA-Z]{10}$`,
        // which is what every knowledge-base API takes and which the knowledge base's NAME
        // (`^([0-9a-zA-Z][_-]?){1,100}$`) is not — so the two values here are STAND-INS of
        // the forms that service's own model fixes, and `super::provisioning` re-points them
        // at what the create really answered
        // ([`super::provisioner::Provisioner::BedrockKnowledgeBase`]). The status and the two
        // instants are that service's to report and are not computed here: they reach the
        // resource off its create's own answer.
        BEDROCK_KNOWLEDGE_BASE => {
            let id = service_assigned_label(
                BEDROCK_KNOWLEDGE_BASE_ID_SALT,
                name,
                BEDROCK_KNOWLEDGE_BASE_ID_DIGITS,
            );
            attributes.insert(
                "KnowledgeBaseArn".to_string(),
                ::emulator_server::emulator_arn(
                    BEDROCK_ARN_NAMESPACE,
                    region,
                    &format!("knowledge-base/{id}"),
                ),
            );
            attributes.insert("KnowledgeBaseId".to_string(), id.clone());
            (id, attributes)
        }
        // Amazon Braket's own control plane: the spending limit an
        // `AWS::Braket::SpendingLimit` declares. "Ref returns the spending limit ARN" (the
        // type's primary identifier); Fn::GetAtt reads `SpendingLimitArn` and `CreatedAt`.
        // Amazon Braket MINTS that ARN — `CreateSpendingLimit` takes no identifier for the
        // limit and answers the `spendingLimitArn` it assigned, whose resource path carries a
        // uuid of that service's own (`SpendingLimitArn`:
        // `^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$`) — so the value
        // here is a STAND-IN of that form, which `super::provisioning` re-points at the ARN
        // the create really answered
        // ([`super::provisioner::Provisioner::BraketSpendingLimit`]). `CreatedAt` is that
        // service's clock and is not computed here: it reaches the resource off that
        // service's own report of the limit it created.
        BRAKET_SPENDING_LIMIT => {
            let arn = ::emulator_server::emulator_arn(
                BRAKET_ARN_NAMESPACE,
                region,
                &format!(
                    "{BRAKET_SPENDING_LIMIT_PATH}/{}",
                    generated_uuid(resource_type, name)
                ),
            );
            attributes.insert("SpendingLimitArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // AWS Amplify's own control plane: the app an `AWS::Amplify::App` declares. "Ref
        // returns the ARN of the app"; `Fn::GetAtt` reads [`AMPLIFY_APP_ATTRIBUTES`]. Every
        // one of those is a value only that service can produce — the `appId` it mints, the
        // ARN built around it, the default domain it serves the app's branches under, and the
        // name as it records it — so each is a STAND-IN here, which `super::provisioning`
        // re-points at what the create really answered
        // ([`super::provisioner::Provisioner::AmplifyApp`]).
        AMPLIFY_APP => {
            for attribute in AMPLIFY_APP_ATTRIBUTES {
                attributes.insert(
                    (*attribute).to_string(),
                    service_assigned_document(name, attribute),
                );
            }
            (
                service_assigned_document(name, APP_ARN_STAND_IN_ATTRIBUTE),
                attributes,
            )
        }
        // Amplify UI Builder's own control plane: the component, the form or the theme an
        // `AWS::AmplifyUIBuilder::{Component,Form,Theme}` declares. The type's primary
        // identifier is the TRIPLE its own resource shape is identified by — the app, the
        // environment and the id — so "Ref returns" that triple joined with
        // [`COMPOSITE_IDENTITY_SEPARATOR`] ([`amplifyuibuilder_identity`]), which is also the
        // resource's physical id; `Fn::GetAtt` reads the three parts
        // ([`AMPLIFYUIBUILDER_ATTRIBUTES`]) and, for a component and a theme, the two instants
        // their structures carry read-only.
        //
        // That service MINTS the id — each create takes the app and the environment as URI
        // labels and the entity's name in its payload, takes no identifier, and answers the
        // `Uuid` it assigned, which is what every call on the entity is addressed by — so the
        // id here, and the identity built around it, are STAND-INS, which
        // `super::provisioning` re-points at what the create really answered
        // ([`super::provisioner::Provisioner::AmplifyUiBuilderComponent`] and its two
        // siblings). The app and the environment are the declaration's own values, reported as
        // the entity itself reports them.
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_FORM | AMPLIFYUIBUILDER_THEME => {
            let id = generated_uuid(resource_type, name);
            for property in [AMPLIFYUIBUILDER_APP_ID, AMPLIFYUIBUILDER_ENVIRONMENT_NAME] {
                if let Some(value) = properties
                    .get(property)
                    .and_then(|value| text(value, scope, 0, budget))
                {
                    attributes.insert(property.to_string(), value);
                }
            }
            attributes.insert(AMPLIFYUIBUILDER_ID_ATTRIBUTE.to_string(), id.clone());
            if resource_type != AMPLIFYUIBUILDER_FORM {
                for attribute in AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES {
                    attributes.insert(
                        (*attribute).to_string(),
                        service_assigned_document(name, attribute),
                    );
                }
            }
            let identity = amplifyuibuilder_identity(
                attributes
                    .get(AMPLIFYUIBUILDER_APP_ID)
                    .map(String::as_str)
                    .unwrap_or_default(),
                attributes
                    .get(AMPLIFYUIBUILDER_ENVIRONMENT_NAME)
                    .map(String::as_str)
                    .unwrap_or_default(),
                &id,
            );
            (identity, attributes)
        }
        // Amazon Managed Workflows for Apache Airflow's own control plane: the environment an
        // `AWS::MWAA::Environment` declares. "Ref returns the name of the Amazon MWAA
        // environment", which is the handle every Amazon MWAA operation on one takes, so the
        // physical id is that name; Fn::GetAtt reads the ten values
        // [`MWAA_ENVIRONMENT_ATTRIBUTES`] names. The ARN is the one of them these rules can
        // compute — both this and that service build it from the one account id this emulator
        // mints and the environment's name (`environment_arn` in its own state module) — and
        // every other is a value only Amazon MWAA produces, so each stands in until
        // `super::provisioning` has reported what that service really answered
        // ([`super::provisioner::Provisioner::MwaaEnvironment`]).
        MWAA_ENVIRONMENT => {
            attributes.insert(
                MWAA_ENVIRONMENT_ARN_ATTRIBUTE.to_string(),
                ::emulator_server::emulator_arn(
                    MWAA_ARN_NAMESPACE,
                    region,
                    &format!("{MWAA_ENVIRONMENT_PATH}/{name}"),
                ),
            );
            for attribute in MWAA_ENVIRONMENT_ATTRIBUTES
                .iter()
                .filter(|attribute| **attribute != MWAA_ENVIRONMENT_ARN_ATTRIBUTE)
            {
                attributes.insert(
                    (*attribute).to_string(),
                    service_assigned_document(name, attribute),
                );
            }
            (name.to_string(), attributes)
        }
        // "Ref returns the name"; Fn::GetAtt: Name, WorkloadIdentityArn. A workload
        // identity is named, not id'd: `CreateWorkloadIdentity` takes a `name`
        // (`^[A-Za-z0-9_.-]+$`) and answers that name plus the ARN, whose shape the model
        // leaves unpatterned, so it is built in this service's own ARN form.
        "AWS::BedrockAgentCore::WorkloadIdentity" => {
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("workload-identity-directory/default/workload-identity/{name}"),
            );
            attributes.insert("Name".to_string(), name.to_string());
            attributes.insert("WorkloadIdentityArn".to_string(), arn);
            (name.to_string(), attributes)
        }
        // "Ref returns the app monitor name"; Fn::GetAtt: Id ("The unique ID of the new
        // app monitor"). CloudWatch RUM ASSIGNS that id — the template cannot name it —
        // and its form is fixed by RUM's own Smithy model: `AppMonitorId` is
        // `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
        // `length 36..36`, so the resource reports an identifier of THAT form rather than
        // CloudFormation's `<stack>-<logical id>-<unique id>` name (the same case
        // AWS::Transfer::Server is in above).
        "AWS::RUM::AppMonitor" => {
            attributes.insert("Id".to_string(), generated_uuid("rum/appmonitor", name));
            (name.to_string(), attributes)
        }
        // "Ref returns the association ID"; Fn::GetAtt: AssociationId. Systems Manager ASSIGNS
        // that id — `CreateAssociation` takes no identifier for the association, only the
        // document it applies and the targets it applies it to — so a declaration reports an
        // identifier of that service's own shape (the 8-4-4-4-12 hexadecimal its
        // `AssociationId` fixes) rather than the document `Name` or CloudFormation's
        // `<stack>-<logical id>-<unique id>`, neither of which any Systems Manager call on an
        // association would accept. The stand-in is a function of the STACK and the LOGICAL
        // ID rather than of the resource's name, because the name a declaration of this type
        // carries is the document's and two associations of one stack may apply the same
        // document; `super::provisioning` re-points it at the id the create really answered.
        SSM_ASSOCIATION => {
            let id = generated_uuid(
                resource_type,
                &format!("{}/{}", scope.stack.stack_id, declared.logical_id),
            );
            attributes.insert(SSM_ASSOCIATION_ID_ATTRIBUTE.to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the alarm name"; Fn::GetAtt: Arn. A CloudWatch alarm's ARN spells
        // its resource with a COLON
        // (`arn:aws:cloudwatch:<region>:<account>:alarm:<alarm name>`), which is why the
        // type is catalogued rather than left to the uniform `<type>/<name>` convention.
        "AWS::CloudWatch::Alarm" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("cloudwatch", region, &format!("alarm:{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the name of the metric stream"; Fn::GetAtt: Arn, CreationDate,
        // LastUpdateDate, State. Only the ARN is a function of the resource's identity (a
        // metric stream's ARN spells its resource `metric-stream/<name>`, which is what
        // CloudWatch's own `PutMetricStream` answers); the other three are values the
        // OWNING service decides when it creates the stream — two instants of its clock and
        // the state its lifecycle is in — so they are reported only where that service
        // reports them (`super::provisioner` records what the create answered) rather than
        // minted here.
        CLOUDWATCH_METRIC_STREAM => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "cloudwatch",
                    region,
                    &format!("metric-stream/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the experiment template ID"; Fn::GetAtt: Id. FIS ASSIGNS that id —
        // the type has no name property for a template to set — so a declaration reports an
        // identifier of FIS's own shape (its `EXT` prefix and its 12-character suffix, each
        // character one FIS's own minting admits) rather than CloudFormation's
        // `<stack>-<logical id>-<unique id>` name (the case AWS::Transfer::Server and
        // AWS::RUM::AppMonitor are in). The characters here are uppercase hexadecimal, a
        // subset of the base-36 alphabet FIS mints from: a stand-in, since this is also the
        // type `super::provisioning` creates in FIS itself, which reports the id IT minted
        // back over the `Id` attribute — and THAT id, not this one, is what the stack
        // reports for a resource FIS created.
        FIS_EXPERIMENT_TEMPLATE => {
            let id = format!(
                "{FIS_TEMPLATE_ID_PREFIX}{}",
                derived_digits(
                    FIS_TEMPLATE_ID_PREFIX,
                    name,
                    FIS_TEMPLATE_ID_DIGITS,
                    Alphabet::Upper,
                )
            );
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the detector's id"; Fn::GetAtt: Id. GuardDuty ASSIGNS that id —
        // `CreateDetector` takes no identifier of any kind — so a declaration reports an
        // identifier of GuardDuty's own shape (32 lowercase hexadecimal characters) rather
        // than CloudFormation's `<stack>-<logical id>-<unique id>` name. This is also the
        // type `super::provisioning` creates in GuardDuty itself, which reports the id IT
        // minted back over the `Id` attribute — and THAT id, not this one, is what the stack
        // reports for a detector GuardDuty created.
        GUARDDUTY_DETECTOR => {
            let id = derived_digits(
                DETECTOR_ID_SALT,
                name,
                DETECTOR_ID_HEX_DIGITS,
                Alphabet::Lower,
            );
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the IPSet's id"; Fn::GetAtt: Id. GuardDuty ASSIGNS that id —
        // `CreateIPSet` takes the list's name, format and location and answers with the
        // `ipSetId` it minted — so a declaration reports an identifier of GuardDuty's own
        // shape (32 lowercase hexadecimal characters, as a detector id is) rather than
        // CloudFormation's `<stack>-<logical id>-<unique id>` name, which no GuardDuty call
        // would accept as an `IpSetId`. This is also the type `super::provisioning` creates
        // in GuardDuty itself, which reports the id IT minted back over the `Id` attribute
        // — and THAT id, not this one, is what the stack reports for a list GuardDuty
        // created.
        GUARDDUTY_IP_SET => {
            let id = derived_digits(IP_SET_ID_SALT, name, DETECTOR_ID_HEX_DIGITS, Alphabet::Lower);
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the hosted zone ID"; Fn::GetAtt: Id, NameServers. Route 53 ASSIGNS
        // that id — `CreateHostedZone` takes no identifier, and the zone's `Name` is only
        // the DNS name it answers for — so a declaration reports an identifier of Route 53's
        // own shape rather than the name, which no operation of that service would accept as
        // a zone id. `NameServers` is a LIST, so no attribute carries it. This is also the
        // type `super::provisioning` creates in Route 53 itself, which reports the id IT
        // minted back over the `Id` attribute — and THAT id, not this one, is what the stack
        // reports for a zone Route 53 created.
        ROUTE53_HOSTED_ZONE => {
            let id = format!(
                "{HOSTED_ZONE_ID_PREFIX}{}",
                derived_digits(
                    HOSTED_ZONE_ID_SALT,
                    name,
                    HOSTED_ZONE_ID_DIGITS,
                    Alphabet::Upper,
                )
            );
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the health check ID"; Fn::GetAtt: HealthCheckId. Route 53 ASSIGNS
        // that id — `CreateHealthCheck` takes no identifier, and the type declares no name
        // at all — so a declaration reports an identifier of Route 53's own shape (the
        // 8-4-4-4-12 lowercase hex a health check id takes) rather than CloudFormation's
        // `<stack>-<logical id>-<unique id>` name, which no operation of that service would
        // accept as a health check id. This is also the type `super::provisioning` creates
        // in Route 53 itself, which reports the id IT minted back over the `HealthCheckId`
        // attribute — and THAT id, not this one, is what the stack reports for a health
        // check Route 53 created.
        ROUTE53_HEALTH_CHECK => {
            let id = generated_uuid(resource_type, name);
            attributes.insert("HealthCheckId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the collection ID"; Fn::GetAtt: Arn, Id. Route 53 ASSIGNS that id —
        // `CreateCidrCollection` takes the collection's `Name` and answers the UUID it
        // minted, which is what every call on the collection takes — so a declaration
        // reports an identifier of that service's own shape (the 8-4-4-4-12 lowercase hex
        // the model's `UUID` fixes) rather than the name, and the collection's ARN is built
        // from it the way Route 53 builds one (`cidrcollection/<id>`, with no region, since
        // Route 53 is a global service). This is also the type `super::provisioning`
        // creates in Route 53 itself, which reports the id IT minted and the ARN it built
        // back over these attributes.
        ROUTE53_CIDR_COLLECTION => {
            let id = generated_uuid(resource_type, name);
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    ROUTE53_ARN_NAMESPACE,
                    "",
                    &format!("cidrcollection/{id}"),
                ),
            );
            attributes.insert("Id".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the delegation set ARN"; Fn::GetAtt: Id (and `NameServers`, a LIST,
        // which Route 53 assigns and `super::provisioning` reports off its create).
        // `CreateReusableDelegationSet` takes no identifier — the type declares only the
        // caller reference and the zone whose name servers the set is made reusable from —
        // so a declaration reports an identifier of Route 53's own shape rather than
        // CloudFormation's `<stack>-<logical id>-<unique id>` name, which no operation of
        // that service would accept as a `ResourceId`, with the set's ARN built from it the
        // way Route 53 builds one ([`delegation_set_arn`]).
        ROUTE53_DELEGATION_SET => {
            let id = format!(
                "{DELEGATION_SET_ID_PREFIX}{}",
                derived_digits(
                    DELEGATION_SET_ID_SALT,
                    name,
                    HOSTED_ZONE_ID_DIGITS,
                    Alphabet::Upper,
                )
            );
            attributes.insert("Id".to_string(), id.clone());
            (delegation_set_arn(&id), attributes)
        }
        // "Ref returns the traffic policy ARN"; Fn::GetAtt: Id, Type, Version. Route 53
        // ASSIGNS that id — `CreateTrafficPolicy` takes the policy's name and document and
        // answers the id it minted, and the policy's `Name` is only what it is called — so a
        // declaration reports an identifier of that service's own shape (the 8-4-4-4-12
        // lowercase hex a traffic policy id takes, its `TrafficPolicyId` being
        // `@length(min: 1, max: 36)`) rather than the name, which no operation of that
        // service would accept as a policy id. The other two published values are not
        // assigned at all: "for a new traffic policy, the value of `Version` is always 1"
        // (`TrafficPolicy$Version`), and the `Type` is "the DNS type of the resource record
        // sets that Amazon Route 53 creates when you use [the] traffic policy", which the
        // DOCUMENT the declaration carries states ([`declared_record_type`]).
        ROUTE53_TRAFFIC_POLICY => {
            let id = generated_uuid(resource_type, name);
            attributes.insert("Id".to_string(), id.clone());
            if let Some(record_type) = declared_record_type(properties) {
                attributes.insert("Type".to_string(), record_type);
            }
            attributes.insert(
                "Version".to_string(),
                FIRST_TRAFFIC_POLICY_VERSION.to_string(),
            );
            (traffic_policy_arn(&id), attributes)
        }
        // "Ref returns the key ID"; Fn::GetAtt: Arn, KeyId. KMS ASSIGNS that id —
        // `CreateKey` takes no identifier — so a declaration reports an identifier of KMS's
        // own shape (a version-4 UUID) rather than CloudFormation's
        // `<stack>-<logical id>-<unique id>` name, with the key ARN built from it the way
        // KMS builds one (`key/<key id>`). This is also the type `super::provisioning`
        // creates in KMS itself, which reports the id IT minted and the ARN it built back
        // over these attributes — and THOSE, not the ones computed here, are what the stack
        // reports for a key KMS created.
        KMS_KEY => {
            let id = generated_uuid(resource_type, name);
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(KMS_ARN_NAMESPACE, region, &format!("key/{id}")),
            );
            attributes.insert("KeyId".to_string(), id.clone());
            (id, attributes)
        }
        // "Ref returns the ARN of the cluster" for both kinds of Amazon MSK cluster, and
        // that ARN is the type's one published `Fn::GetAtt` attribute too. Amazon MSK
        // MINTS it: its resource path is `cluster/<name>/<id>`, where the id is the
        // service's own, so what is computed here is an ARN of that published shape with a
        // stand-in id derived from the resource's name. These are also the types
        // `super::provisioning` creates in Amazon MSK itself, which reports the ARN IT
        // minted back as the resource's identity — and THAT ARN, not this one, is what the
        // stack reports for a cluster MSK created.
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => {
            let arn = ::emulator_server::emulator_arn(
                MSK_ARN_NAMESPACE,
                region,
                &format!(
                    "cluster/{name}/{}-{MSK_CLUSTER_ARN_ORDINAL}",
                    generated_uuid(resource_type, name)
                ),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the Amazon MQ broker ID"; Fn::GetAtt: Arn ("the broker's ARN").
        // Amazon MQ MINTS that id — `CreateBroker` takes a broker NAME and answers with the
        // `brokerId` it generated — so a declaration reports an identifier of Amazon MQ's
        // own shape (`b-` followed by 128 bits in the 8-4-4-4-12 grouping its broker ids
        // carry) rather than CloudFormation's `<stack>-<logical id>-<unique id>` name, with
        // the broker ARN built from it the way Amazon MQ builds one
        // (`broker:<name>:<broker id>`). This is also the type `super::provisioning` creates
        // in Amazon MQ itself, which reports the id IT minted and the ARN it built back as
        // the resource's identity — and THOSE, not the ones computed here, are what the
        // stack reports for a broker Amazon MQ created, which is what
        // [`identity_is_service_assigned`] names this type for.
        MQ_BROKER => {
            let id = format!("{MQ_BROKER_ID_PREFIX}{}", generated_uuid(resource_type, name));
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    MQ_ARN_NAMESPACE,
                    region,
                    &format!("broker:{name}:{id}"),
                ),
            );
            (id, attributes)
        }
        // "Ref returns the backup vault name"; Fn::GetAtt: BackupVaultArn, BackupVaultName.
        // This is the type `super::provisioning` creates in AWS Backup itself, which then
        // answers with the vault's own ARN — the same ARN this computes, since both are
        // built from the one account id this emulator mints and the vault's name, which is
        // how AWS Backup keys a vault (`backup_vault_arn` in that service's own state
        // module). (A plan and a selection of the same namespace carry no arm here: each
        // one's identifier and ARN are MINTED by AWS Backup, so there is nothing for these
        // naming rules to compute — see [`identity_is_service_assigned`].)
        BACKUP_VAULT => {
            attributes.insert(
                "BackupVaultArn".to_string(),
                ::emulator_server::emulator_arn(
                    BACKUP_ARN_NAMESPACE,
                    region,
                    &format!("backup-vault:{name}"),
                ),
            );
            attributes.insert("BackupVaultName".to_string(), name.to_string());
            (name.to_string(), attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the private CA"; Fn::GetAtt: Arn,
        // CertificateSigningRequest. Both are Amazon Web Services Private CA's OWN: it
        // mints the ARN (`identity_is_service_assigned` names the type) and it builds the
        // certification request from the key it generated, so neither is a value these
        // naming rules can compute — each is a stand-in
        // ([`super::provisioning::repoint_references`] re-points it at what that service
        // answered once the CA has been created).
        ACMPCA_CERTIFICATE_AUTHORITY => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    ACMPCA_ARN_NAMESPACE,
                    region,
                    &format!("certificate-authority/{name}"),
                ),
            );
            attributes.insert(
                "CertificateSigningRequest".to_string(),
                service_assigned_document(name, "CertificateSigningRequest"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the ARN of the issued certificate"; Fn::GetAtt: Arn, Certificate.
        // The ARN carries the serial that service allocated and the body is the
        // certificate it signed, so both are stand-ins here too.
        ACMPCA_CERTIFICATE => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    ACMPCA_ARN_NAMESPACE,
                    region,
                    &format!("certificate-authority/{name}/certificate/{name}"),
                ),
            );
            attributes.insert(
                "Certificate".to_string(),
                service_assigned_document(name, "Certificate"),
            );
            (name.to_string(), attributes)
        }
        // Fn::GetAtt: CompleteCertificateChain — the certificate the CA was activated with
        // beside the chain above it, which is that service's to assemble. The resource IS
        // the activation of ONE private CA, and `CertificateAuthorityArn` is the property
        // naming it, so that ARN is what identifies the resource.
        ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION => {
            attributes.insert(
                "CompleteCertificateChain".to_string(),
                service_assigned_document(name, "CompleteCertificateChain"),
            );
            let identity = properties
                .get("CertificateAuthorityArn")
                .and_then(|declared| text(declared, scope, 0, budget))
                .unwrap_or_else(|| name.to_string());
            (identity, attributes)
        }
        // The two Hook types a template declares a CloudFormation Hook with. "Ref returns the
        // Hook ARN" and `Fn::GetAtt: HookArn` returns that same ARN — the registry ARN form an
        // activated Hook of this account and Region is addressed by (`type/hook/<Type-Name>`,
        // built by [`crate::state::registry::type_arn`], so the output a stack publishes is
        // the identifier the extension APIs take for a Hook rather than a name of
        // CloudFormation's own).
        //
        // The Hook's own type name is the `Alias` its declaration states ("the type name
        // alias for the Hook … must be in the form `Private::Hook::MyCustomHook`"), which is
        // what makes the ARN name the Hook a client configures rather than the logical id of
        // the resource that declared it; a declaration stating none leaves the ARN on
        // CloudFormation's own generated name for the resource, exactly as every other type
        // whose name is a property does.
        CLOUDFORMATION_GUARD_HOOK | CLOUDFORMATION_LAMBDA_HOOK => {
            let alias = properties
                .get("Alias")
                .and_then(|declared| text(declared, scope, 0, budget))
                .filter(|alias| !alias.is_empty())
                .unwrap_or_else(|| name.to_string());
            let arn = crate::state::registry::type_arn(
                region,
                crate::state::registry::registry_type::HOOK,
                &alias,
            );
            attributes.insert("HookArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the record name"; the type publishes no `Fn::GetAtt` attribute. The
        // record name CloudFormation reports is the DNS name WITHOUT its terminal dot: a
        // zone's own names are fully qualified in Route 53 ("Route 53 treats
        // `www.example.com` and `www.example.com.` as identical"), while the value this type
        // hands back is the name in the form a client uses it in — joined into a URL, or
        // compared with the name the template declared minus the dot. The name the record is
        // CREATED under is the declaration's own ([`crate::state::provisioner`]), so the
        // trailing dot is dropped only from the identifier the stack reports.
        ROUTE53_RECORD_SET => (
            without_terminal_dot(name).to_string(),
            attributes,
        ),
        _ => (name.to_string(), attributes),
    }
}

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
        ACMPCA_CERTIFICATE_AUTHORITY => &["Arn", "CertificateSigningRequest"],
        ACMPCA_CERTIFICATE => &["Arn", "Certificate"],
        ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION => &["CompleteCertificateChain"],
        BACKUP_VAULT => &["BackupVaultArn", "BackupVaultName"],
        "AWS::SNS::Topic" => &["TopicArn", "TopicName"],
        SQS_QUEUE => &["Arn", "QueueName", "QueueUrl"],
        // Each of the three SageMaker AI types publishes the one attribute that carries the
        // resource's own name.
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL => &["ModelName"],
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE => {
            &["NotebookInstanceName"]
        }
        crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            &["NotebookInstanceLifecycleConfigName"]
        }
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
        "AWS::DynamoDB::Table" => &["Arn"],
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
        ECS_CLUSTER => &["Arn"],
        EKS_CLUSTER => &[
            "Arn",
            "CertificateAuthorityData",
            "ClusterSecurityGroupId",
            "EncryptionConfigKeyArn",
            "Endpoint",
            "OpenIdConnectIssuerUrl",
        ],
        EKS_POD_IDENTITY_ASSOCIATION => &["AssociationArn", "AssociationId"],
        SSM_ASSOCIATION => &[SSM_ASSOCIATION_ID_ATTRIBUTE],
        BATCH_COMPUTE_ENVIRONMENT => &["ComputeEnvironmentArn"],
        BATCH_JOB_QUEUE => &["JobQueueArn"],
        BATCH_SERVICE_ENVIRONMENT => &["ServiceEnvironmentArn"],
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
        "AWS::RDS::DBCluster" => &[
            "DBClusterArn",
            "DBClusterResourceId",
            "Endpoint.Address",
            "Endpoint.Port",
            "ReadEndpoint.Address",
        ],
        "AWS::RDS::DBInstance" => &[
            "DBInstanceArn",
            "DbiResourceId",
            "Endpoint.Address",
            "Endpoint.Port",
        ],
        SECRETSMANAGER_SECRET => &["Id"],
        EC2_VPC => &["VpcId"],
        EC2_SUBNET => &["SubnetId"],
        EC2_SECURITY_GROUP => &["GroupId"],
        EC2_INTERNET_GATEWAY => &["InternetGatewayId"],
        EC2_ROUTE_TABLE => &["RouteTableId"],
        EC2_EIP => &["AllocationId"],
        EC2_NAT_GATEWAY => &["NatGatewayId"],
        EC2_DHCP_OPTIONS => &["DhcpOptionsId"],
        EC2_VPN_GATEWAY => &["VPNGatewayId"],
        EC2_NETWORK_ACL | EC2_NETWORK_INTERFACE | EC2_VPC_ENDPOINT => &["Id"],
        EC2_VOLUME => &["VolumeId"],
        // Source: `AWS::EC2::LaunchTemplate`'s own "Return values" section — the id EC2
        // assigned the template, and the default and latest version numbers of it.
        EC2_LAUNCH_TEMPLATE => &[
            "DefaultVersionNumber",
            "LatestVersionNumber",
            "LaunchTemplateId",
        ],
        // Source: `AWS::EC2::Instance`'s own "Return values" section — the zone the instance
        // was placed in, its private DNS name and IPv4 address, and its public ones. "Ref
        // returns the ID of the instance", which is the resource's physical id rather than one
        // of these attributes.
        EC2_INSTANCE => &[
            "AvailabilityZone",
            "PrivateDnsName",
            "PrivateIp",
            "PublicDnsName",
            "PublicIp",
        ],
        ECS_SERVICE => &["Name"],
        ELB_LISTENER => &["ListenerArn"],
        ELB_LISTENER_RULE => &["IsDefault", "RuleArn"],
        BEDROCK_GUARDRAIL => &["GuardrailArn", "GuardrailId", "Version"],
        BEDROCK_KNOWLEDGE_BASE => &["KnowledgeBaseArn", "KnowledgeBaseId"],
        // A spending limit's ARN is the one return value these naming rules can compute; its
        // `CreatedAt` is Amazon Braket's clock, which reaches the resource off that
        // service's own report of the limit rather than from here, exactly as a knowledge
        // base's instants do.
        BRAKET_SPENDING_LIMIT => &["SpendingLimitArn"],
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_THEME => AMPLIFYUIBUILDER_TIMED_ATTRIBUTES,
        AMPLIFYUIBUILDER_FORM => AMPLIFYUIBUILDER_ATTRIBUTES,
        AMPLIFY_APP => AMPLIFY_APP_ATTRIBUTES,
        MWAA_ENVIRONMENT => MWAA_ENVIRONMENT_ATTRIBUTES,
        "AWS::BedrockAgentCore::Memory" => &["MemoryArn", "MemoryId"],
        "AWS::BedrockAgentCore::Runtime" => {
            &["AgentRuntimeArn", "AgentRuntimeId", "AgentRuntimeVersion"]
        }
        "AWS::BedrockAgentCore::CodeInterpreter" => &["CodeInterpreterArn", "CodeInterpreterId"],
        "AWS::BedrockAgentCore::Browser" => &["BrowserArn", "BrowserId"],
        "AWS::BedrockAgentCore::Gateway" => &["GatewayArn", "GatewayId"],
        "AWS::BedrockAgentCore::WorkloadIdentity" => &["Name", "WorkloadIdentityArn"],
        "AWS::RUM::AppMonitor" => &["Id"],
        "AWS::CloudWatch::Alarm" => &["Arn"],
        CODEBUILD_REPORT_GROUP => &["Arn"],
        CODEBUILD_PROJECT => &["Arn"],
        CLOUDWATCH_METRIC_STREAM => &["Arn"],
        CODEARTIFACT_DOMAIN => &["Arn", "Name", "Owner"],
        CODEARTIFACT_REPOSITORY => &["Arn", "DomainOwner", "Name"],
        COGNITO_USER_POOL => &["Arn", "ProviderName", "ProviderURL"],
        COGNITO_USER_POOL_CLIENT => &["ClientSecret"],
        FIS_EXPERIMENT_TEMPLATE => &["Id"],
        CLOUDFRONT_DISTRIBUTION => &["DomainName", "Id"],
        CLOUDFRONT_ORIGIN_ACCESS_CONTROL => &["Id"],
        CLOUDFRONT_ORIGIN_ACCESS_IDENTITY => &["Id", "S3CanonicalUserId"],
        CLOUDFRONT_CACHE_POLICY | CLOUDFRONT_ORIGIN_REQUEST_POLICY
        | CLOUDFRONT_RESPONSE_HEADERS_POLICY => &["Id"],
        APIGATEWAY_REST_API => &["RestApiId", "RootResourceId"],
        APIGATEWAY_RESOURCE => &["ResourceId"],
        APIGATEWAY_DEPLOYMENT => &["DeploymentId"],
        APIGATEWAY_API_KEY => &["APIKeyId"],
        APIGATEWAY_AUTHORIZER => &["AuthorizerId"],
        APIGATEWAY_ACCOUNT | APIGATEWAY_USAGE_PLAN | APIGATEWAY_USAGE_PLAN_KEY => &["Id"],
        APIGATEWAYV2_API => &["ApiEndpoint"],
        APIGATEWAYV2_API_MAPPING => &["ApiMappingId"],
        APIGATEWAYV2_DOMAIN_NAME => &["DomainNameArn", "RegionalDomainName"],
        GUARDDUTY_DETECTOR | GUARDDUTY_IP_SET => &["Id"],
        ROUTE53_HEALTH_CHECK => &["HealthCheckId"],
        ROUTE53_HOSTED_ZONE => &["Id"],
        ROUTE53_CIDR_COLLECTION => &["Arn", "Id"],
        ROUTE53_DELEGATION_SET => &["Id"],
        ROUTE53_TRAFFIC_POLICY => &["Id", "Type", "Version"],
        KMS_KEY => &["Arn", "KeyId"],
        MSK_CLUSTER | MSK_SERVERLESS_CLUSTER => &["Arn"],
        MQ_BROKER => &["Arn"],
        EVENTS_RULE => &["Arn"],
        SSM_PARAMETER => &["Type", "Value"],
        STEP_FUNCTIONS_ACTIVITY => &["Arn", "Name"],
        STEP_FUNCTIONS_STATE_MACHINE => &["Arn", "Name"],
        SCHEDULER_SCHEDULE_GROUP | SCHEDULER_SCHEDULE => &["Arn"],
        RESOURCE_GROUPS_GROUP => &["Arn"],
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
/// identifier is the stage name it states.)
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
        // A VPC LINK is one more, for the reason its own constant records
        // ([`APIGATEWAY_VPC_LINK`]): `CreateVpcLink` takes no identifier and answers the id
        // API Gateway assigned, every call on a link — and the private integration that
        // reaches a VPC through it — takes that id, and a template declares the link's name
        // rather than its identifier.
        || resource_type == APIGATEWAY_VPC_LINK
        // An Amazon EKS pod identity association is one more: that create takes no
        // identifier and answers the ARN and the id Amazon EKS minted for the association,
        // and "Ref returns the association ARN" — a template declares the cluster, the
        // namespace, the service account and the role, never the association's own id.
        || resource_type == EKS_POD_IDENTITY_ASSOCIATION
        // A State Manager association is one more: `CreateAssociation` takes no identifier
        // for the association — the `Name` a declaration states names the DOCUMENT the
        // association applies, and Systems Manager would refuse it as an association id —
        // and answers the `AssociationId` it minted, which is what "Ref returns the
        // association ID" publishes and what `DescribeAssociation`, `UpdateAssociation` and
        // `DeleteAssociation` each take.
        || resource_type == SSM_ASSOCIATION
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
        // An Amazon SNS subscription's identity is the ARN `Subscribe` answers with: it
        // carries an id SNS mints per subscription, which no naming rule could compute.
        || resource_type == SNS_SUBSCRIPTION
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
        || resource_type == FIS_EXPERIMENT_TEMPLATE
        || resource_type == WAF_REGIONAL_IP_SET
        || resource_type == WAFV2_WEB_ACL
        || resource_type == WAFV2_RULE_GROUP
        || resource_type == WAFV2_IP_SET
        || resource_type == WAFV2_REGEX_PATTERN_SET
        || resource_type == GRAFANA_WORKSPACE
        || resource_type == EMR_CLUSTER
        || resource_type == GUARDDUTY_DETECTOR
        // The two CloudFront types this emulator provisions are the same case: neither
        // `CreateDistribution` nor `CreateOriginAccessControl` takes an identifier — a
        // template declares a configuration, never an id — and each answers the `E…`
        // identifier CloudFront minted, which is what "Ref returns the distribution ID" and
        // "Ref returns the origin access control's unique identifier" publish and what every
        // CloudFront call on the resource takes.
        || resource_type == CLOUDFRONT_DISTRIBUTION
        || resource_type == CLOUDFRONT_ORIGIN_ACCESS_CONTROL
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
        || resource_type == KMS_KEY
        || resource_type == AOSS_COLLECTION
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
        // An Amazon Braket SPENDING LIMIT is one more: `CreateSpendingLimit` takes no
        // identifier for the limit — the declaration states the device, the amount and the
        // period — and answers the `spendingLimitArn` that service minted, whose resource
        // path carries a uuid of its own, so the identifier in force is the one it answered.
        // That ARN is also what every call on a limit takes (`UpdateSpendingLimit` and
        // `DeleteSpendingLimit` each address one by it).
        || resource_type == BRAKET_SPENDING_LIMIT
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
}

/// The type of an IAM access key: the long-term credential of an IAM user, under the
/// namespace the IAM model's own `awsService.cloudFormationName` (`IAM`) spells.
///
/// "Ref returns the AWS::IAM::AccessKey resource's access key ID", and `Fn::GetAtt` reads
/// `SecretAccessKey` — "the secret access key for the specified AWS::IAM::AccessKey
/// resource". IAM mints both, which is why [`identity_is_service_assigned`] names this type
/// and why neither value is computed here ([`return_values`]).
pub const IAM_ACCESS_KEY: &str = "AWS::IAM::AccessKey";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Data Firehose stream.
/// The middle segment is the Firehose model's own `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model).
pub const FIREHOSE_DELIVERY_STREAM: &str = "AWS::KinesisFirehose::DeliveryStream";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Kinesis data stream — a
/// different service from the Firehose one above, whose middle segment is the Kinesis
/// model's own `awsService.cloudFormationName` (`Kinesis`).
///
/// "Ref returns the stream name", which is the name the owning service creates the stream
/// under, so a GENERATED one is held to that service's own rules for it ([`naming_rules`])
/// and the identity is not service-assigned.
pub const KINESIS_STREAM: &str = "AWS::Kinesis::Stream";

/// The `AWS::<cloudFormationName>::<Resource>` types of the Amazon Bedrock resources an
/// agent estate declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`Bedrock`).
///
/// Each one's owning API — Amazon Bedrock Agent's build-time service — identifies the
/// resource by a ten-character identifier IT mints (`CreateKnowledgeBase` answers a
/// `knowledgeBaseId`, `CreateAgent` an `agentId`) and takes a new name on the matching
/// update, which is why [`name_is_create_only`] names these types.
pub const BEDROCK_KNOWLEDGE_BASE: &str = "AWS::Bedrock::KnowledgeBase";
pub const BEDROCK_AGENT: &str = "AWS::Bedrock::Agent";
pub const BEDROCK_AGENT_ALIAS: &str = "AWS::Bedrock::AgentAlias";
pub const BEDROCK_DATA_SOURCE: &str = "AWS::Bedrock::DataSource";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Bedrock GUARDRAIL, the one
/// resource of that estate whose owning API is Bedrock's own control plane
/// (`CreateGuardrail`) rather than the agent build-time one.
pub const BEDROCK_GUARDRAIL: &str = "AWS::Bedrock::Guardrail";

/// Amazon Bedrock's ARN namespace, which is that model's own `awsService.arnNamespace`
/// (`bedrock`) — the namespace a guardrail ARN carries
/// (`arn:aws:bedrock:<region>:<account>:guardrail/<id>`, from `GuardrailArn`'s pattern).
pub const BEDROCK_ARN_NAMESPACE: &str = "bedrock";

/// The salt the guardrail-id stand-in is derived under, and how many characters it carries:
/// `GuardrailId` is `^[a-z0-9]+$` with `@length(max: 64)`, so the stand-in is lower-case
/// alphanumeric and carries no separator. Twelve characters is the length Amazon Bedrock's
/// own minted ids carry.
const BEDROCK_GUARDRAIL_ID_SALT: &str = "bedrock/guardrail";
const BEDROCK_GUARDRAIL_ID_DIGITS: usize = 12;

/// The version a freshly created guardrail carries: `CreateGuardrail`'s `version` is
/// `^DRAFT$` ("This value will always be DRAFT").
const BEDROCK_GUARDRAIL_DRAFT_VERSION: &str = "DRAFT";

/// The salt the knowledge-base-id stand-in is derived under, and how many characters it
/// carries: a `knowledgeBaseId` is `^[0-9a-zA-Z]{10}$`, so the stand-in is exactly ten
/// alphanumeric characters and carries no separator.
const BEDROCK_KNOWLEDGE_BASE_ID_SALT: &str = "bedrock/knowledge-base";
const BEDROCK_KNOWLEDGE_BASE_ID_DIGITS: usize = 10;

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Braket SPENDING LIMIT: the
/// cost control that service's own `CreateSpendingLimit` makes for a quantum device. The
/// middle segment is that model's own `awsService.sdkId` (`Braket`).
///
/// "Ref returns the spending limit ARN", and `Fn::GetAtt` reads `SpendingLimitArn` and
/// `CreatedAt`. Amazon Braket mints that ARN and keeps that instant, which is why
/// [`identity_is_service_assigned`] names this type and why neither value can be recomputed
/// from the template.
pub const BRAKET_SPENDING_LIMIT: &str = "AWS::Braket::SpendingLimit";

/// Amazon Braket's ARN namespace, which is the namespace a spending-limit ARN carries
/// (`SpendingLimitArn`: `^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$`).
pub const BRAKET_ARN_NAMESPACE: &str = "braket";

/// The resource path a spending-limit ARN carries before the identifier Amazon Braket minted
/// for it, from that same pattern.
const BRAKET_SPENDING_LIMIT_PATH: &str = "spending-limit";

/// The `AWS::<cloudFormationName>::<Resource>` type of an AWS Amplify APP: the hosting
/// application that service's own `CreateApp` makes, and the parent every other Amplify
/// resource hangs off. The middle segment is that model's own `awsService.cloudFormationName`
/// (`Amplify`).
///
/// "Ref returns the ARN of the app" — the type's primary identifier — and `Fn::GetAtt` reads
/// [`AMPLIFY_APP_ATTRIBUTES`]. AWS Amplify MINTS the `appId` (`^d[a-z0-9]+$`, which no name
/// CloudFormation decides satisfies) and the ARN built around it, and serves the app's branches
/// under a default domain of its own, so none of those values can be recomputed from the
/// template: that is what [`identity_is_service_assigned`] names this type for.
pub const AMPLIFY_APP: &str = "AWS::Amplify::App";

/// Every `Fn::GetAtt` attribute an [`AMPLIFY_APP`] publishes, in the sorted order a resource's
/// own attribute map holds them in: the id that service minted, the app's name, its ARN and the
/// default domain it is served under — each of them a value only AWS Amplify can produce, so
/// each stands in ([`service_assigned_document`]) until that service has reported what it
/// really answered.
pub(super) const AMPLIFY_APP_ATTRIBUTES: &[&str] =
    &["AppId", "AppName", "Arn", "DefaultDomain"];

/// The `AWS::<cloudFormationName>::<Resource>` types of the three user-interface entities of
/// an Amplify app environment: the component, the form and the theme Amplify UI
/// Builder's own `CreateComponent`, `CreateForm` and `CreateTheme` make. The middle segment is
/// that model's own `awsService.cloudFormationName` (`AmplifyUIBuilder`) and the resource
/// segment each entity's own `@cfnResource` name (`Component`, `Form`, `Theme`).
///
/// Each type's identity is that service's to mint: every one of those creates takes the app
/// and the environment as its URI labels and the entity's NAME in its payload, takes no
/// identifier at all, and answers the `id` Amplify UI Builder assigned — a `Uuid`, which is
/// what `GetComponent`, `UpdateForm`, `DeleteTheme` and every other call on one is addressed
/// by. That is what [`identity_is_service_assigned`] names all three for.
///
/// The values a resource of one of these types publishes are the three identifiers of its own
/// resource shape (`smithy_query(query="shape", shape="ComponentResource")`:
/// `identifiers: {id, appId, environmentName}`) — the app and the environment the declaration
/// states, and the id that service minted — beside the members the entity itself carries as
/// READ-ONLY (`cfnMutability: "read"`), which are the two instants a component and a theme are
/// reported with ([`AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES`]); a form's own structure carries
/// neither.
pub const AMPLIFYUIBUILDER_COMPONENT: &str = "AWS::AmplifyUIBuilder::Component";
pub const AMPLIFYUIBUILDER_FORM: &str = "AWS::AmplifyUIBuilder::Form";
pub const AMPLIFYUIBUILDER_THEME: &str = "AWS::AmplifyUIBuilder::Theme";

/// Every `Fn::GetAtt` attribute an `AWS::AmplifyUIBuilder::Form` publishes, in the sorted order
/// a resource's own attribute map holds them in: the identifiers of the type's resource shape
/// ([`AMPLIFYUIBUILDER_COMPONENT`]).
///
/// Two of them are the declaration's own values, and the `Id` is the one value only Amplify UI
/// Builder can produce, so it stands in as a uuid of that member's own form
/// ([`generated_uuid`]) until `super::provisioning` has reported the id the create really
/// answered.
pub(super) const AMPLIFYUIBUILDER_ATTRIBUTES: &[&str] = &["AppId", "EnvironmentName", "Id"];

/// The two further attributes a COMPONENT and a THEME publish: the instants their own
/// structures carry `cfnMutability: "read"` on (`Component$createdAt`, `Component$modifiedAt`
/// and the same pair on `Theme`), which is that model's own statement that they are read-only
/// properties of the type. A `Form`'s structure carries neither member, so a form publishes
/// only [`AMPLIFYUIBUILDER_ATTRIBUTES`].
///
/// Both are Amplify UI Builder's clock, so neither is computed here: each stands in
/// ([`service_assigned_document`]) until that service has reported the entity it created.
const AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES: &[&str] = &["CreatedAt", "ModifiedAt"];

/// [`AMPLIFYUIBUILDER_ATTRIBUTES`] and [`AMPLIFYUIBUILDER_INSTANT_ATTRIBUTES`] together, in
/// that sorted order: what a component and a theme publish.
const AMPLIFYUIBUILDER_TIMED_ATTRIBUTES: &[&str] =
    &["AppId", "CreatedAt", "EnvironmentName", "Id", "ModifiedAt"];

/// The properties a declaration of one of those types states the app environment the entity
/// belongs to in, which are also two of the values it publishes
/// ([`AMPLIFYUIBUILDER_ATTRIBUTES`]).
const AMPLIFYUIBUILDER_APP_ID: &str = "AppId";
const AMPLIFYUIBUILDER_ENVIRONMENT_NAME: &str = "EnvironmentName";

/// The attribute carrying the identifier Amplify UI Builder mints for the entity.
const AMPLIFYUIBUILDER_ID_ATTRIBUTE: &str = "Id";

/// The physical resource id of one of those three entities: the TRIPLE its own resource shape
/// is identified by — the app, the environment and the id that service minted — joined with
/// [`COMPOSITE_IDENTITY_SEPARATOR`], which is what the type's `Ref` returns.
///
/// Not a row of [`composite_identity_properties`]: that one builds an identity out of the
/// values a declaration STATES, and the last part of this one is the id Amplify UI Builder
/// assigns, which no declaration carries. So the identity is built here as a stand-in and
/// again by [`crate::state::provisioner`] from the entity that service answered — the one
/// place each of the three parts is really known.
pub(crate) fn amplifyuibuilder_identity(app_id: &str, environment_name: &str, id: &str) -> String {
    [app_id, environment_name, id].join(COMPOSITE_IDENTITY_SEPARATOR)
}

/// The three parts of such an identity, where `identity` is one
/// ([`amplifyuibuilder_identity`]); `None` for a value that is not.
pub(crate) fn amplifyuibuilder_identity_parts(identity: &str) -> Option<(&str, &str, &str)> {
    let mut parts = identity.split(COMPOSITE_IDENTITY_SEPARATOR);
    let (app_id, environment_name, id) = (parts.next()?, parts.next()?, parts.next()?);
    parts.next().is_none().then_some((app_id, environment_name, id))
}

/// The attribute of [`AMPLIFY_APP_ATTRIBUTES`] that carries the app's ARN, which is the
/// identity a `Ref` to one resolves to.
const APP_ARN_STAND_IN_ATTRIBUTE: &str = "Arn";

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Managed Workflows for
/// Apache Airflow ENVIRONMENT: the Apache Airflow deployment that service's own
/// `CreateEnvironment` makes. The middle segment is that model's own `awsService.sdkId`
/// (`MWAA`).
///
/// "Ref returns the name of the Amazon MWAA environment", which is the name the owning
/// service creates the environment under — so a GENERATED one is held to that service's own
/// rules for it ([`naming_rules`]) and the identity is not service-assigned.
pub const MWAA_ENVIRONMENT: &str = "AWS::MWAA::Environment";

/// Amazon MWAA's ARN namespace (that model's `awsService.arnNamespace`), which is `airflow`
/// rather than the `MWAA` its resource type is spelled with — the namespace an environment
/// ARN carries (`EnvironmentArn`:
/// `^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+`).
const MWAA_ARN_NAMESPACE: &str = "airflow";

/// The resource path an environment ARN carries before the environment's name, from that
/// same pattern.
const MWAA_ENVIRONMENT_PATH: &str = "environment";

/// Every `Fn::GetAtt` attribute an `AWS::MWAA::Environment` publishes, in the sorted order a
/// resource's own attribute map holds them in.
///
/// Source: the type's own "Return values" section — the environment's ARN, the Apache Airflow
/// web server's URL, the Celery Executor's queue, the two VPC endpoint services, and the
/// CloudWatch Logs group each Apache Airflow log type publishes to, which that section spells
/// `LoggingConfiguration.<log type>.CloudWatchLogGroupArn`.
///
/// Only the ARN is a value these naming rules can compute. Every other one is Amazon MWAA's
/// own — the web server host it brings up, the queue and the endpoint services it makes for
/// the environment, the log groups it creates for the log types the declaration enabled — so
/// each stands in ([`service_assigned_document`]) until that service has reported what it
/// really answered.
const MWAA_ENVIRONMENT_ATTRIBUTES: &[&str] = &[
    "Arn",
    "CeleryExecutorQueue",
    "DatabaseVpcEndpointService",
    "LoggingConfiguration.DagProcessingLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WebserverLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WorkerLogs.CloudWatchLogGroupArn",
    "WebserverUrl",
    "WebserverVpcEndpointService",
];

/// The one attribute of [`MWAA_ENVIRONMENT_ATTRIBUTES`] these naming rules compute rather
/// than stand in for.
const MWAA_ENVIRONMENT_ARN_ATTRIBUTE: &str = "Arn";

/// The `AWS::<cloudFormationName>::<Resource>` types of the two OpenSearch Serverless
/// resources a stack creates. The middle segment is that service's own
/// `awsService.cloudFormationName` (`OpenSearchServerless`).
pub const AOSS_COLLECTION: &str = "AWS::OpenSearchServerless::Collection";
pub const AOSS_SECURITY_POLICY: &str = "AWS::OpenSearchServerless::SecurityPolicy";

/// The type of an Amazon OpenSearch Service domain — a different service from the
/// serverless one above, whose own model is reached by the `es` signing name. Its middle
/// segment is the one the resource-type namespace publishes (`OpenSearchService`) rather
/// than that model's shorter `awsService.cloudFormationName` (`OpenSearch`).
///
/// "Ref returns the domain name", which is the name the owning service creates the domain
/// under — so a GENERATED one is held to that service's own rules for it
/// ([`naming_rules`]) and the identity is not service-assigned.
pub const OPENSEARCH_DOMAIN: &str = "AWS::OpenSearchService::Domain";

/// The type of an Amazon S3 bucket, whose middle segment is that model's own
/// `cloudFormationName` the same way. "Ref returns the bucket name" for this type, so a
/// bucket's physical resource id is the name its owning service creates it under — which
/// is why [`naming_rules`] holds a GENERATED one to S3's bucket naming rules.
pub const S3_BUCKET: &str = "AWS::S3::Bucket";

/// The type of an Amazon S3 TABLE bucket, the third bucket variant beside `AWS::S3::Bucket`
/// and `AWS::S3Vectors::VectorBucket`. The middle segment is that model's own sdk identity
/// (`smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/\
/// com-amazonaws-s3tables-s3tablebuckets.json")` reports `sdkId: S3Tables`).
///
/// This type's primary identifier is its own ARN (`bucket/<name>` under the ARN namespace
/// below), so `Ref` resolves to the ARN its owning service keys the bucket by — which is
/// what `GetTableBucket`, `PutTableBucketPolicy` and every other S3 Tables operation on a
/// table bucket take as `tableBucketARN`.
pub const S3TABLES_TABLE_BUCKET: &str = "AWS::S3Tables::TableBucket";

/// Amazon S3 Tables' ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`s3tables`) rather than the `S3Tables` its resource types are spelled with.
pub const S3TABLES_ARN_NAMESPACE: &str = "s3tables";

/// The segment a table bucket occupies in an Amazon S3 Tables ARN, from the
/// `TableBucketARN` pattern that model declares
/// (`…:s3tables:<region>:<account>:bucket/<name>`).
///
/// The same literal as the Amazon S3 Vectors one beside it, and a separate constant all the
/// same: each is its own model's statement about its own ARNs, so neither follows from the
/// other and a change to one is not a change to both.
pub const S3TABLES_BUCKET_SEGMENT: &str = "bucket/";

/// The one `Fn::GetAtt` attribute an `AWS::S3Tables::TableBucket` publishes, spelled with
/// the upper-case `ARN` its own model spells the member with (`TableBucketARN`).
pub const S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE: &str = "TableBucketARN";

/// The types of the two Amazon S3 Vectors resources a stack creates. The middle segment is
/// that model's own sdk identity (`smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/\
/// com-amazonaws-s3vectors-s3vectors.json")` reports `sdkId: S3Vectors`), beside the
/// sibling bucket variants `AWS::S3::Bucket` and `AWS::S3Tables::TableBucket`.
///
/// Each type's primary identifier is its own ARN — a vector bucket is
/// `bucket/<name>` and an index `bucket/<bucket>/index/<name>` under the ARN namespace
/// below — so `Ref` resolves to the ARN its owning service keys the resource by, which is
/// what `GetVectorBucket`/`GetIndex` take as `vectorBucketArn`/`indexArn`.
pub const S3VECTORS_VECTOR_BUCKET: &str = "AWS::S3Vectors::VectorBucket";
pub const S3VECTORS_INDEX: &str = "AWS::S3Vectors::Index";

/// Amazon S3 Vectors' ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`s3vectors`) rather than the `S3Vectors` its resource types are spelled with.
pub const S3VECTORS_ARN_NAMESPACE: &str = "s3vectors";

/// The segment a vector bucket occupies in an Amazon S3 Vectors ARN, from the
/// `VectorBucketArn` pattern that model declares
/// (`…:s3vectors:<region>:<account>:bucket/<name>`).
pub const S3VECTORS_BUCKET_SEGMENT: &str = "bucket/";

/// The type of an Amazon EMR cluster: the EMR model carries
/// `awsService.cloudFormationName: EMR`. "Ref returns the cluster ID", the `j-` job-flow id
/// `RunJobFlow` mints, which is why [`identity_is_service_assigned`] names this type.
pub const EMR_CLUSTER: &str = "AWS::EMR::Cluster";

/// The type of an AWS Transfer Family server: that model carries
/// `awsService.cloudFormationName: Transfer`. "Ref returns the Amazon Resource Name (ARN)
/// of the server", and `Fn::GetAtt` reads `Arn` and `ServerId` — "the service-assigned ID
/// of the server that is created". Transfer Family MINTS both (`CreateServer` takes no
/// identifier and answers the `ServerId` it assigned), which is why
/// [`identity_is_service_assigned`] names this type: the ARN a `Ref` resolves to is the one
/// that service created the server under, not one these naming rules computed.
pub const TRANSFER_SERVER: &str = "AWS::Transfer::Server";

/// The types of the two kinds of Amazon MSK cluster a stack creates: the Amazon MSK model
/// carries `awsService.cloudFormationName: MSK`, and the namespace publishes one type per
/// kind of cluster — a provisioned one, whose brokers the declaration sizes, and a
/// serverless one, which has none.
///
/// For both, "Ref returns the ARN of the cluster", which is also each type's one published
/// `Fn::GetAtt` attribute. Amazon MSK MINTS that ARN (its resource path is
/// `cluster/<name>/<id>`, where the id is the service's own), which is why
/// [`identity_is_service_assigned`] names both types.
pub const MSK_CLUSTER: &str = "AWS::MSK::Cluster";
pub const MSK_SERVERLESS_CLUSTER: &str = "AWS::MSK::ServerlessCluster";

/// Amazon MSK's ARN namespace, which is its model's own `awsService.arnNamespace`
/// (`kafka`) rather than the `MSK` its resource types are spelled with.
pub const MSK_ARN_NAMESPACE: &str = "kafka";

/// The type of an Amazon MQ broker: that middle segment is the Amazon MQ model's own
/// `awsService.cloudFormationName` (`AmazonMQ`).
///
/// "Ref returns the Amazon MQ broker ID", and `Fn::GetAtt` reads `Arn` — the broker's own
/// ARN. Amazon MQ MINTS that id (`CreateBroker` takes the broker's name and answers with
/// the `brokerId` it generated, which is the `@httpLabel` every operation on a broker is
/// addressed by), which is why [`identity_is_service_assigned`] names this type: the id a
/// `Ref` resolves to is the one that service created the broker under, not one these naming
/// rules computed.
pub const MQ_BROKER: &str = "AWS::AmazonMQ::Broker";

/// Amazon MQ's ARN namespace, which is its model's own `awsService.arnNamespace` (`mq`)
/// rather than the `AmazonMQ` its resource type is spelled with.
pub const MQ_ARN_NAMESPACE: &str = "mq";

/// What an Amazon MQ broker id begins with (`b-` followed by 128 bits in the 8-4-4-4-12
/// grouping `BrokerId`'s own documented form carries).
///
/// The id built on it here is a PLACEHOLDER, and one that is never reported for a broker
/// that exists: the id a created broker carries is the one Amazon MQ minted, which
/// [`super::provisioning::reconcile`] adopts over this one. What this value is for is the
/// shape of the identifier a declaration reports BEFORE its create answers.
pub(super) const MQ_BROKER_ID_PREFIX: &str = "b-";

/// The 1-based ordinal an Amazon MSK cluster ARN ends with
/// (`cluster/<name>/<id>-<ordinal>`), as the stand-in identifier computed here carries it.
///
/// A PLACEHOLDER, and one that is never reported for a cluster that exists: the ARN a
/// created cluster carries is the one Amazon MSK minted, where the ordinal is that
/// service's own count of the clusters it has registered, and
/// [`super::provisioning::reconcile`] adopts that ARN over this one. What this value is
/// for is the shape of the identifier a declaration reports BEFORE its create answers.
pub(super) const MSK_CLUSTER_ARN_ORDINAL: u32 = 1;

/// The same for an Amazon SQS queue: the SQS model carries
/// `awsService.cloudFormationName: SQS`.
pub const SQS_QUEUE: &str = "AWS::SQS::Queue";

/// The provisioned-cluster type of Amazon Redshift: the Redshift model carries
/// `awsService.cloudFormationName: Redshift`.
///
/// "Ref returns the name of the cluster" — a cluster's `ClusterIdentifier` — so this
/// resource's identity is CloudFormation's to compute, under that property
/// ([`declared_name_property`]) and Amazon Redshift's own rules for it
/// ([`naming_rules`]).
pub const REDSHIFT_CLUSTER: &str = "AWS::Redshift::Cluster";

/// The cluster SUBNET GROUP of Amazon Redshift: the subnets of a VPC a cluster is
/// launched into, which a cluster's `ClusterSubnetGroupName` names.
///
/// "Ref returns the name of the cluster subnet group" and the type declares no name
/// property of its own — its three properties are `Description`, `SubnetIds` and `Tags` —
/// so the name is one CloudFormation GENERATES, held to Amazon Redshift's own rules for
/// it ([`naming_rules`]), and that generated name is the physical resource id a dependent
/// cluster resolves the group by.
pub const REDSHIFT_CLUSTER_SUBNET_GROUP: &str = "AWS::Redshift::ClusterSubnetGroup";

/// The two resource types that carry a QUEUE's access policy: the one that applies a
/// document to a list of queues, and the one that belongs to a single queue.
///
/// Neither is a resource of Amazon SQS's own — a queue's policy is its `Policy` ATTRIBUTE —
/// so neither carries a [`return_values`] arm: the default applies and the physical
/// resource id is the name CloudFormation generated for the resource, which is what "Ref
/// returns the policy ID" for these types means here.
pub const SQS_QUEUE_POLICY: &str = "AWS::SQS::QueuePolicy";
pub const SQS_QUEUE_INLINE_POLICY: &str = "AWS::SQS::QueueInlinePolicy";

/// The `AWS::<cloudFormationName>::<Resource>` type of a SageMaker model package group —
/// the model registry's container for the versioned model packages registered into it, and
/// the carrier of the registry's resource policy. The middle segment is the SageMaker
/// model's own `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model reports `SageMaker`).
///
/// This type deliberately carries NO [`return_values`] arm, so the default applies: its
/// physical resource id is the group's NAME. That is what every SageMaker model-registry
/// operation addresses the group by — `ModelPackageGroupName` is modelled `EntityName`,
/// name-only on the policy operations — so the name is the identifier a client can use,
/// and it is what the provisioner passes to `CreateModelPackageGroup`. The group's ARN is
/// not computed here either: `Arn`/`ModelPackageGroupArn` are the ARN SageMaker itself
/// answered, which the provisioner records on the resource, rather than one minted from a
/// pattern. SageMaker's own model says nothing about this type's `Ref`, so the identifier a
/// client can resolve is preferred over a guess at CloudFormation's.
pub const SAGEMAKER_MODEL_PACKAGE_GROUP: &str = "AWS::SageMaker::ModelPackageGroup";

/// An Amazon CloudWatch metric stream's resource type (the CloudWatch model:
/// `cloudFormationName: CloudWatch`). This is the type `super::provisioning` creates in
/// CloudWatch itself, through that service's own `PutMetricStream`.
pub const CLOUDWATCH_METRIC_STREAM: &str = "AWS::CloudWatch::MetricStream";

/// An Amazon CloudWatch dashboard's resource type. This is the type `super::provisioning`
/// creates in CloudWatch itself, through that service's own `PutDashboard`.
pub const CLOUDWATCH_DASHBOARD: &str = "AWS::CloudWatch::Dashboard";

/// The same for an Amazon SNS topic: the SNS model carries
/// `awsService.cloudFormationName: SNS`.
pub const SNS_TOPIC: &str = "AWS::SNS::Topic";

/// An Amazon SNS subscription's resource type, under the same namespace: the type a
/// template declares a subscriber of a topic with, beside the topic's own inline
/// `Subscription` list.
///
/// It carries no [`return_values`] arm, so the computed physical id is the generated name
/// — a stand-in that the create replaces with the subscription ARN Amazon SNS mints, which
/// is this type's published return value ("Ref returns the subscription ARN") and the
/// identifier `Unsubscribe`/`SetSubscriptionAttributes` address a subscription by.
pub const SNS_SUBSCRIPTION: &str = "AWS::SNS::Subscription";

/// The two resource types that carry a TOPIC's access policy: the one that applies a
/// document to a list of topics, and the one that belongs to a single topic.
///
/// Neither is a resource of Amazon SNS's own — a topic's policy is its `Policy` ATTRIBUTE —
/// so neither carries a [`return_values`] arm: the default applies and the physical resource
/// id is the name CloudFormation generated for the resource, exactly as it is for the two
/// queue-policy types ([`SQS_QUEUE_POLICY`]).
pub const SNS_TOPIC_POLICY: &str = "AWS::SNS::TopicPolicy";
pub const SNS_TOPIC_INLINE_POLICY: &str = "AWS::SNS::TopicInlinePolicy";

/// An Amazon SES configuration set's resource type (the SES model:
/// `awsService.cloudFormationName: SES`). This is the type `super::provisioning` creates
/// in Amazon SES itself, through that service's own `CreateConfigurationSet`.
///
/// It deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the set's NAME — which is that type's published return value
/// ("Ref returns the configuration set name") and the identifier every operation of the
/// owning service addresses a configuration set by (`ConfigurationSetName`, on
/// `DescribeConfigurationSet` and `DeleteConfigurationSet` alike). The type publishes no
/// `Fn::GetAtt` attribute, so it carries no entry in [`published_attribute_names`] either.
pub const SES_CONFIGURATION_SET: &str = "AWS::SES::ConfigurationSet";

/// An Amazon SES receipt rule set's resource type, under the same namespace. This is the
/// type `super::provisioning` creates in Amazon SES itself, through that service's own
/// `CreateReceiptRuleSet`.
///
/// It carries NO [`return_values`] arm either, so its physical resource id is the set's
/// NAME — that type's published return value ("Ref returns the rule set name") and the
/// identifier every operation of the owning service addresses a receipt rule set by
/// (`RuleSetName`, on `DescribeReceiptRuleSet`, `SetActiveReceiptRuleSet` and
/// `DeleteReceiptRuleSet` alike). The name property it is read from is unconventional, so
/// the type is registered in [`declared_name_property`].
pub const SES_RECEIPT_RULE_SET: &str = "AWS::SES::ReceiptRuleSet";

/// One receipt rule OF such a set, under the same namespace: the type a template declares a
/// rule of a rule set with, created through the owning service's own `CreateReceiptRule`.
///
/// Its published return value is the RULE's name ("Ref returns the receipt rule name"),
/// which a declaration states INSIDE the type's `Rule` property rather than through a name
/// property of its own — so no naming rule here computes it and
/// [`identity_is_service_assigned`] names this type: the identifier in force is the one the
/// create answered with.
pub const SES_RECEIPT_RULE: &str = "AWS::SES::ReceiptRule";

/// The `AWS::ECR::PublicRepository` resource type: an Amazon ECR Public repository.
///
/// Spelled as AWS's own CloudFormation resource reference publishes it, under the `ECR`
/// namespace beside the private registry's `AWS::ECR::Repository`, rather than from the
/// ECR Public model's `awsService.cloudFormationName` (`ECRPUBLIC`): the type is what a
/// template spells.
pub const ECR_PUBLIC_REPOSITORY: &str = "AWS::ECR::PublicRepository";

/// An Amazon ECR repository's resource type (ECR's model: `cloudFormationName: ECR`).
pub const ECR_REPOSITORY: &str = "AWS::ECR::Repository";

/// An AWS CodeBuild report group's resource type (CodeBuild's model:
/// `awsService.cloudFormationName: CodeBuild`). This is the type `super::provisioning`
/// creates in CodeBuild itself, through that service's own `CreateReportGroup`.
///
/// Its physical resource id is the report group's NAME — that type's published return
/// value ("Ref returns the report group name"), which is also the `Name` property a
/// declaration states it under ([`physical_name`]) and the one value
/// `CreateReportGroup` takes to key the group by. [`return_values`] carries its one
/// published attribute, `Arn`: the ARN CodeBuild forms for a report group is that
/// service's own `report-group/<name>` path under the single account id this emulator
/// mints, so the value computed here and the one the create answers with are the same
/// ARN — the position [`SNS_TOPIC`] is in.
pub const CODEBUILD_REPORT_GROUP: &str = "AWS::CodeBuild::ReportGroup";

/// An AWS CodeBuild build project's resource type, under that same namespace. This is the
/// type `super::provisioning` creates in CodeBuild itself, through that service's own
/// `CreateProject`.
///
/// Its physical resource id is the project's NAME — that type's published return value
/// ("Ref returns the project name"), which is also the `Name` property a declaration states
/// it under ([`physical_name`]) and the one value `CreateProject` takes to key the project
/// by, which is what every CodeBuild operation on a project (`StartBuild`,
/// `BatchGetProjects`, `DeleteProject`) addresses it with. [`return_values`] carries its one
/// published attribute, `Arn`: the ARN CodeBuild forms for a project is that service's own
/// `project/<name>` path under the single account id this emulator mints, so the value
/// computed here and the one the create answers with are the same ARN — the position
/// [`CODEBUILD_REPORT_GROUP`] is in.
pub const CODEBUILD_PROJECT: &str = "AWS::CodeBuild::Project";

/// The ARN namespace of the CodeBuild service, from its model's `aws.api#service`
/// `arnNamespace` — the service segment of every ARN it forms.
pub const CODEBUILD_ARN_NAMESPACE: &str = "codebuild";

/// An AWS CodeCommit repository's resource type (that model:
/// `awsService.cloudFormationName: CodeCommit`). This is the type `super::provisioning`
/// creates in CodeCommit itself, through that service's own `CreateRepository`.
///
/// It deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the repository's NAME — which is that type's published return
/// value ("Ref returns the repository name") and the identifier every CodeCommit operation
/// addresses a repository by (`repositoryName`, on `GetRepository`,
/// `PutRepositoryTriggers` and `DeleteRepository` alike). Its published attributes are not
/// computed here either: the ARN and the two clone URLs are the ones CodeCommit itself
/// answered the create with — the clone URLs name the endpoint this emulator serves the
/// repository's git transport on, which no pattern here could mint — and the provisioner
/// records them on the resource.
pub const CODECOMMIT_REPOSITORY: &str = "AWS::CodeCommit::Repository";

/// An AWS CodeArtifact domain's resource type (that model:
/// `awsService.cloudFormationName: CodeArtifact`). This is the type `super::provisioning`
/// creates in CodeArtifact itself, through that service's own `CreateDomain`.
///
/// It carries a [`return_values`] arm because its `Ref` is NOT its name: "Ref returns the
/// Amazon Resource Name (ARN) of the domain", and the name is one of the three values
/// `Fn::GetAtt` reads off it (`Arn`, `Name`, `Owner` — the type's read-only properties).
/// That is what a repository's own declaration names the domain holding it by, since a
/// `Ref` to the domain would hand `CreateRepository` an ARN rather than the domain name
/// that call takes.
pub const CODEARTIFACT_DOMAIN: &str = "AWS::CodeArtifact::Domain";

/// An AWS CodeArtifact repository's resource type, under the same namespace, created
/// through that service's own `CreateRepository` in the domain its declaration names.
///
/// Its `Ref` is its ARN too ("Ref returns the Amazon Resource Name (ARN) of the
/// repository"), and its published attributes are `Arn`, `DomainOwner` and `Name`. The
/// `DomainName` a declaration states is a create-only PROPERTY rather than one of them.
pub const CODEARTIFACT_REPOSITORY: &str = "AWS::CodeArtifact::Repository";

/// The resource types a template declares a CloudFormation HOOK with: the Guard Hook, which
/// evaluates CloudFormation Guard rules against a stack operation's targets, and the Lambda
/// Hook, which hands those targets to a function. Both are among the types this service
/// admits ([`crate::state::resource_types`]).
///
/// Each carries a [`return_values`] arm because its `Ref` is NOT its name: "Ref returns the
/// Hook ARN", and `HookArn` — the ARN of the activated Hook — is what `Fn::GetAtt` reads off
/// it. Those are the two values a declaration publishes about a Hook, and a stack that
/// outputs either hands its caller the identifier the extension APIs address that Hook by.
pub const CLOUDFORMATION_GUARD_HOOK: &str = "AWS::CloudFormation::GuardHook";
/// See [`CLOUDFORMATION_GUARD_HOOK`].
pub const CLOUDFORMATION_LAMBDA_HOOK: &str = "AWS::CloudFormation::LambdaHook";

/// CodeArtifact's ARN namespace, which its own model carries as `awsService.arnNamespace`
/// (`codeartifact`) — the service segment of a domain's and a repository's ARN, and the
/// one that service's own state module builds both ARNs under.
pub(super) const CODEARTIFACT_ARN_NAMESPACE: &str = "codeartifact";

/// The type of an Amazon Cognito identity pool (the cognito-identity model's
/// `awsService.cloudFormationName: Cognito`). This is the type `super::super::provisioning`
/// creates in Amazon Cognito Identity itself.
///
/// It deliberately carries NO [`return_values`] arm: "Ref returns the Amazon Cognito
/// identity pool ID" and that id is the owning service's to mint
/// ([`identity_is_service_assigned`] names the type), so the identifier the naming rules
/// compute is only the stand-in a declaration is resolved against until `CreateIdentityPool`
/// answers — and the `Name` the type publishes through `Fn::GetAtt` is what that create
/// reports for the pool.
pub const COGNITO_IDENTITY_POOL: &str = "AWS::Cognito::IdentityPool";

/// The type of the role attachment that binds an identity pool's IAM roles and role
/// mappings to it — the `SetIdentityPoolRoles` call in template form.
///
/// It carries no [`return_values`] arm either, and its identity is NOT service-assigned: the
/// attachment is not a resource of its own in the owning API (a pool has at most one role
/// configuration, which each call replaces), so CloudFormation's own generated identifier is
/// the physical id and the pool it configures stays a declared PROPERTY.
pub const COGNITO_IDENTITY_POOL_ROLE_ATTACHMENT: &str =
    "AWS::Cognito::IdentityPoolRoleAttachment";

/// The type of an Amazon Cognito user-pool group: the cognito-idp model carries
/// `awsService.cloudFormationName: Cognito`.
///
/// This type deliberately carries NO [`return_values`] arm, so the default applies and its
/// physical resource id is the group's NAME — the identifier every operation of the owning
/// service addresses a group by (`GetGroup`, `UpdateGroup` and `DeleteGroup` each take a
/// `GroupName` beside the `UserPoolId` of the pool that holds it), and the one a template
/// declares through [`declared_name_property`]. Its `UserPoolId` is the other half of that
/// identity and stays a declared PROPERTY, which is what the provisioner's calls carry.
pub const COGNITO_USER_POOL_GROUP: &str = "AWS::Cognito::UserPoolGroup";

/// The three attributes `AWS::Cognito::UserPool` publishes, read off the IDENTITY in force:
/// the pool's ARN, and the issuer a pool's tokens name it by, as a name
/// (`cognito-idp.<region>.amazonaws.com/<id>`) and as the URL of that name.
///
/// The ONE place that formula is written, because the identity it is read off changes: the
/// name CloudFormation computed stands in until `CreateUserPool` has answered, and the id
/// Amazon Cognito minted is the identity from then on — so the provisioner recomputes the
/// three from that id ([`crate::state::provisioning::cognito_user_pool`]) rather than keeping
/// values built on a name that identifies no pool.
pub fn user_pool_attributes(region: &str, identity: &str) -> Vec<(String, String)> {
    let host = format!("{COGNITO_IDP_ARN_NAMESPACE}.{region}.{URL_SUFFIX}");
    vec![
        (
            "Arn".to_string(),
            ::emulator_server::emulator_arn(
                COGNITO_IDP_ARN_NAMESPACE,
                region,
                &format!("userpool/{identity}"),
            ),
        ),
        ("ProviderName".to_string(), format!("{host}/{identity}")),
        (
            "ProviderURL".to_string(),
            format!("https://{host}/{identity}"),
        ),
    ]
}

/// The type of an Amazon Cognito user pool (the cognito-idp model's
/// `awsService.cloudFormationName: Cognito`), whose published return values
/// [`return_values`] carries: a pool is how a federated identity pool, an app client and an
/// OpenID relying party all name the same user pool, and each of them names it through one
/// of those values.
pub const COGNITO_USER_POOL: &str = "AWS::Cognito::UserPool";

/// The type of an APP CLIENT of a user pool, whose published return values
/// [`return_values`] carries: the client id its `Ref` returns is what every sign-in and
/// every `DescribeUserPoolClient` names the client by, and the secret its `ClientSecret`
/// publishes is what a confidential client signs its token requests with.
pub const COGNITO_USER_POOL_CLIENT: &str = "AWS::Cognito::UserPoolClient";

/// The type of a third-party IDENTITY PROVIDER of a user pool. Its identity is the
/// `ProviderName` its declaration states — the name that service addresses a provider by,
/// beside the pool that holds it — which is what [`declared_name_property`] names the
/// property it is stated in.
pub const COGNITO_USER_POOL_IDENTITY_PROVIDER: &str = "AWS::Cognito::UserPoolIdentityProvider";

/// The type of a RESOURCE SERVER of a user pool: the API whose custom scopes an app client of
/// the pool may be permitted. Its identity is the `Identifier` its declaration states — the
/// value every call on one addresses it by beside its pool, and the one a scope is spelled
/// `<identifier>/<scope>` with — which is what [`declared_name_property`] names the property
/// it is stated in.
pub const COGNITO_USER_POOL_RESOURCE_SERVER: &str = "AWS::Cognito::UserPoolResourceServer";

/// Amazon Cognito user pools' ARN namespace and endpoint prefix, which its own model
/// carries as `awsService.arnNamespace` and `endpointPrefix` (`cognito-idp`) — the service
/// segment of a pool's ARN and the host its pool endpoints are served under.
pub(super) const COGNITO_IDP_ARN_NAMESPACE: &str = "cognito-idp";

/// An Amazon ECS cluster's resource type (ECS's model: `cloudFormationName: ECS`). This
/// is the type `super::provisioning` creates in Amazon ECS itself.
pub const ECS_CLUSTER: &str = "AWS::ECS::Cluster";

/// The three AWS Elastic Beanstalk resource types `super::provisioning` creates in
/// Elastic Beanstalk itself (that model: `cloudFormationName: ElasticBeanstalk`).
///
/// Each deliberately carries NO [`return_values`] arm, so the default applies and the
/// physical resource id is the resource's NAME — which is what each type's published
/// return value is ("Ref returns the Elastic Beanstalk application name", "Ref returns
/// the Elastic Beanstalk environment name", "Ref returns the auto-generated version
/// label") and what every Elastic Beanstalk operation addresses the resource by. The
/// environment's one published attribute, `EndpointURL`, is not computed here either: it
/// is the endpoint Elastic Beanstalk itself answered the create with, which the
/// provisioner records on the resource.
///
/// `AWS::ElasticBeanstalk::ConfigurationTemplate` is NOT among them: this emulator's
/// `DeleteConfigurationTemplate` is still a stub, so a stack that created one could never
/// delete it, and the type is left declared-only (which `super::provisioning` documents)
/// rather than provisioned into a resource no stack deletion could remove.
pub const ELASTIC_BEANSTALK_APPLICATION: &str = "AWS::ElasticBeanstalk::Application";
/// See [`ELASTIC_BEANSTALK_APPLICATION`].
pub const ELASTIC_BEANSTALK_APPLICATION_VERSION: &str =
    "AWS::ElasticBeanstalk::ApplicationVersion";
/// See [`ELASTIC_BEANSTALK_APPLICATION`].
pub const ELASTIC_BEANSTALK_ENVIRONMENT: &str = "AWS::ElasticBeanstalk::Environment";

/// An Amazon ECS task definition's resource type.
pub const ECS_TASK_DEFINITION: &str = "AWS::ECS::TaskDefinition";

/// An AWS Batch compute environment's resource type (that model:
/// `cloudFormationName: Batch`). "Ref returns the compute environment ARN"; `Fn::GetAtt`:
/// `ComputeEnvironmentArn`.
pub const BATCH_COMPUTE_ENVIRONMENT: &str = "AWS::Batch::ComputeEnvironment";
/// An AWS Batch job queue's resource type. "Ref returns the job queue ARN";
/// `Fn::GetAtt`: `JobQueueArn`.
pub const BATCH_JOB_QUEUE: &str = "AWS::Batch::JobQueue";
/// An AWS Batch service environment's resource type. "Ref returns the service environment
/// ARN"; `Fn::GetAtt`: `ServiceEnvironmentArn`.
pub const BATCH_SERVICE_ENVIRONMENT: &str = "AWS::Batch::ServiceEnvironment";
/// AWS Batch's ARN namespace, from its model's `awsService.arnNamespace`.
pub(super) const BATCH_ARN_NAMESPACE: &str = "batch";

/// Amazon EKS's ARN namespace, from its model's `awsService.arnNamespace`, which the ARNs
/// of a cluster and of a pod identity association are built under.
pub(super) const EKS_ARN_NAMESPACE: &str = "eks";

/// The three CodeDeploy resource types `super::provisioning` creates in CodeDeploy itself
/// (that model: `cloudFormationName: CodeDeploy`).
///
/// Each deliberately carries NO [`return_values`] arm, so the default applies and the
/// physical resource id is the resource's NAME — which is what each type's published return
/// value is ("Ref returns the application name", "Ref returns the deployment group name",
/// "Ref returns the deployment configuration name") and the identifier every CodeDeploy
/// operation addresses the resource by (`GetApplication` takes an `applicationName`,
/// `GetDeploymentGroup` that name and a `deploymentGroupName`, `GetDeploymentConfig` a
/// `deploymentConfigName`). A deployment group's `ApplicationName` is the other half of its
/// identity and stays a declared PROPERTY, which is what the provisioner's calls carry.
pub const CODEDEPLOY_APPLICATION: &str = "AWS::CodeDeploy::Application";
/// See [`CODEDEPLOY_APPLICATION`].
pub const CODEDEPLOY_DEPLOYMENT_GROUP: &str = "AWS::CodeDeploy::DeploymentGroup";
/// See [`CODEDEPLOY_APPLICATION`].
pub const CODEDEPLOY_DEPLOYMENT_CONFIG: &str = "AWS::CodeDeploy::DeploymentConfig";

/// An Elastic Load Balancing v2 load balancer's resource type (that model's
/// `cloudFormationName: ElasticLoadBalancingV2`).
pub const ELB_LOAD_BALANCER: &str = "AWS::ElasticLoadBalancingV2::LoadBalancer";

/// An Elastic Load Balancing v2 target group's resource type.
pub const ELB_TARGET_GROUP: &str = "AWS::ElasticLoadBalancingV2::TargetGroup";

/// An AWS Fault Injection Service experiment template's resource type (the FIS model:
/// `cloudFormationName: FIS`). This is the type `super::provisioning` creates in FIS
/// itself, so the id a stack's output reports for it is the id FIS minted.
pub const FIS_EXPERIMENT_TEMPLATE: &str = "AWS::FIS::ExperimentTemplate";

/// An AWS WAF Classic regional IPSet's resource type (the AWS WAF Regional model:
/// `cloudFormationName: WAFRegional`). This is a type `super::provisioning` creates in that
/// service itself, and "Ref returns the IPSet ID" — an id only AWS WAF Classic mints — so
/// the id a stack's output reports for it is the one that service answered with.
pub const WAF_REGIONAL_IP_SET: &str = "AWS::WAFRegional::IPSet";

/// The four WAF (WAFv2) resource types a stack creates in that service (the WAFv2 model:
/// `cloudFormationName: WAFv2`).
///
/// Each one's identity is WAF's OWN to mint: its create takes the NAME and the scope the
/// template declares and answers a `Summary` carrying the `Id` it assigned and the `ARN` it
/// built from the scope, that name and that id — "a unique identifier for the … This ID is
/// returned in the responses to create and list commands. You provide it to operations like
/// update and delete" (`WebACLSummary$Id` and its siblings). So the identifiers a stack
/// publishes for one are the ones that service answered with, which is what
/// `GetWebACL`/`GetIPSet`/`GetRuleGroup`/`GetRegexPatternSet` resolve and what
/// `AssociateWebACL` takes, rather than values these naming rules computed.
pub const WAFV2_WEB_ACL: &str = "AWS::WAFv2::WebACL";
pub const WAFV2_RULE_GROUP: &str = "AWS::WAFv2::RuleGroup";
pub const WAFV2_IP_SET: &str = "AWS::WAFv2::IPSet";
pub const WAFV2_REGEX_PATTERN_SET: &str = "AWS::WAFv2::RegexPatternSet";

/// The type that puts one of those web ACLs IN FRONT OF a protected resource (the same
/// model, `cloudFormationName: WAFv2`).
///
/// It is no entity of AWS WAF's own: it is the BINDING `AssociateWebACL` makes and
/// `DisassociateWebACL` removes, and what identifies the binding is the pair it binds — so
/// its physical resource id is that pair rather than a name no call of that service takes
/// ([`composite_identity_properties`]).
pub const WAFV2_WEB_ACL_ASSOCIATION: &str = "AWS::WAFv2::WebACLAssociation";

/// The type of an Amazon EKS access entry: the access one IAM principal has to one cluster
/// (the Amazon EKS model's `awsService.cloudFormationName: EKS`).
///
/// Another binding identified by the pair it binds: every operation on an access entry
/// addresses it by the cluster and the principal, which are that operation's two
/// `@httpLabel`s, so this type's physical resource id is that pair too
/// ([`composite_identity_properties`]).
pub const EKS_ACCESS_ENTRY: &str = "AWS::EKS::AccessEntry";

/// The type of an Amazon EKS cluster: the Kubernetes control plane that service runs (the
/// Amazon EKS model's `awsService.cloudFormationName: EKS`).
///
/// "Ref returns the name of the cluster", which is also the handle every Amazon EKS
/// operation on one takes (`DescribeCluster$name`) and the `Name` property a declaration
/// states, so the physical resource id the naming rules compute for the type is already the
/// identifier in force.
pub const EKS_CLUSTER: &str = "AWS::EKS::Cluster";

/// The type of an Amazon EKS pod identity association: the IAM role one Kubernetes service
/// account of one cluster assumes (same model).
///
/// `CreatePodIdentityAssociation` takes no identifier — a declaration states the cluster,
/// the namespace, the service account and the role — and answers the association's own ARN
/// and id, which is why this type's identity is the owning service's
/// ([`identity_is_service_assigned`]).
pub const EKS_POD_IDENTITY_ASSOCIATION: &str = "AWS::EKS::PodIdentityAssociation";

/// The `AWS::<cloudFormationName>::<Resource>` types of the AWS AppConfig resources a
/// configuration estate declares. The middle segment is that model's own
/// `awsService.cloudFormationName` (`AppConfig`).
///
/// Each one's identity is AppConfig's OWN to mint: every create takes the resource's NAME and
/// no identifier, and answers the `Id` that service assigned — modelled `Id` with pattern
/// `^[a-z0-9]{4,7}$`, which no name CloudFormation decides could satisfy. "Ref returns the
/// application ID" and "Ref returns the deployment strategy ID", and the ids a child's
/// `Fn::GetAtt EnvironmentId`/`ConfigurationProfileId` reports are the ones its create
/// answered — which is also the `ApplicationId`, `EnvironmentId`, `ConfigurationProfileId`
/// and `DeploymentStrategyId` every later AppConfig call takes
/// (`CreateHostedConfigurationVersion`, `StartDeployment`). That is what
/// [`identity_is_service_assigned`] names all four for, and
/// [`crate::state::provisioning::appconfig_resources`] is what creates them.
pub const APPCONFIG_APPLICATION: &str = "AWS::AppConfig::Application";
pub const APPCONFIG_ENVIRONMENT: &str = "AWS::AppConfig::Environment";
pub const APPCONFIG_CONFIGURATION_PROFILE: &str = "AWS::AppConfig::ConfigurationProfile";
pub const APPCONFIG_DEPLOYMENT_STRATEGY: &str = "AWS::AppConfig::DeploymentStrategy";

/// The separator CloudFormation joins the parts of a COMPOSITE physical resource id with.
pub(super) const COMPOSITE_IDENTITY_SEPARATOR: &str = "|";

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
///   operation on an access entry addresses one by.
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
/// service answered instead ([`amplifyuibuilder_identity`]).
pub fn composite_identity_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        WAFV2_WEB_ACL_ASSOCIATION => &["ResourceArn", "WebACLArn"],
        EKS_ACCESS_ENTRY => &["ClusterName", "PrincipalArn"],
        AMPLIFYUIBUILDER_COMPONENT | AMPLIFYUIBUILDER_FORM | AMPLIFYUIBUILDER_THEME => {
            AMPLIFYUIBUILDER_ATTRIBUTES
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

/// Give a resource the physical id its type's composite primary identifier makes it, where
/// its type has one.
///
/// A composite identity is a function of the resource's RESOLVED properties — the other
/// resources the binding names resolve to their own identifiers first — so this is applied
/// where those are filled in ([`resolve_properties`]) rather than where the naming rules run.
/// A type with no composite identifier is left exactly as it is.
pub(super) fn adopt_composite_identity(resource: &mut StoredResource) {
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
///   `stageName`).
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

/// An Amazon EventBridge rule's resource type (the EventBridge model:
/// `cloudFormationName: Events`). This is a type `super::provisioning` creates in
/// EventBridge itself, and "Ref returns the rule name", so the rule a stack's output names
/// is the one `events DescribeRule` answers with.
pub const EVENTS_RULE: &str = "AWS::Events::Rule";

/// An Amazon EventBridge Pipes pipe's resource type (the Pipes model:
/// `cloudFormationName: Pipes`, `arnNamespace: pipes`). This is a type
/// `super::provisioning` creates in EventBridge Pipes itself, and "Ref returns the pipe
/// name" — which is the physical id the naming rules already compute for the type — so the
/// pipe a stack's output names is the one `pipes DescribePipe` answers with, and the `Arn`
/// and `CurrentState` the type publishes are the ones that service reports for it.
pub const PIPES_PIPE: &str = "AWS::Pipes::Pipe";

/// The event bus a rule declaring none is bound to — "if you omit this, the default event
/// bus is used" (`PutRule$EventBusName`) — and the one bus whose name a rule ARN does not
/// carry a segment for.
pub(super) const DEFAULT_EVENT_BUS: &str = "default";

/// The bus NAME an `EventBusName` property names.
///
/// EventBridge's `EventBusNameOrArn` admits either a bare name or an event-bus ARN, so an
/// ARN is reduced to everything after its `event-bus/` resource-type segment — the whole
/// remainder, not the text after the last `/`, because a bus name may itself carry `/`.
pub(super) fn event_bus_name(declared: &str) -> String {
    match declared.split_once(":event-bus/") {
        Some((_, name)) => name.to_string(),
        None => declared.to_string(),
    }
}

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

/// The prefix of an experiment-template id, as FIS mints one (`EXT` + a base-36 suffix;
/// FIS's own `TEMPLATE_ID_PREFIX`).
pub(super) const FIS_TEMPLATE_ID_PREFIX: &str = "EXT";

/// How many characters follow [`FIS_TEMPLATE_ID_PREFIX`] in an experiment-template id:
/// FIS's own `ID_SUFFIX_LEN`, the width of the base-36 suffix every id it mints carries.
pub(super) const FIS_TEMPLATE_ID_DIGITS: usize = 12;

/// The `AWS::CloudFront::Distribution` resource type: one CloudFront distribution.
pub const CLOUDFRONT_DISTRIBUTION: &str = "AWS::CloudFront::Distribution";

/// The `AWS::CloudFront::OriginAccessControl` resource type: the origin access control an
/// origin is read through.
pub const CLOUDFRONT_ORIGIN_ACCESS_CONTROL: &str = "AWS::CloudFront::OriginAccessControl";

/// The `AWS::CloudFront::CloudFrontOriginAccessIdentity` resource type: the legacy origin
/// access identity an Amazon S3 bucket policy names by canonical user id.
pub const CLOUDFRONT_ORIGIN_ACCESS_IDENTITY: &str =
    "AWS::CloudFront::CloudFrontOriginAccessIdentity";

/// CloudFront's three policy resource types, each identified by a UUID CloudFront mints.
pub const CLOUDFRONT_CACHE_POLICY: &str = "AWS::CloudFront::CachePolicy";
pub const CLOUDFRONT_ORIGIN_REQUEST_POLICY: &str = "AWS::CloudFront::OriginRequestPolicy";
pub const CLOUDFRONT_RESPONSE_HEADERS_POLICY: &str = "AWS::CloudFront::ResponseHeadersPolicy";

/// The Amazon API Gateway resource types whose published `Ref` is a SERVICE-ASSIGNED id
/// rather than a name a template states: a REST API, a resource of one, and a deployment
/// of one.
pub const APIGATEWAY_REST_API: &str = "AWS::ApiGateway::RestApi";
pub const APIGATEWAY_RESOURCE: &str = "AWS::ApiGateway::Resource";
pub const APIGATEWAY_DEPLOYMENT: &str = "AWS::ApiGateway::Deployment";

/// The Amazon API Gateway resource types that govern ACCESS to a REST API, whose published
/// `Ref` is a SERVICE-ASSIGNED id in the same position: `CreateApiKey`, `CreateAuthorizer`
/// and `CreateUsagePlan` each take no identifier and answer the `id` that service assigned —
/// "Ref returns the API key ID", "Ref returns the authorizer ID", "Ref returns the usage
/// plan ID" — and a `UsagePlanKey`'s own id is the id of the key its create put in the plan
/// ("Ref returns the usage plan key ID"). A template declares a key's or a plan's NAME and
/// an association's members, never any of those ids, and every call on one takes the minted
/// id (`GetApiKey$apiKey`, `GetAuthorizer$authorizerId`, `GetUsagePlan$usagePlanId`), which
/// is why [`identity_is_service_assigned`] names all four.
pub const APIGATEWAY_API_KEY: &str = "AWS::ApiGateway::ApiKey";
pub const APIGATEWAY_AUTHORIZER: &str = "AWS::ApiGateway::Authorizer";
pub const APIGATEWAY_USAGE_PLAN: &str = "AWS::ApiGateway::UsagePlan";
pub const APIGATEWAY_USAGE_PLAN_KEY: &str = "AWS::ApiGateway::UsagePlanKey";

/// `AWS::ApiGateway::VpcLink`: the VPC link a PRIVATE integration reaches a VPC through,
/// whose published `Ref` is a SERVICE-ASSIGNED id in the same position as the types above.
///
/// `CreateVpcLink` takes no identifier and answers the `id` API Gateway assigned — "Ref
/// returns the VPC link ID", which is also this type's one `Fn::GetAtt` attribute
/// (`VpcLinkId`) — and every call on a link takes that id (`GetVpcLink$vpcLinkId`,
/// `DeleteVpcLink$vpcLinkId`), as does the integration that reaches a VPC through it
/// (`Integration$connectionId`). A template declares the link's NAME, never its id, so
/// [`identity_is_service_assigned`] names this type: the identifier in force is the one that
/// service minted ([`crate::state::provisioning::apigateway_rest_api`]).
pub const APIGATEWAY_VPC_LINK: &str = "AWS::ApiGateway::VpcLink";

/// `AWS::ApiGateway::Account`: the account-level settings of the REST control plane — the
/// CloudWatch Logs role API Gateway writes an account's logs under — which a stack really
/// sets in that service ([`crate::state::provisioning::apigateway_rest_api`]).
///
/// "Ref returns the ID for the account", and `Fn::GetAtt` reads the same value as `Id`. The
/// settings are a SINGLETON of the account and Region, so there is no identifier for a
/// caller to state and none for API Gateway to assign — `UpdateAccount` (`PATCH /account`)
/// takes and answers neither — which is why [`identity_is_service_assigned`] does not name
/// this type: the id CloudFormation minted for the resource is the one in force.
pub const APIGATEWAY_ACCOUNT: &str = "AWS::ApiGateway::Account";

/// `AWS::ApiGateway::DomainName`: a custom domain name of the REST control plane, which a
/// stack creates in API Gateway itself ([`super::provisioner`]).
///
/// "Ref returns the domain name" — the name the template states in its own `DomainName`
/// property, which is this type's identifier ([`declared_name_property`]) — while every
/// attribute the type publishes (the CloudFront distribution an edge-optimized domain is
/// served by, the host a regional one is served on, the ARN API Gateway names the domain
/// by) is a value only that service assigns, so each is read off the create's answer rather
/// than computed by the naming rules ([`return_values`] carries no arm for this type).
pub const APIGATEWAY_DOMAIN_NAME: &str = "AWS::ApiGateway::DomainName";

/// `AWS::ApiGateway::BasePathMapping`: the base path of such a custom domain name that one
/// stage of a REST API answers on, which a stack creates in API Gateway itself
/// ([`super::provisioner`]).
///
/// The mapping is addressed by the domain name that holds it and its own base path — the two
/// `@httpLabel`s of every operation on one — and both are properties the declaration states,
/// so every call the provider makes is built from the declaration rather than from an
/// identifier only API Gateway can assign. It gets no arm here either: with none, `Ref` of
/// the resource is the stand-in [`physical_name`] computes for a type that declares no name
/// property, which nothing reads — no call of this type's provider takes it, and the type
/// publishes no `Fn::GetAtt` attribute at all.
pub const APIGATEWAY_BASE_PATH_MAPPING: &str = "AWS::ApiGateway::BasePathMapping";

/// The Amazon API Gateway v2 resource types a stack creates in that service.
///
/// `AWS::ApiGatewayV2::Api`: "Ref returns the API ID", and `Fn::GetAtt` reads
/// `ApiEndpoint` — the API Gateway v2 service MINTS both (`CreateApi` takes no identifier
/// and answers the `ApiId` it assigned together with the endpoint it serves the api on),
/// which is why [`identity_is_service_assigned`] names this type.
///
/// `AWS::ApiGatewayV2::ApiMapping`: "Ref returns the API mapping ID", which
/// `CreateApiMapping` mints the same way, and which is also this type's one published
/// `Fn::GetAtt` attribute.
///
/// `AWS::ApiGatewayV2::DomainName`: "Ref returns the domain name" — the name the template
/// states, which is this type's own identifier — and `Fn::GetAtt` reads
/// `RegionalDomainName` and `RegionalHostedZoneId`, the two values that service resolves
/// for the regional endpoint it serves the domain on.
pub const APIGATEWAYV2_API: &str = "AWS::ApiGatewayV2::Api";
pub const APIGATEWAYV2_API_MAPPING: &str = "AWS::ApiGatewayV2::ApiMapping";
pub const APIGATEWAYV2_DOMAIN_NAME: &str = "AWS::ApiGatewayV2::DomainName";

/// The Amazon API Gateway v2 resource types whose resources live INSIDE an api — the ones a
/// stack declares to make the api it deployed serve anything at all.
///
/// `AWS::ApiGatewayV2::Integration`: "Ref returns the integration ID",
/// `AWS::ApiGatewayV2::Route`: "Ref returns the route ID",
/// `AWS::ApiGatewayV2::Deployment`: "Ref returns the deployment ID" and
/// `AWS::ApiGatewayV2::Authorizer`: "Ref returns the authorizer ID" — each of those ids is
/// MINTED by API Gateway v2 (its create takes no identifier and answers the one it
/// assigned), which is why [`identity_is_service_assigned`] names all four.
///
/// `AWS::ApiGatewayV2::Stage` is not one of them: "Ref returns the stage name", the name the
/// template states, which is this type's own identifier.
pub const APIGATEWAYV2_INTEGRATION: &str = "AWS::ApiGatewayV2::Integration";
pub const APIGATEWAYV2_ROUTE: &str = "AWS::ApiGatewayV2::Route";
pub const APIGATEWAYV2_STAGE: &str = "AWS::ApiGatewayV2::Stage";
pub const APIGATEWAYV2_DEPLOYMENT: &str = "AWS::ApiGatewayV2::Deployment";
pub const APIGATEWAYV2_AUTHORIZER: &str = "AWS::ApiGatewayV2::Authorizer";

/// How many characters follow the `E` of a CloudFront-assigned identifier.
///
/// CloudFront mints a distribution id, an origin access control id and an origin access
/// identity id in one shape: `E` and 13 more characters — the form
/// `CloudFrontOriginAccessIdentity$Id` documents by example ("for example,
/// E74FTE3AJFJ256A") and the one this emulator's own CloudFront service mints an id in.
pub(super) const CLOUDFRONT_ID_BODY_LEN: usize = 13;

/// How many characters an API-Gateway-assigned identifier carries: ten, the width of the
/// `RestApi`, `Resource` and `Deployment` ids API Gateway mints (and the width this
/// emulator's own API Gateway service mints one in).
pub(super) const APIGATEWAY_ID_LEN: usize = 10;

/// How many lowercase hexadecimal characters an Amazon S3 canonical user id carries: 64,
/// the width S3 writes one in (and the width this emulator's CloudFront service mints the
/// canonical user id of an origin access identity in).
pub(super) const S3_CANONICAL_USER_ID_HEX_CHARS: usize = 64;

/// The salts the identifiers above are derived under — one per identifier, so two ids of
/// ONE resource are never the same digits (see [`derived_digits`]).
pub(super) const CLOUDFRONT_DISTRIBUTION_ID_SALT: &str = "cloudfront/distribution-id";
pub(super) const CLOUDFRONT_DISTRIBUTION_DOMAIN_SALT: &str = "cloudfront/distribution-domain-label";
pub(super) const CLOUDFRONT_ORIGIN_ACCESS_CONTROL_ID_SALT: &str = "cloudfront/origin-access-control-id";
pub(super) const CLOUDFRONT_ORIGIN_ACCESS_IDENTITY_ID_SALT: &str = "cloudfront/origin-access-identity-id";
pub(super) const CLOUDFRONT_S3_CANONICAL_USER_ID_SALT: &str = "cloudfront/s3-canonical-user-id";
pub(super) const APIGATEWAY_REST_API_ID_SALT: &str = "apigateway/rest-api-id";
pub(super) const APIGATEWAY_ROOT_RESOURCE_ID_SALT: &str = "apigateway/root-resource-id";
pub(super) const APIGATEWAY_RESOURCE_ID_SALT: &str = "apigateway/resource-id";
pub(super) const APIGATEWAY_DEPLOYMENT_ID_SALT: &str = "apigateway/deployment-id";
pub(super) const APIGATEWAY_API_KEY_ID_SALT: &str = "apigateway/api-key-id";
pub(super) const APIGATEWAY_AUTHORIZER_ID_SALT: &str = "apigateway/authorizer-id";
pub(super) const APIGATEWAY_USAGE_PLAN_ID_SALT: &str = "apigateway/usage-plan-id";
pub(super) const APIGATEWAY_USAGE_PLAN_KEY_ID_SALT: &str = "apigateway/usage-plan-key-id";
pub(super) const APIGATEWAY_VPC_LINK_ID_SALT: &str = "apigateway/vpc-link-id";
pub(super) const APIGATEWAY_ACCOUNT_ID_SALT: &str = "apigateway/account-id";
pub(super) const APIGATEWAYV2_API_ID_SALT: &str = "apigatewayv2/api-id";
pub(super) const APIGATEWAYV2_API_MAPPING_ID_SALT: &str = "apigatewayv2/api-mapping-id";
pub(super) const APIGATEWAYV2_DOMAIN_LABEL_SALT: &str = "apigatewayv2/regional-domain-label";

/// An identifier of the shape CloudFront ASSIGNS one in: `E` followed by
/// [`CLOUDFRONT_ID_BODY_LEN`] uppercase characters.
///
/// The characters are uppercase hexadecimal, a subset of the base-36 alphabet CloudFront
/// mints from — a stand-in of the right shape, stable across reads for the reason
/// [`derived_digits`] gives.
pub(super) fn cloudfront_assigned_id(salt: &str, name: &str) -> String {
    format!(
        "E{}",
        derived_digits(salt, name, CLOUDFRONT_ID_BODY_LEN, Alphabet::Upper)
    )
}

/// The domain name CloudFront serves a distribution on: `d<label>.cloudfront.net`.
///
/// `Distribution$DomainName` is "The domain name that corresponds to the distribution …
/// such as d111111abcdef8.cloudfront.net" — a generated label under CloudFront's own
/// `cloudfront.net` zone, in the lowercase a DNS label takes. It is a DIFFERENT value from
/// the distribution's id, so it is derived under its own salt.
pub(super) fn cloudfront_distribution_domain_name(name: &str) -> String {
    format!(
        "d{}.{CLOUDFRONT_DOMAIN_SUFFIX}",
        derived_digits(
            CLOUDFRONT_DISTRIBUTION_DOMAIN_SALT,
            name,
            CLOUDFRONT_ID_BODY_LEN,
            Alphabet::Lower,
        )
    )
}

/// The DNS zone CloudFront serves a distribution's own domain name under — not
/// [`URL_SUFFIX`], which is where the AWS service endpoints live.
pub(super) const CLOUDFRONT_DOMAIN_SUFFIX: &str = "cloudfront.net";

/// An identifier of the shape API Gateway ASSIGNS one in: [`APIGATEWAY_ID_LEN`] lowercase
/// characters, drawn from the same derivation as every other identifier here.
pub(super) fn api_gateway_assigned_id(salt: &str, name: &str) -> String {
    derived_digits(salt, name, APIGATEWAY_ID_LEN, Alphabet::Lower)
}

/// The URI an API Gateway v2 api is invoked on: "The URI of the API, of the form
/// `{api-id}.execute-api.{region}.amazonaws.com`" (`Api$ApiEndpoint`), under the scheme
/// the api's protocol speaks — a WEBSOCKET api is reached over `wss`, an HTTP api over
/// `https`.
///
/// The protocol comes from the resource's own declared `ProtocolType`, evaluated through
/// the template's intrinsics like any other property, and an api whose declaration names
/// none is an HTTP one, that being the only other value of the modelled enum.
pub(super) fn apigatewayv2_api_endpoint(
    api_id: &str,
    region: &str,
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> String {
    let websocket = properties
        .get("ProtocolType")
        .and_then(|value| text(value, scope, 0, budget))
        .is_some_and(|protocol| protocol.eq_ignore_ascii_case(APIGATEWAYV2_WEBSOCKET_PROTOCOL));
    let scheme = if websocket { "wss" } else { "https" };
    format!("{scheme}://{api_id}.execute-api.{region}.{URL_SUFFIX}")
}

/// The `ProtocolType` whose apis are reached over the WebSocket scheme, out of the
/// `HTTP | WEBSOCKET` enum API Gateway v2's model declares.
pub(super) const APIGATEWAYV2_WEBSOCKET_PROTOCOL: &str = "WEBSOCKET";

/// The ARN namespace API Gateway v2's resources live in, which is that model's own
/// `awsService.arnNamespace` (`apigateway`) rather than the `ApiGatewayV2` its resource
/// types are spelled with.
pub(super) const APIGATEWAYV2_ARN_NAMESPACE: &str = "apigateway";

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
        resource.properties =
            resolved_value(&declared.properties, scope, 0, budget).unwrap_or(Value::Null);
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
/// does not publish: "Template error: resource <logical id> does not support attribute type
/// <attribute> in Fn::GetAtt". Fixed text around the two names the error is about.
pub fn unsupported_attribute_reason(logical_id: &str, attribute: &str) -> String {
    format!(
        "Template error: resource {logical_id} does not support attribute type {attribute} in \
         Fn::GetAtt"
    )
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

/// The FIRST `Fn::GetAtt` of `value` naming an attribute the resource it names does not
/// publish, as `(logical id, attribute)`.
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
                    let published = scope
                        .resources
                        .get(&logical_id)
                        .map(|resource| publishes_attribute(&resource.attributes, &attribute));
                    if published == Some(false) {
                        return Some((logical_id, attribute));
                    }
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
pub(super) const ECS_SERVICE: &str = "AWS::ECS::Service";

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

/// The salts a listener's and a rule's assigned ids are derived under, beside the load
/// balancer's and the target group's (see [`service_assigned_label`]).
pub(super) const ELB_LISTENER_ID_SALT: &str = "elasticloadbalancing/listener-id";
pub(super) const ELB_LISTENER_RULE_ID_SALT: &str = "elasticloadbalancing/listener-rule-id";

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


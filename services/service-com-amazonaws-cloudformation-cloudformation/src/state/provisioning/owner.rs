// SPDX-License-Identifier: CC0-1.0

//! The one place each provisioning question is PUT to whichever module owns a resource type
//! ([`super::Owner`]).
//!
//! Every method here is the same question asked of one owner — create it, is it ready, apply
//! a changed declaration, re-tag it, delete it, what does its service report about it — so the
//! deployment algorithm beside it ([`super::reconcile`]) is unchanged by where an arm lives.
//! It is a file of its own so that adding a type's arms does not grow the algorithm's file
//! past the length a change to this repository may touch.

use super::*;

impl Owner {
    /// Create one resource in the service that owns it — see
    /// [`Provisioner::create`](super::provisioner::Provisioner::create).
    pub(super) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        propagated: &[(String, String)],
        acknowledged: &[String],
    ) -> Result<Created, Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner.create(ctx, region, resource, propagated).await
            }
            Self::DynamodbTable => {
                dynamodb_table::create(ctx, region, resource, propagated).await
            }
            // An inline policy carries no tags of its own: it is not a resource, so
            // CloudFormation propagates none to it.
            Self::IamPolicy(kind) => iam_policy::create(ctx, region, kind, resource)
                .await
                .map(|()| Created::default()),
            // Neither an alias nor a published version takes tags: Lambda's tagging calls
            // address FUNCTIONS, so there is no call that could carry a tag to one.
            Self::LambdaAlias => lambda_alias::create(ctx, region, resource).await,
            Self::LambdaVersion => lambda_version::create(ctx, region, resource).await,
            Self::LambdaEventSourceMapping => {
                lambda_event_source_mapping::create(ctx, region, resource, propagated).await
            }
            Self::LambdaFunction => {
                lambda_function::create(ctx, region, resource, propagated).await
            }
            // A layer version takes no tags: Lambda's tagging calls address FUNCTIONS, so
            // there is no call that could carry a tag to one.
            Self::LambdaLayerVersion => {
                lambda_layer_version::create(ctx, region, resource).await
            }
            // A policy statement carries no tags of its own: it is not a resource, so
            // CloudFormation propagates none to it.
            Self::LambdaPermission => lambda_permission::create(ctx, region, resource).await,
            Self::LogsResource(kind) => {
                logs_resources::create(ctx, region, kind, resource, propagated).await
            }
            Self::IotAuthorizer => {
                iot_authorizer::create(ctx, region, resource, propagated).await
            }
            Self::Ec2VpcNetwork(kind) => {
                ec2_vpc_network::create(ctx, region, kind, resource, propagated).await
            }
            Self::Ec2Instance => {
                ec2_instance::create(ctx, region, resource, propagated).await
            }
            // A task-definition revision IS taggable: its `Tags` property is the tag list
            // `RegisterTaskDefinition` takes, so the revision carries the tags CloudFormation
            // gives the resource beside its own.
            Self::EcsTaskDefinition => {
                ecs_task_definition::create(ctx, region, resource, propagated).await
            }
            Self::CognitoIdentityPool(kind) => {
                cognito_identity_pool::create(ctx, region, kind, resource, propagated).await
            }
            // A POOL is taggable: its `UserPoolTags` property is the tag map `CreateUserPool`
            // takes, so it carries the tags CloudFormation gives the resource beside its own.
            // Neither an app client nor an identity provider is: every tagging call of that
            // service addresses a pool, so there is no call that could carry a tag to one.
            Self::CognitoUserPool(kind) => {
                cognito_user_pool::create(ctx, region, kind, resource, propagated).await
            }
            // A rotation schedule carries no tags of its own: it is not a resource of the
            // owning service, but the rotation CONFIGURATION of the secret it names, so
            // CloudFormation propagates none to it.
            Self::SecretsManagerRotationSchedule => {
                secretsmanager_rotation_schedule::create(ctx, region, resource).await
            }
            // A secret IS taggable: its `Tags` property is the tag list `CreateSecret` takes,
            // so it carries the tags CloudFormation gives the resource beside its own.
            Self::SecretsManagerSecret => {
                secretsmanager_secret::create(ctx, region, resource, propagated).await
            }
            // A nested stack carries the stack-level tags CloudFormation propagates to the
            // resources it creates and is created under the acknowledgement the parent's own
            // operation gave, which `CreateStack` takes as its `Tags` and `Capabilities`.
            //
            // BOXED, unlike every other arm: this call reaches CloudFormation's OWN
            // `CreateStack`, so the whole deploy-a-stack chain — this walk included — runs
            // again inside it. Boxing keeps the inner chain's state off this future, so the
            // stack a request needs does not multiply with the nesting the template declares.
            //
            // HOW DEEP that re-entry can go is bounded before the call is made, not here:
            // every level carries the chain of template references above it, and
            // `nested_stack::beyond_the_bounds` refuses a child past
            // `nested_stack::MAX_NESTING_DEPTH` — and one whose own reference the chain is
            // already deploying, which is what a template that nests itself is — as this
            // resource's failure. So the number of re-entries is fixed by that ceiling rather
            // than by the template graph, and a cyclic reference fails the resource instead of
            // recursing. The same bound is applied by the update and delete arms below, which
            // re-enter the same way.
            Self::NestedStack => {
                Box::pin(nested_stack::create(
                    ctx,
                    region,
                    resource,
                    propagated,
                    acknowledged,
                ))
                .await
            }
            Self::StepFunctionsStateMachine => {
                step_functions_state_machine::create(ctx, region, resource, propagated).await
            }
            Self::ApiGatewayRestApi(kind) => {
                apigateway_rest_api::create(ctx, region, kind, resource, propagated).await
            }
            // Neither of these types publishes a `Tags` property, so CloudFormation
            // propagates no tag to one: there is no call that could carry one.
            Self::ApplicationAutoScaling(kind) => {
                application_auto_scaling::create(ctx, region, kind, resource).await
            }
            // Every one of these types IS taggable: its `Tags` property is the tag map each
            // AppConfig create takes, so the resource carries the tags CloudFormation gives it
            // beside its own.
            Self::AppConfigResource(kind) => {
                appconfig_resources::create(ctx, region, kind, resource, propagated).await
            }
            // Every one of these types publishes a `Tags` property and every one of the four
            // creates takes that list as its own member, so a stack's load balancer, target
            // group, listener and rule carry the tags CloudFormation gives them.
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::create(ctx, region, kind, resource, propagated).await
            }
        }
    }

    /// Whether the owning service reports the created resource ready — see
    /// [`Provisioner::readiness`](super::provisioner::Provisioner::readiness).
    ///
    /// `declared` is the properties the resource was declared with, where the records say
    /// which they are, so an arm that waits for a declared SETTING of a resource asks about
    /// the ones the declaration carries rather than about every setting of the type
    /// ([`dynamodb_table::readiness`]). `None` means the declaration is not known here, and
    /// an arm that reads it then holds to whatever that service reports about the resource.
    pub(super) async fn readiness(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
        declared: Option<&Value>,
    ) -> Result<Readiness, Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner.readiness(ctx, region, physical_resource_id).await
            }
            Self::DynamodbTable => {
                Ok(dynamodb_table::readiness(ctx, region, physical_resource_id, declared).await)
            }
            // The document is embedded in every entity it names the moment the put answers.
            Self::IamPolicy(_) => Ok(Readiness::Ready),
            // The alias is in force, and the version exists and can be invoked, the moment
            // the call that made it answers: both are snapshots of a function AWS Lambda
            // already reports settled.
            Self::LambdaAlias | Self::LambdaVersion => Ok(Readiness::Ready),
            Self::LambdaEventSourceMapping => Ok(lambda_event_source_mapping::readiness()),
            Self::LambdaFunction => Ok(lambda_function::readiness()),
            // The version exists, and a function may name it, the moment
            // `PublishLayerVersion` answers.
            Self::LambdaLayerVersion => Ok(Readiness::Ready),
            // The statement is in force the moment `AddPermission` answers.
            Self::LambdaPermission => Ok(Readiness::Ready),
            // A log group, a stream and a metric filter are each in force the moment the
            // call that made them answers, and an authorizer exists in the status its
            // declaration asked for.
            Self::LogsResource(_) => Ok(logs_resources::ready()),
            Self::IotAuthorizer => Ok(iot_authorizer::ready()),
            // A task-definition revision is `ACTIVE` the moment `RegisterTaskDefinition`
            // answers.
            Self::EcsTaskDefinition => Ok(ecs_task_definition::ready()),
            // Each of these resources EXISTS from the moment its create answers: the table
            // takes its routes, the association holds, the gateway is there to be attached,
            // the attachment is in force, the address is allocated, and the endpoint's own
            // route is in the tables the create named. A NAT GATEWAY is the one that comes up
            // in a state nothing routes through, which that arm waits out
            // ([`ec2_vpc_network::readiness`]).
            Self::Ec2VpcNetwork(kind) => {
                ec2_vpc_network::readiness(ctx, region, kind, physical_resource_id).await
            }
            // `RunInstances` answers the instance `pending`, in which nothing runs on it, so
            // this arm waits for Amazon EC2 to report it `running`
            // ([`ec2_instance::readiness`]).
            Self::Ec2Instance => {
                ec2_instance::readiness(ctx, region, physical_resource_id).await
            }
            // A pool exists, and its role configuration is in force, from the moment the
            // owning service's own call answers.
            Self::CognitoIdentityPool(_) => Ok(cognito_identity_pool::ready()),
            // A user pool, an app client and an identity provider each exist from the moment
            // their own create answers.
            Self::CognitoUserPool(_) => Ok(cognito_user_pool::ready()),
            // `CreateStack` answers as soon as the stack's creation has STARTED, so the
            // parent's resource is held open until CloudFormation reports the child settled.
            Self::NestedStack => {
                Box::pin(nested_stack::readiness(ctx, region, physical_resource_id)).await
            }
            // `RotateSecret` answers while the rotation it started is still in flight, so
            // the resource is held open until the owning service reports the secret settled
            // ([`secretsmanager_rotation_schedule::readiness`]).
            Self::SecretsManagerRotationSchedule => {
                secretsmanager_rotation_schedule::readiness(ctx, region, physical_resource_id)
                    .await
            }
            Self::SecretsManagerSecret => Ok(secretsmanager_secret::readiness()),
            Self::StepFunctionsStateMachine => Ok(step_functions_state_machine::readiness()),
            // Each of these resources EXISTS from the moment its create answers: API Gateway
            // reports no state an api, a resource, a method, a deployment or a stage is still
            // coming up in — a stage serves the deployment it points at as soon as the create
            // that made it has answered. A VPC LINK is the one that comes up in a state no
            // integration carries traffic through, which that arm waits out
            // ([`apigateway_rest_api::readiness`]).
            Self::ApiGatewayRestApi(kind) => {
                apigateway_rest_api::readiness(ctx, region, kind, physical_resource_id).await
            }
            // A target is registered, and a policy is in force, from the moment the owning
            // service's own call answers.
            Self::ApplicationAutoScaling(_) => Ok(application_auto_scaling::ready()),
            // An application, an environment, a configuration profile and a deployment
            // strategy each exist from the moment their own create answers.
            Self::AppConfigResource(_) => Ok(appconfig_resources::ready()),
            // `CreateLoadBalancer` answers the load balancer `provisioning`, in which it
            // serves no connection, so that arm waits for Elastic Load Balancing to report it
            // `active` ([`elastic_load_balancing::readiness`]); a target group, a listener and
            // a rule each exist the moment their own create answers.
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::readiness(ctx, region, kind, physical_resource_id).await
            }
        }
    }

    /// The attributes the owning service publishes for a resource it now reports up — see
    /// [`Provisioner::ready_attributes`](super::provisioner::Provisioner::ready_attributes).
    pub(super) async fn ready_attributes(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource_type: &str,
        physical_resource_id: &str,
    ) -> Result<BTreeMap<String, String>, Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner
                    .ready_attributes(ctx, region, resource_type, physical_resource_id)
                    .await
            }
            // A function's `Arn` is the one its create answered with, so a resource this
            // owner created has every attribute it publishes already, and neither a layer
            // version nor a policy statement publishes one at all.
            // A mapping publishes the id its own create answered, which the resource already
            // carries as its physical id, so there is nothing left to read for it either; and
            // an inline policy publishes no attribute at all.
            // An alias's and a version's identifiers are the ones their own create
            // answered with, so there is nothing left to read for either.
            Self::IamPolicy(_)
            | Self::LambdaAlias
            | Self::LambdaEventSourceMapping
            | Self::LambdaFunction
            | Self::LambdaLayerVersion
            | Self::LambdaPermission
            | Self::LambdaVersion => Ok(BTreeMap::new()),
            // A table's `Arn` and `StreamArn` are the ones its own `CreateTable` answered
            // with, so there is nothing left to read once the resource is up.
            Self::DynamodbTable => Ok(BTreeMap::new()),
            // A log group's `Arn` is the one CloudFormation's own naming rules compute for
            // it, and neither a stream nor a metric filter publishes an attribute at all; an
            // authorizer's `Arn` is the one its create answered with. So there is nothing
            // left to read once any of them is up.
            Self::LogsResource(_) | Self::IotAuthorizer => Ok(BTreeMap::new()),
            // A task-definition revision's `TaskDefinitionArn` is the one its own
            // registration answered with, so there is nothing left to read once it is up.
            Self::EcsTaskDefinition => Ok(BTreeMap::new()),
            // Each of these types publishes its own identifier (an Elastic IP address its
            // address and allocation id, an attachment nothing at all), and the create
            // answered it, so there is nothing to read once the resource is up.
            Self::Ec2VpcNetwork(_) => Ok(BTreeMap::new()),
            // An INSTANCE is the one Amazon EC2 type here with more to read once it is up:
            // its addresses and DNS names are not assigned until it is running
            // ([`ec2_instance::ready_attributes`]).
            Self::Ec2Instance => {
                ec2_instance::ready_attributes(ctx, region, physical_resource_id).await
            }
            // A pool's `Name` is the one its own create answered with, and a role attachment
            // publishes no attribute this emulator computes, so there is nothing left to
            // read once either resource is up.
            Self::CognitoIdentityPool(_) => Ok(BTreeMap::new()),
            // A pool's ARN and issuer are read off the id its own create answered, a client's
            // name and secret are what that create answered, and an identity provider
            // publishes no attribute at all — so there is nothing left to read once any of
            // these resources is up.
            Self::CognitoUserPool(_) => Ok(BTreeMap::new()),
            // A nested stack's own id is what its create answered with, and the attributes it
            // publishes are its CHILD's outputs — "Fn::GetAtt … Outputs.<NestedStackOutputName>"
            // — which the child publishes only once it has finished coming up, so they are
            // read now ([`nested_stack::ready_attributes`]).
            Self::NestedStack => {
                Box::pin(nested_stack::ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            // A secret's ARN and name are the ones its own create answered with, so there is
            // nothing left to read once the resource is up; a state machine's `Arn` and `Name`
            // are the same case.
            // A rotation schedule's `Id` is the ARN of the secret it was configured on, which
            // its own create answered, so there is nothing left to read once it is up.
            Self::SecretsManagerRotationSchedule => Ok(BTreeMap::new()),
            Self::SecretsManagerSecret | Self::StepFunctionsStateMachine => Ok(BTreeMap::new()),
            // An api's id and its root resource, and the id of a resource or a deployment, are
            // the ones their own creates answered with, so there is nothing left to read once
            // the resource is up.
            Self::ApiGatewayRestApi(_) => Ok(BTreeMap::new()),
            // A target publishes no attribute at all, and a policy's `Arn` is the one its own
            // put answered with, so there is nothing left to read once either is up.
            Self::ApplicationAutoScaling(_) => Ok(BTreeMap::new()),
            // Each of these resources publishes the identifier its own create answered — which
            // the resource already carries as its physical id — and a profile's `KmsKeyArn`
            // came back with that same create, so there is nothing left to read once the
            // resource is up.
            Self::AppConfigResource(_) => Ok(BTreeMap::new()),
            // A load balancer is the one type here with more to read once it is up: the host
            // it publishes, its hosted zone and the groups in front of it are what Elastic
            // Load Balancing fills in as it provisions the load balancer
            // ([`elastic_load_balancing::ready_attributes`]). A target group, a listener and a
            // rule publish the ARN their own create answered, so there is nothing left to read
            // for any of them.
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::ready_attributes(
                    ctx,
                    region,
                    kind,
                    physical_resource_id,
                )
                .await
            }
        }
    }

    /// Whether the owning service has finished removing a deleted resource — see
    /// [`Provisioner::removal`](super::provisioner::Provisioner::removal).
    pub(super) async fn removal(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
    ) -> Result<Removal, Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner.removal(ctx, region, physical_resource_id).await
            }
            Self::DynamodbTable => Ok(dynamodb_table::removal()),
            // `Delete<entity>Policy` takes the embedded document away, so there is nothing
            // to wait for.
            Self::IamPolicy(_) => Ok(Removal::Gone),
            // `DeleteAlias` takes the alias away, so there is nothing to wait for.
            Self::LambdaAlias => Ok(Removal::Gone),
            Self::LambdaVersion => {
                lambda_version::removal(ctx, region, physical_resource_id).await
            }
            Self::LambdaEventSourceMapping => {
                lambda_event_source_mapping::removal(ctx, region, physical_resource_id).await
            }
            Self::LambdaFunction => {
                lambda_function::removal(ctx, region, physical_resource_id).await
            }
            // `DeleteLayerVersion` takes the version away, so there is nothing to wait for.
            Self::LambdaLayerVersion => Ok(Removal::Gone),
            // `RemovePermission` takes the statement away, so there is nothing to wait for.
            Self::LambdaPermission => Ok(Removal::Gone),
            // Each of these deletes takes the resource away as it answers: neither
            // CloudWatch Logs nor AWS IoT reports a state a removed one is still being
            // deleted in.
            Self::LogsResource(_) => Ok(logs_resources::removal()),
            Self::IotAuthorizer => Ok(iot_authorizer::removal()),
            // `DeregisterTaskDefinition` marks the revision `INACTIVE` as it answers, so
            // there is nothing to wait for.
            Self::EcsTaskDefinition => Ok(ecs_task_definition::removal()),
            // Each of these deletes takes the resource away as it answers — Amazon EC2
            // reports no state a deleted route table, association, endpoint, gateway,
            // attachment, address or route is still being removed in — except the NAT
            // gateway, whose `deleting` state that arm waits out
            // ([`ec2_vpc_network::removal`]).
            Self::Ec2VpcNetwork(kind) => {
                ec2_vpc_network::removal(ctx, region, kind, physical_resource_id).await
            }
            // `TerminateInstances` moves the instance into `shutting-down`, so this arm waits
            // for Amazon EC2 to report it `terminated` ([`ec2_instance::removal`]).
            Self::Ec2Instance => {
                ec2_instance::removal(ctx, region, physical_resource_id).await
            }
            // `DeleteIdentityPool` takes the pool away as it answers, and the call that
            // clears a pool's roles leaves nothing behind either.
            Self::CognitoIdentityPool(_) => Ok(Removal::Gone),
            // `DeleteUserPool`, `DeleteUserPoolClient` and `DeleteIdentityProvider` each take
            // the resource away as they answer.
            Self::CognitoUserPool(_) => Ok(Removal::Gone),
            // `DeleteStack` starts the child's deletion, so the parent's resource is held
            // open until CloudFormation no longer holds the stack.
            Self::NestedStack => {
                Box::pin(nested_stack::removal(ctx, region, physical_resource_id)).await
            }
            Self::SecretsManagerRotationSchedule => {
                Ok(secretsmanager_rotation_schedule::removal())
            }
            Self::SecretsManagerSecret => {
                secretsmanager_secret::removal(ctx, region, physical_resource_id).await
            }
            Self::StepFunctionsStateMachine => {
                step_functions_state_machine::removal(ctx, region, physical_resource_id).await
            }
            // Every API Gateway delete here takes the resource away as it answers: that
            // service reports no state a deleted api, resource, method, deployment, stage or
            // VPC link is still being removed in.
            Self::ApiGatewayRestApi(_) => Ok(Removal::Gone),
            // Deregistering a target and deleting a policy each take the resource away as the
            // call answers: Application Auto Scaling reports no state a deleted one is still
            // being removed in.
            Self::ApplicationAutoScaling(_) => Ok(Removal::Gone),
            // Each AppConfig delete takes the resource away as it answers: that service reports
            // no state a deleted application, environment, profile or strategy is still being
            // removed in.
            Self::AppConfigResource(_) => Ok(appconfig_resources::removal()),
            // Each Elastic Load Balancing delete takes the resource away as it answers: that
            // service reports no state a deleted load balancer, target group, listener or rule
            // is still being removed in.
            Self::ElasticLoadBalancing(_) => Ok(elastic_load_balancing::removal()),
        }
    }

    /// Apply a changed declaration to a resource that exists — see
    /// [`Provisioner::update`](super::provisioner::Provisioner::update).
    pub(super) async fn update(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
        propagated: &[(String, String)],
        acknowledged: &[String],
    ) -> Result<Option<Created>, Refusal> {
        match self {
            // The bucket's own provider, whose update half lives beside this algorithm
            // ([`s3_bucket`]): a property Amazon S3 publishes a call to write on a live
            // bucket is applied to the bucket the stack created rather than replacing it.
            Self::Builtin(Provisioner::S3Bucket) => {
                s3_bucket::update(ctx, region, resource, declared).await
            }
            Self::Builtin(provisioner) => {
                provisioner.update(ctx, region, resource, declared).await
            }
            Self::DynamodbTable => {
                dynamodb_table::update(ctx, region, resource, declared).await
            }
            Self::IamPolicy(kind) => iam_policy::update(ctx, region, kind, resource, declared)
                .await
                .map(|()| Some(Created::default())),
            Self::LambdaAlias => lambda_alias::update(ctx, region, resource, declared)
                .await
                .map(Some),
            // A published version cannot be CHANGED: its code and configuration are the
            // snapshot the publish took, and Lambda publishes no call that edits one — so a
            // changed declaration is reported as not applied rather than as a success that
            // dropped the caller's change.
            Self::LambdaVersion => Ok(None),
            Self::LambdaEventSourceMapping => {
                lambda_event_source_mapping::update(ctx, region, resource, declared).await
            }
            Self::LambdaFunction => lambda_function::update(ctx, region, resource, declared)
                .await
                .map(Some),
            // A published layer version cannot be CHANGED: Lambda publishes no call that
            // edits one, so a changed declaration is reported as not applied rather than as
            // a success that dropped the caller's change.
            Self::LambdaLayerVersion => Ok(None),
            // A statement of a policy cannot be CHANGED in place: `AddPermission` adds one
            // and `RemovePermission` removes one, so a changed declaration is applied by
            // replacing the statement with the two calls Lambda publishes
            // ([`lambda_permission::update`]).
            Self::LambdaPermission => lambda_permission::update(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::LogsResource(kind) => {
                logs_resources::update(ctx, region, kind, resource, declared).await
            }
            Self::IotAuthorizer => {
                iot_authorizer::update(ctx, region, resource, declared).await
            }
            // A task-definition revision cannot be CHANGED: Amazon ECS publishes no call that
            // edits one, so a changed declaration registers a new revision rather than
            // reaching this ([`ecs_task_definition::update`]).
            Self::EcsTaskDefinition => Ok(ecs_task_definition::update()),
            // Neither an association nor a route table has a property that can change on the
            // resource that exists (both of an association's are its identity, and a route
            // table's VPC is create-only), every property of a gateway, of an attachment and
            // of an Elastic IP address requires the resource to be replaced, and the members
            // of an endpoint that can change are `ModifyVpcEndpoint`'s, which this emulator
            // does not serve yet — so a changed declaration is reported as not applied
            // rather than as a success that dropped the caller's change.
            Self::Ec2VpcNetwork(_) => Ok(None),
            // An instance's own properties are either part of the launch that made it or
            // settings a `ModifyInstanceAttribute` would change, which this emulator does not
            // serve — so a changed declaration is reported as not applied rather than as a
            // success that dropped the caller's change.
            Self::Ec2Instance => Ok(None),
            Self::CognitoIdentityPool(kind) => {
                cognito_identity_pool::update(ctx, region, kind, resource, declared).await
            }
            Self::CognitoUserPool(kind) => {
                cognito_user_pool::update(ctx, region, kind, resource, declared, propagated)
                    .await
            }
            // A nested stack is CHANGED by CloudFormation's own `UpdateStack`, under the id
            // the create minted, so the child keeps its identity and its resources. Boxed for
            // the reason [`Self::create`]'s arm records.
            Self::NestedStack => {
                Box::pin(nested_stack::update(
                    ctx,
                    region,
                    resource,
                    declared,
                    propagated,
                    acknowledged,
                ))
                .await
            }
            Self::SecretsManagerRotationSchedule => {
                secretsmanager_rotation_schedule::update(ctx, region, resource, declared).await
            }
            Self::SecretsManagerSecret => {
                secretsmanager_secret::update(ctx, region, resource, declared).await
            }
            Self::StepFunctionsStateMachine => {
                step_functions_state_machine::update(ctx, region, resource, declared, propagated)
                    .await
            }
            // A CHANGED declaration of one of these is applied by no arm here yet: API Gateway
            // changes an api, a method and a stage through its own PATCH calls, which this
            // module makes only for the settings a create could not carry — so a changed
            // declaration is reported as NOT applied rather than as a success that dropped the
            // caller's change.
            Self::ApiGatewayRestApi(_) => Ok(None),
            Self::ApplicationAutoScaling(kind) => {
                application_auto_scaling::update(ctx, region, kind, resource, declared).await
            }
            Self::AppConfigResource(kind) => {
                appconfig_resources::update(ctx, region, kind, resource, declared).await
            }
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::update(ctx, region, kind, resource, declared).await
            }
        }
    }

    /// Bring a carried-forward resource's tags back into step — see
    /// [`Provisioner::retag`](super::provisioner::Provisioner::retag).
    pub(super) async fn retag(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
        propagated: &[(String, String)],
    ) -> Result<(), Refusal> {
        match self {
            // A bucket's tags are Amazon S3's own `PutBucketTagging`/`DeleteBucketTagging`,
            // which is where the set a bucket must end up carrying is reconciled
            // ([`s3_bucket::retag`]) — the create's tagging call applies to the bucket it
            // creates and to no bucket that already exists.
            Self::Builtin(Provisioner::S3Bucket) => {
                s3_bucket::retag(ctx, region, resource, declared, propagated).await
            }
            Self::Builtin(provisioner) => {
                provisioner
                    .retag(ctx, region, resource, declared, propagated)
                    .await
            }
            Self::LambdaFunction => {
                lambda_function::retag(ctx, region, resource, declared, propagated).await
            }
            // The tags of a table that already exists are reconciled by no arm here yet —
            // as for every type whose arm states no `retag` ([`Provisioner::retag`]'s own
            // default) — so nothing is asked about them.
            Self::DynamodbTable => Ok(()),
            // Neither a layer version nor a policy statement is taggable, and neither is an
            // inline policy, so nothing is asked about their tags; the tags of a mapping that
            // already exists are reconciled by no arm here yet ([`Provisioner::retag`]'s own
            // default).
            // Neither an alias nor a published version is taggable either: Lambda's tagging
            // calls address functions, so nothing is asked about their tags.
            Self::IamPolicy(_)
            | Self::LambdaAlias
            | Self::LambdaEventSourceMapping
            | Self::LambdaLayerVersion
            | Self::LambdaPermission
            | Self::LambdaVersion => Ok(()),
            // Neither a log stream nor a metric filter is taggable at all, and the tags of a
            // log group and of an authorizer that already exist are reconciled by no arm here
            // yet ([`Provisioner::retag`]'s own default), so nothing is asked about them.
            Self::LogsResource(_) | Self::IotAuthorizer => Ok(()),
            // A task-definition revision's tags are the ones its registration carried, and no
            // deployment changes a revision, so nothing is asked about them.
            Self::EcsTaskDefinition => Ok(()),
            // The tags of an Amazon EC2 resource that already exists are reconciled by no arm
            // here yet — as for every other type of that service ([`Provisioner::retag`]'s
            // own default) — so nothing is asked about them.
            Self::Ec2VpcNetwork(_) | Self::Ec2Instance => Ok(()),
            // A pool's tags are replaced by its own update call, which carries the
            // declaration's whole configuration, so nothing is asked about them here.
            Self::CognitoIdentityPool(_) => Ok(()),
            // A user pool's tags are replaced by its own update call, which carries the
            // declaration's whole tag map, and neither an app client nor an identity provider
            // is taggable — so nothing is asked about any of them here.
            Self::CognitoUserPool(_) => Ok(()),
            // A nested stack's own tags are the ones its `UpdateStack` carries, over the
            // template it already has: the set it must end up with is reconciled here, since
            // a deployment can state the stack's tags differently with no declaration
            // changing at all ([`nested_stack::retag`]). Boxed for the reason
            // [`Self::create`]'s arm records.
            Self::NestedStack => {
                Box::pin(nested_stack::retag(ctx, region, resource, declared, propagated))
                    .await
            }
            // The tags of a secret and of a state machine that already exist are reconciled by
            // no arm here yet ([`Provisioner::retag`]'s own default), so nothing is asked about
            // them.
            // A rotation schedule is not taggable at all, so nothing is asked about its tags.
            Self::SecretsManagerRotationSchedule => Ok(()),
            Self::SecretsManagerSecret | Self::StepFunctionsStateMachine => Ok(()),
            // Nor are the tags of an api or a stage that already exists ([`Provisioner::retag`]'s
            // own default), so nothing is asked about those either.
            Self::ApiGatewayRestApi(_) => Ok(()),
            // Neither of these types is taggable, so nothing is asked about their tags.
            Self::ApplicationAutoScaling(_) => Ok(()),
            // The tags of an AppConfig resource that already exists are reconciled by no arm
            // here yet ([`Provisioner::retag`]'s own default), so nothing is asked about them.
            Self::AppConfigResource(_) => Ok(()),
            // The tags of a load balancer, a target group, a listener or a rule that already
            // exists are reconciled by no arm here yet ([`Provisioner::retag`]'s own default),
            // so nothing is asked about them.
            Self::ElasticLoadBalancing(_) => Ok(()),
        }
    }

    /// Delete one resource from the service that owns it — see
    /// [`Provisioner::delete`](super::provisioner::Provisioner::delete).
    pub(super) async fn delete(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        final_snapshot: Option<&str>,
    ) -> Result<(), Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner
                    .delete(ctx, region, resource, final_snapshot)
                    .await
            }
            // DynamoDB's `DeleteTable` takes no snapshot of a table on the way out: there
            // is no such call (a backup is `CreateBackup`, a call of its own), so a
            // `Snapshot` deletion policy on this type deletes the table with no snapshot
            // invented for it.
            Self::DynamodbTable => dynamodb_table::delete(ctx, region, resource).await,
            // Lambda's `DeleteFunction` takes no snapshot of a function on the way out:
            // there is no such call, so a `Snapshot` deletion policy on this type deletes
            // the function with no snapshot invented for it.
            // IAM takes no snapshot of an embedded policy on the way out: there is no such
            // call, so a `Snapshot` deletion policy on this type deletes it with no snapshot
            // invented for it.
            Self::IamPolicy(kind) => iam_policy::delete(ctx, region, kind, resource).await,
            // Lambda takes no snapshot of a mapping on the way out either: there is no such
            // call, so a `Snapshot` deletion policy on this type deletes the mapping with no
            // snapshot invented for it.
            // Lambda takes no snapshot of an alias or of a published version on the way out
            // either: there is no such call, so a `Snapshot` deletion policy on either type
            // deletes the resource with no snapshot invented for it.
            Self::LambdaAlias => lambda_alias::delete(ctx, region, resource).await,
            Self::LambdaVersion => lambda_version::delete(ctx, region, resource).await,
            Self::LambdaEventSourceMapping => {
                lambda_event_source_mapping::delete(ctx, region, resource).await
            }
            Self::LambdaFunction => lambda_function::delete(ctx, region, resource).await,
            // `DeleteLayerVersion` takes no snapshot of a version on the way out either.
            Self::LambdaLayerVersion => {
                lambda_layer_version::delete(ctx, region, resource).await
            }
            Self::LambdaPermission => lambda_permission::delete(ctx, region, resource).await,
            // Neither CloudWatch Logs nor AWS IoT takes a snapshot of one of these on the way
            // out: there is no such call for a log group, a stream, a metric filter or an
            // authorizer, so a `Snapshot` deletion policy on one of them deletes the resource
            // with no snapshot invented for it.
            Self::LogsResource(kind) => {
                logs_resources::delete(ctx, region, kind, resource).await
            }
            Self::IotAuthorizer => iot_authorizer::delete(ctx, region, resource).await,
            // Amazon ECS takes no snapshot of a task-definition revision on the way out:
            // there is no such call, so a `Snapshot` deletion policy on one deregisters the
            // revision with no snapshot invented for it.
            Self::EcsTaskDefinition => ecs_task_definition::delete(ctx, region, resource).await,
            // Amazon EC2 takes no snapshot of any of these on the way out: there is no such
            // call for a route table, an association, an endpoint, a gateway, an attachment
            // or an address, so a `Snapshot` deletion policy on one of them deletes the
            // resource with no snapshot invented for it.
            Self::Ec2VpcNetwork(kind) => {
                ec2_vpc_network::delete(ctx, region, kind, resource).await
            }
            // Nor of an INSTANCE: `CreateImage` and `CreateSnapshot` are calls of their own
            // that this teardown does not make, so a `Snapshot` deletion policy on one
            // terminates the instance with no image or snapshot invented for it.
            Self::Ec2Instance => ec2_instance::delete(ctx, region, resource).await,
            // Amazon Cognito Identity takes no snapshot of a pool on the way out: there is no
            // such call, so a `Snapshot` deletion policy on either type deletes the resource
            // with no snapshot invented for it.
            Self::CognitoIdentityPool(kind) => {
                cognito_identity_pool::delete(ctx, region, kind, resource).await
            }
            // Amazon Cognito user pools takes no snapshot of a pool, an app client or an
            // identity provider on the way out: there is no such call, so a `Snapshot`
            // deletion policy on one of them deletes the resource with no snapshot invented
            // for it.
            Self::CognitoUserPool(kind) => {
                cognito_user_pool::delete(ctx, region, kind, resource).await
            }
            // CloudFormation takes no snapshot of a stack on the way out: there is no such
            // call, so a `Snapshot` deletion policy on this type deletes the nested stack —
            // and each resource of it under ITS own deletion policies — with no snapshot
            // invented for it. Boxed for the reason [`Self::create`]'s arm records.
            Self::NestedStack => Box::pin(nested_stack::delete(ctx, region, resource)).await,
            // Secrets Manager takes no snapshot of a rotation configuration on the way out:
            // there is no such call, so a `Snapshot` deletion policy on this type turns the
            // rotation off with no snapshot invented for it.
            Self::SecretsManagerRotationSchedule => {
                secretsmanager_rotation_schedule::delete(ctx, region, resource).await
            }
            // Secrets Manager takes no snapshot of a secret on the way out: there is no such
            // call, so a `Snapshot` deletion policy on this type deletes the secret with no
            // snapshot invented for it.
            Self::SecretsManagerSecret => {
                secretsmanager_secret::delete(ctx, region, resource).await
            }
            // Step Functions takes no snapshot of a state machine on the way out: there is no
            // such call, so a `Snapshot` deletion policy on this type deletes the machine with
            // no snapshot invented for it.
            Self::StepFunctionsStateMachine => {
                step_functions_state_machine::delete(ctx, region, resource).await
            }
            // API Gateway takes no snapshot of an api, a resource, a method, a deployment or a
            // stage on the way out: there is no such call, so a `Snapshot` deletion policy on
            // one of them deletes the resource with no snapshot invented for it.
            Self::ApiGatewayRestApi(kind) => {
                apigateway_rest_api::delete(ctx, region, kind, resource).await
            }
            // Application Auto Scaling takes no snapshot of a target or a policy on the way
            // out: there is no such call, so a `Snapshot` deletion policy on one of them
            // deletes the resource with no snapshot invented for it.
            Self::ApplicationAutoScaling(kind) => {
                application_auto_scaling::delete(ctx, region, kind, resource).await
            }
            // AppConfig takes no snapshot of an application, an environment, a profile or a
            // strategy on the way out: there is no such call, so a `Snapshot` deletion policy
            // on one of them deletes the resource with no snapshot invented for it.
            Self::AppConfigResource(kind) => {
                appconfig_resources::delete(ctx, region, kind, resource).await
            }
            // Elastic Load Balancing takes no snapshot of a load balancer, a target group, a
            // listener or a rule on the way out: there is no such call, so a `Snapshot`
            // deletion policy on one of them deletes the resource with no snapshot invented
            // for it.
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::delete(ctx, region, kind, resource).await
            }
        }
    }

    /// The resource's configuration as its owning service reports it, for drift detection —
    /// see
    /// [`Provisioner::actual_configuration`](super::provisioner::Provisioner::actual_configuration).
    pub(crate) async fn actual_configuration(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Option<Value>, Refusal> {
        match self {
            Self::Builtin(provisioner) => {
                provisioner.actual_configuration(ctx, region, resource).await
            }
            // A table reports no configuration of its own to compare yet: this arm reads
            // nothing back for drift detection, so nothing of it is reported as drifted
            // rather than guessed at.
            Self::DynamodbTable => Ok(None),
            // An embedded policy reports no configuration of its own to compare: the
            // document is read by the entity's own drift read, as a policy statement's is.
            Self::IamPolicy(_) => Ok(None),
            Self::LambdaAlias => lambda_alias::actual_configuration(ctx, region, resource).await,
            // A published version is immutable, so nothing of it can have drifted from its
            // declaration.
            Self::LambdaVersion => Ok(None),
            Self::LambdaEventSourceMapping => {
                lambda_event_source_mapping::actual_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                )
                .await
            }
            Self::LambdaFunction => {
                lambda_function::actual_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                )
                .await
            }
            // A published layer version is immutable — every property of the type requires
            // the resource to be replaced, and Lambda publishes no call that edits one — so
            // nothing of it can have drifted from its declaration; and the policy a
            // statement belongs to is read by the function's own drift read, so a statement
            // reports no configuration of its own to compare either.
            Self::LambdaLayerVersion | Self::LambdaPermission => Ok(None),
            Self::LogsResource(kind) => {
                logs_resources::actual_configuration(ctx, region, kind, resource).await
            }
            Self::IotAuthorizer => {
                iot_authorizer::actual_configuration(ctx, region, resource).await
            }
            // A task-definition revision is immutable — every property of the type requires
            // the resource to be replaced, and Amazon ECS publishes no call that edits one —
            // so nothing of it can have drifted from its declaration.
            Self::EcsTaskDefinition => Ok(None),
            // These types report no configuration of their own to compare either: every
            // property each one has is part of its identity or of the call that made it, so
            // there is no member a later read could find changed. An INSTANCE's are the same
            // case here: the launch is what set every one of them, and no call this emulator
            // serves can change one on the instance that exists.
            Self::Ec2VpcNetwork(_) | Self::Ec2Instance => Ok(None),
            Self::CognitoIdentityPool(kind) => {
                cognito_identity_pool::actual_configuration(ctx, region, kind, resource).await
            }
            Self::CognitoUserPool(kind) => {
                cognito_user_pool::actual_configuration(ctx, region, kind, resource).await
            }
            // A nested stack's own configuration is its TEMPLATE, and the drift of a nested
            // stack is detected by running a detection against that stack itself ("perform
            // DetectStackDrift directly on the nested stack"), so this arm reads nothing
            // back for the parent's resource rather than guessing at it.
            Self::NestedStack => Ok(None),
            Self::SecretsManagerRotationSchedule => {
                secretsmanager_rotation_schedule::actual_configuration(ctx, region, resource)
                    .await
            }
            Self::SecretsManagerSecret => {
                secretsmanager_secret::actual_configuration(ctx, region, resource).await
            }
            Self::StepFunctionsStateMachine => {
                step_functions_state_machine::actual_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                )
                .await
            }
            Self::ApiGatewayRestApi(kind) => {
                apigateway_rest_api::actual_configuration(ctx, region, kind, resource).await
            }
            // A target and a policy report no configuration of their own to compare yet: this
            // arm reads nothing back for drift detection, so nothing of either is reported as
            // drifted rather than guessed at.
            Self::ApplicationAutoScaling(_) => Ok(None),
            Self::AppConfigResource(kind) => {
                appconfig_resources::actual_configuration(ctx, region, kind, resource).await
            }
            Self::ElasticLoadBalancing(kind) => {
                elastic_load_balancing::actual_configuration(ctx, region, kind, resource).await
            }
        }
    }

    /// How this type's provider words a refusal it classified — see
    /// [`Provisioner::handler_message`](super::provisioner::Provisioner::handler_message).
    pub(super) fn handler_message(self, class: &str, message: &str) -> String {
        match self {
            Self::Builtin(provisioner) => provisioner.handler_message(class, message),
            // AWS IAM's resource provider reports the service's own message as it came.
            Self::IamPolicy(_) => message.to_string(),
            // AWS Lambda's resource provider reports the service's own message as it came.
            Self::LambdaAlias
            | Self::LambdaEventSourceMapping
            | Self::LambdaFunction
            | Self::LambdaLayerVersion
            | Self::LambdaPermission
            | Self::LambdaVersion => message.to_string(),
            // Amazon DynamoDB's does the same.
            Self::DynamodbTable => message.to_string(),
            // Amazon CloudWatch Logs's and AWS IoT's do the same.
            Self::LogsResource(_) | Self::IotAuthorizer => message.to_string(),
            // Amazon ECS's does the same.
            Self::EcsTaskDefinition => message.to_string(),
            // Amazon EC2's does the same.
            Self::Ec2VpcNetwork(_) | Self::Ec2Instance => message.to_string(),
            // Amazon Cognito Identity's and Amazon Cognito user pools' do the same.
            Self::CognitoIdentityPool(_) | Self::CognitoUserPool(_) => message.to_string(),
            // The nested-stack provider reports what CloudFormation answered as it came: the
            // child's own refusal, which is the account of why the stack was not created.
            Self::NestedStack => message.to_string(),
            // AWS Secrets Manager's and AWS Step Functions' do the same.
            Self::SecretsManagerRotationSchedule
            | Self::SecretsManagerSecret
            | Self::StepFunctionsStateMachine => message.to_string(),
            // Amazon API Gateway's does the same.
            Self::ApiGatewayRestApi(_) => message.to_string(),
            // Application Auto Scaling's does the same.
            Self::ApplicationAutoScaling(_) => message.to_string(),
            // AWS AppConfig's does the same.
            Self::AppConfigResource(_) => message.to_string(),
            // Elastic Load Balancing's does the same.
            Self::ElasticLoadBalancing(_) => message.to_string(),
        }
    }

    /// Which `HandlerErrorCode` class this type's provider puts a refusal carrying `code`
    /// in — see
    /// [`Provisioner::handler_error_class`](super::provisioner::Provisioner::handler_error_class).
    pub(super) fn handler_error_class(self, code: &str, classified: &'static str) -> &'static str {
        match self {
            Self::Builtin(provisioner) => provisioner.handler_error_class(code, classified),
            // A rule's PRIORITY is where its listener evaluates it — "a listener can't have
            // multiple rules with the same priority" — so a priority that listener already
            // holds a rule at is a rule that ALREADY EXISTS at the place the declaration asks
            // for, which is the class this provider puts that refusal in rather than the
            // in-use class the code's own wording reads as. The distinction is what a client
            // branches on: an already-exists failure is one no retry of the same template can
            // clear.
            Self::ElasticLoadBalancing(_) if code.contains(PRIORITY_IN_USE) => {
                super::HANDLER_ERROR_ALREADY_EXISTS
            }
            // Every other provider here reports the class the error code itself falls in.
            _ => classified,
        }
    }
}

/// The error code Elastic Load Balancing refuses a rule asking for a priority its listener
/// already evaluates a rule at with: the `awsQueryError` code of `CreateRule`'s own
/// `PriorityInUseException`
/// (`smithy_query(query="operation",operation="CreateRule")`: `awsQueryError {code:
/// 'PriorityInUse', httpResponseCode: 400}`), which `SetRulePriorities` refuses the same
/// conflict with.
const PRIORITY_IN_USE: &str = "PriorityInUse";

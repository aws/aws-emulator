// SPDX-License-Identifier: CC0-1.0

//! Creating a stack's declared resources in the services that own them, and deleting them
//! when the stack is deleted.
//!
//! ## The rule this module implements
//!
//! A stack is not just a record of a template. When CloudFormation creates a stack it
//! calls the OWNING service's own create API once per resource the template's `Resources`
//! section declares, and it deletes those same resources when the stack is deleted. What
//! each resource IS — the physical id a `Ref` resolves to and the attributes an
//! `Fn::GetAtt` resolves to — is computed by [`super::template`]; what this module adds is
//! the call that makes the resource exist, carrying the tags CloudFormation tags it with
//! ([`StackProvenance`]).
//!
//! The call is made in-process through the handler's own callback into this emulator
//! ([`smithy_server_runtime::OperationContext::callback`]), signed with the emulator's
//! credentials: the resource is created by the SAME code path a client's own
//! `CreateDeliveryStream` or `CreateBucket` goes through, so a stack cannot create a
//! resource that differs from one the service would have created itself.
//!
//! There are TWO shapes that call takes, and which one a type uses follows from what its
//! properties are:
//!
//! * on the owning service's WIRE PROTOCOL, through [`call_json_rpc`] here — for a type
//!   whose properties ARE the create request's own members, like
//!   `AWS::KinesisFirehose::DeliveryStream`, where the evaluated property DOCUMENT is
//!   passed through verbatim with only the name CloudFormation decides set on the way out.
//!   A typed SDK builder cannot carry a document through: it would need every member of
//!   every destination configuration mapped by hand, which is precisely the mapping that
//!   would drift from the model as the service grows;
//! * through the owning service's official AWS SDK client over this same in-process
//!   callback — for a type whose properties are NOT the create's members and whose service
//!   speaks a protocol other than awsJson (`AWS::S3::Bucket` on restXml through
//!   `aws-sdk-s3`, `AWS::IAM::Role` on awsQuery through `aws-sdk-iam`), where the members
//!   the create takes are named one by one anyway, so the typed builder costs nothing and
//!   the protocol's serialization is the SDK's rather than this crate's.
//!
//! Both shapes ride the one signed, in-process transport below ([`dispatch`]), so neither
//! reaches the network and both are served by the router that serves a client's own call.
//!
//! ## What this module owns, and what [`super::provisioner`] owns
//!
//! This module is the part that is the SAME for every resource type: reconciling what a
//! deployment declares against what the stack already has ([`reconcile`]), deleting what
//! it replaced or what the stack's own deletion owes ([`delete`]), and the in-process,
//! signed transport each of those calls rides on ([`call_json_rpc`] for a service whose
//! model declares awsJson, [`call_query`] for one that declares awsQuery — the wire is the
//! called service's, not this one's). The part that is
//! per type — which service owns it, and the create, update and delete calls that make its
//! resources exist, change and stop existing — is [`super::provisioner`]'s catalogue for
//! every type it already carries, and a module of its own for a type added since that file
//! passed the length a change to this repository may touch ([`lambda_function`],
//! [`lambda_permission`]). Either way the algorithm reaches it only through [`owner_for`],
//! so adding a resource type never touches the algorithm here.
//!
//! A type whose properties are not the create's own members is mapped the one way its
//! owning API takes them, and no further: an `AWS::SQS::Queue`'s properties are the
//! queue's ATTRIBUTES, named identically on both sides, so they are passed through as the
//! `CreateQueue` `Attributes` map ([`queue_attributes`]) rather than as request members.
//! Either way the mapping is the documented correspondence between the type's property
//! names and the API's, never a per-member translation table.
//!
//! ## What a resource type needs before a stack can create it
//!
//! [`owner_for`] maps a template resource type
//! to the operations of the service that owns it. The type usually names that service
//! itself: its middle segment is the service model's own `awsService.cloudFormationName`
//! (Amazon Data Firehose's model carries `cloudFormationName: KinesisFirehose`, so its
//! streams are `AWS::KinesisFirehose::DeliveryStream`). Where CloudFormation's own
//! resource reference publishes a different name the published one wins — Amazon ECR
//! Public's model carries `cloudFormationName: ECRPUBLIC`, but its repositories are
//! published as `AWS::ECR::PublicRepository`, beside the private registry's
//! `AWS::ECR::Repository` — because the type is what a template spells, not what a model
//! suggests.
//!
//! What a type's PROPERTIES are decides how its create call is built, which is why that
//! half lives per type in [`super::provisioner`]: for
//! `AWS::KinesisFirehose::DeliveryStream` the properties ARE the request's members, so the
//! evaluated document is passed through verbatim; for `AWS::ECR::PublicRepository` and the
//! `AWS::OpenSearchServerless::*` types they are not (`RepositoryName` /
//! `RepositoryCatalogData.RepositoryDescription` against `repositoryName` /
//! `catalogData.description`, and `Name`/`Type`/`Policy` against `name`/`type`/`policy`), so
//! those arms map the members the resource reference publishes one by one, and send nothing
//! for a property the template leaves out; and for `AWS::ECS::Cluster` they are the same
//! members in a different CASE, with one renamed, so that arm re-cases them.
//!
//! A type with no entry there is left DECLARED-ONLY: [`StoredResource::materialised`]
//! stays `false` and nothing is created in any service. Such a resource still carries the
//! physical id and the attributes CloudFormation DOCUMENTS for its type, because those are
//! computed from the template and the stack's own identity; that is what lets a stack's
//! `Outputs` name a resource of a type no service here creates yet. The per-resource reads
//! (`DescribeStackResources` and its siblings) report it as the stack DECLARED it, with
//! the status of the deployment that recorded it — this emulator's inherited scope for
//! such a type, not a claim that the resource exists in that service. Adding a provisioner
//! for another type is a local change: one arm in [`owner_for`] and the create/delete pair
//! it names, and that type's resources then really are created and deleted.
//!
//! ## Whose the resource's IDENTITY is
//!
//! For most types CloudFormation decides it: "Ref returns the bucket name", the queue url
//! it built, the stream name the template asked for — [`super::template`] computes the
//! physical resource id from the template and the stack, and the create call carries it
//! OUT to the owning service as the name of the resource to make.
//!
//! For a type whose owning service ASSIGNS the resource's identity it is the other way
//! round ([`super::template::identity_is_service_assigned`]): FIS mints an experiment
//! template's id, AWS WAF Classic answers `CreateIPSet` with an `IPSetId` of its own and
//! "Ref returns the IPSet ID", and Amazon EMR answers `RunJobFlow` with the `j-` job-flow id
//! an `AWS::EMR::Cluster`'s `Ref` returns — so the id a stack's `Ref`, its `Outputs` and its
//! per-resource reads report is the one that service answered, and the name CloudFormation
//! computed is only what the create asked for. [`reconcile`] adopts that id ([`Created`])
//! and the `Id`/`<Type>Id` attributes are re-pointed at it
//! ([`adopt_minted_identity`]), while a later deployment of the same resource keeps it
//! rather than recomputing one ([`super::StoredStack::deploy_template`]): the stack's own
//! deletion has to address the resource by the id its service knows it by.
//!
//! A type that DOES have an entry is never passed off as created: whatever its owning
//! service answers short of success — a modelled refusal, the 501 of an operation this
//! emulator has not implemented, or an answer that identifies nothing at all — leaves the
//! stack `CREATE_FAILED` with that answer as the reason, because the one thing an
//! attempted create that did not answer 2xx tells you is that the resource was not
//! created. A declaration that is missing a property its resource type REQUIRES is
//! refused the same way, naming that property.
//!
//! ## An identifier the owning service assigns
//!
//! Most types' physical resource id is a name the template asked for, which is why
//! [`super::template::provision`] can compute it. Some are SERVER-minted instead: an
//! OpenSearch Serverless collection's `Ref` is its `CollectionId`, which only the create
//! reports. Such a type's create answers that identifier ([`Created`]) and it replaces the
//! computed one, and
//! [`identity_is_service_assigned`](super::template::identity_is_service_assigned) is what
//! tells a re-deploy to carry the recorded identifier forward rather than read the
//! resource as replaced.
//!
//! A resource of ANOTHER such resource — a Resolver rule routing through a Resolver
//! endpoint of the same template — names it by a reference the declaration resolved before
//! either existed, so that property holds the stand-in the naming rules computed. Once the
//! endpoint's create has answered, a property of a resource this walk has not reached yet
//! whose value IS that stand-in is re-pointed at the minted identifier
//! ([`repoint_references`]): the create carries the identifier the owning service knows its
//! dependency by. A declaration that composes such an identifier INTO ITS OWN NAME — a log
//! group named `/aws/<service>/<kind>/<the api id of an api in the same template>` — is the
//! same case one level in: the identity the naming rules took from that name carries the
//! stand-in too, so it is re-pointed with the declaration ([`repoint_identity`]) and the
//! resource is created under the name every reader of that dependency builds. Only the
//! declarations that READ the minted resource are re-pointed with its pair where the stand-in
//! is the NAME the declaration asked for ([`pairs_for`]), since that is a value any other
//! declaration may state as its own. A LATER
//! deployment resolves its declarations against the identifiers the
//! stack already has ([`super::StoredStack::deploy_template`]), which is what makes a
//! re-deploy of the same template read as unchanged.
//!
//! ## Identity, and the isolation these calls can and cannot carry
//!
//! The call is signed with the EMULATOR's own credentials, because that is what they are
//! for: "The emulator's own (root) credentials, `ctx.callback().credentials()` and
//! `ctx.callback().aws_credentials_provider()`, only sign this operation's in-process
//! calls to other services" (`AGENTS.md`). The caller's own signature cannot be reused —
//! the emulator never holds a client's secret (its `CredentialStore` holds only
//! credentials an operation minted), so no signature of the caller's could be produced
//! here.
//!
//! What the call is signed with is not what DECIDES it, though. A stack operation acts as
//! the stack's service role — "CloudFormation uses the role's credentials to make calls on
//! your behalf" — so the permissions that role carries are what say whether the change this
//! call asks for may be made at all, and a call that role is denied is never sent
//! ([`super::service_role`], asked in [`call_json_rpc`] and [`call_query`] before either
//! writes a request).
//!
//! Nor is there state for a credential to select. Every service of this emulator reaches
//! its store by TYPE out of the ONE `InMemoryEngine` the router builds and hands to all of
//! them (`AGENTS.md`: "`engine` is deliberately ONE crate shared by every service crate
//! and the router"; `::engine::InMemoryEngine::state::<T>()` takes no identity), and every
//! ARN it mints carries the single canonical account id
//! (`::emulator_server::EMULATOR_ACCOUNT_ID`). A stack's stream therefore lands in exactly
//! the store a client's own `CreateDeliveryStream` writes to — which is the point of
//! provisioning it — and the account half of a credential scope has one value, so the
//! REGION is the only part there is to propagate: these calls carry the stack's own
//! Region, the one the call that created it was signed for, on both the create and the
//! delete.
//!
//! What this module does keep apart is one STACK's resources from another's: a stack's
//! deletion deletes the resources ITS OWN records say it created
//! ([`StoredResource::materialised`]), never everything an owning service happens to hold.
//! Whether a resource another stack (or a client) already created can be deployed over is
//! the OWNING service's answer, not this module's: Firehose refuses the create with its own
//! uniqueness rule ("This name must be unique per Amazon Web Services account in the same
//! Amazon Web Services Region"), which leaves the second stack `CREATE_FAILED` with that
//! refusal and the existing stream untouched, while S3 answers a re-create of a bucket the
//! caller already owns with 200 in us-east-1 ("for legacy compatibility, if you re-create
//! an existing bucket that you already own…"), so that create is accepted and the bucket
//! is left exactly as it was — nothing is re-created and nothing in it is cleared. The
//! stack's own deletion then deletes what ITS records say it created, so a bucket it
//! deployed over is deleted by the stack that recorded it, and one that still holds objects
//! is refused by S3 and reported as `DELETE_FAILED`. Both answers are held to by
//! `core/router/tests/all/cloudformation_provisions_stack_resources.rs` and by
//! `tests/all/stack_resource_provisioning.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::Value;

use super::provisioner::{provisioner_for, Provisioner};
use super::StoredResource;

/// The `AWS::ApiGateway::*` types of the REST control plane: created in Amazon API Gateway
/// itself ([`apigateway_rest_api`]). Visible to the rest of this service's state because two
/// of those types' own NAMES live here, beside the arms that provision them (the replacement
/// rules read the properties each record cannot take anew, which are the labels that
/// service's own calls address one by).
pub(in crate::state) mod apigateway_rest_api;
/// The four `AWS::Athena::*` types a stack declares — the workgroup its queries run in, the
/// data catalog they are routed through, the query it saves in that workgroup and the capacity
/// its queries run on: created in
/// Amazon Athena itself ([`athena_resources`]). Visible to the rest of this service's state
/// because the saved query's own type NAME lives here, beside the arm that provisions it (the
/// identity in force is the id that create answers, and the properties such a query cannot take
/// anew are read off the type).
pub(in crate::state) mod athena_resources;
/// The two AppSync custom-domain types a stack declares — the domain itself and the mapping
/// of an API onto it: visible to the rest of this service's state because the types' own
/// NAMES live here, beside the arms that provision them (the naming rules read the property
/// each takes its identifier from, and the replacement rules the one that is fixed when the
/// domain is made).
pub(in crate::state) mod appsync_domain_names;
/// The four `AWS::AppConfig::*` types a stack declares — the application, its environment,
/// the configuration profile that reads a configuration and the deployment strategy that
/// rolls one out: created in AWS AppConfig itself ([`appconfig_resources`]).
mod appconfig_resources;
/// The two `AWS::AppConfig::*` types a ROLLOUT is declared with — the configuration content
/// hosted for a profile, and the deployment of one version of it into an environment: created in
/// AWS AppConfig itself ([`appconfig_rollout`]).
mod appconfig_rollout;
/// The three `AWS::AppSync::*` types a stack declares beyond a GraphQL API's own estate — an
/// Event API, the channel namespaces events travel over on it, and the link of a source API
/// into a Merged API: created in AWS AppSync itself ([`appsync_event_and_merged_apis`]).
mod appsync_event_and_merged_apis;
/// The six `AWS::AppSync::*` types a GraphQL API's estate is declared with — the API, its
/// schema, its data sources, its resolvers, its pipeline functions and its API keys: created
/// in AWS AppSync itself ([`appsync_resources`]).
mod appsync_resources;
/// The documents an `AWS::AppSync::*` declaration holds in Amazon S3 rather than in the
/// template — a resolver's or a function's code, a schema's definition, a mapping template, a
/// channel namespace's handlers — read from the object the declaration addresses and submitted
/// as the property whose text they are ([`appsync_s3_documents`]).
mod appsync_s3_documents;
/// Reading the Amazon S3 OBJECT a declaration addresses as the text of the property whose
/// document it is — the one reading a resource PROVIDER of this crate makes of a staged object
/// ([`staged_object`]); a read made AS THE CALLER is [`crate::state::template_url`]'s.
mod staged_object;
/// The OpenAPI definition an api declaration holds in Amazon S3 rather than in the template,
/// read from the object the declaration addresses and imported as the document it is
/// ([`apigateway_definitions`]). Visible to the rest of this service's state because BOTH api
/// types publish that reference and one of the two arms that reads it lives in
/// [`crate::state::provisioner`].
pub(in crate::state) mod apigateway_definitions;
/// Reading a declared property document as the TYPES the owning API models its members as —
/// the one walk every provisioning arm whose owning model types a member as a number or a
/// boolean reaches ([`modelled_declaration`]).
mod modelled_declaration;
// Re-exported to the rest of `crate::state` because the per-type arms of
// [`crate::state::provisioner`] are in the same position as the ones here: the owning model of
// the call THEY make types members as numbers and booleans too, so they read a declaration
// through this one walk rather than a second copy of it.
pub(in crate::state) use modelled_declaration::{modelled_declaration, ModelledScalar};
/// What Amazon ECS models the members of its create calls as, where that is not text — the
/// member catalogue the three `AWS::ECS::*` arms read a declaration through
/// ([`ecs_scalars`]). Visible to the rest of this service's state because two of those arms
/// live in [`crate::state::provisioner`].
pub(in crate::state) mod ecs_scalars;
/// The three Amazon SageMaker AI Studio types a stack declares — the domain, a user profile of
/// one and a shared space in one: created in that control plane itself ([`sagemaker_studio`]).
/// Visible to the rest of this service's state because the types' own NAMES live here, beside
/// the arms that provision them (the return-value rules name all three, the model validation the
/// properties each one requires).
pub(in crate::state) mod sagemaker_studio;
/// The five `AWS::ServiceDiscovery::*` types a Cloud Map namespace's estate is declared with —
/// an HTTP, a public DNS and a private DNS namespace, a service of one, and an instance
/// registered in a service: created in AWS Cloud Map itself ([`service_discovery`]).
mod service_discovery;
/// The two `AWS::ApplicationAutoScaling::*` types a stack declares — the scalable target and
/// the policy that steers it: visible to the rest of this service's state because the types'
/// own NAMES live here, beside the arms that provision them
/// ([`super::template::return_values`] names the two whose identity their create answers).
pub(super) mod application_auto_scaling;
/// Where `AWS::AutoScaling::ScalingPolicy` is provisioned — the catalogue's own arm
/// ([`crate::state::provisioner::auto_scaling_policies`]) — beside the guard that keeps the
/// type to ONE owner, so no module reached earlier by [`owner_for`] can shadow it
/// ([`autoscaling_scaling_policy`]).
mod autoscaling_scaling_policy;
/// The two CloudFront types a stack declares — the distribution and the origin access
/// control its origins reach a private bucket with: created in Amazon CloudFront itself
/// ([`cloudfront_resources`]).
mod cloudfront_resources;
mod cognito_identity_pool;
/// The three Amazon Cognito user-pool types a stack declares — the user pool, the app client
/// that signs in against it and the identity provider it federates with: visible to the rest
/// of this service's state because the types' own NAMES live here, beside the arms that
/// provision them ([`super::template::return_values`] names the two whose identity their
/// create answers, and the one whose name property it reads).
pub(in crate::state) mod cognito_user_pool;
/// `AWS::DynamoDB::Table`: visible to the rest of this service's state because the type's
/// own NAME lives here, beside the arm that provisions it, and more than one module answers
/// about that type ([`super::resource_inventory`]).
pub(super) mod dynamodb_table;
/// `AWS::EC2::Instance`: the instance a stack declares, launched in Amazon EC2 itself
/// ([`ec2_instance`]).
mod ec2_instance;
/// `AWS::EC2::SpotFleet`: the Spot Fleet request a stack declares, made in Amazon EC2 itself
/// ([`ec2_spot_fleet`]).
mod ec2_spot_fleet;
/// The three `AWS::EFS::*` types an Amazon EFS estate is declared with — the file system, an
/// access point of one and the mount target a VPC reaches it through: created in Amazon EFS
/// itself ([`efs_resources`]). Visible to the rest of this service's state because the types'
/// own NAMES live here, beside the arms that provision them (the return-value rules name all
/// three, whose identity their creates answer, and the replacement rules the properties one
/// cannot take anew).
pub(in crate::state) mod efs_resources;
/// The Amazon EC2 VPC-network types a stack declares, created in Amazon EC2 itself, beside
/// the readers of that service's own answers that their arms are built from: visible to the
/// rest of this service's state because the reading of a describe's RESULT SET
/// ([`ec2_vpc_network::set_entry`]) is what resolves an AWS-specifically typed parameter's
/// value too ([`super::parameter_constraints::aws_specific_types`]).
pub(super) mod ec2_vpc_network;
/// `AWS::ECR::PullThroughCacheRule`: the pull through cache rule a stack wires, registered in
/// the private Amazon ECR registry itself. Visible to the rest of this service's state because
/// the type's own NAME lives here, beside the arm that provisions it
/// ([`ecr_pull_through_cache_rule`]).
pub(in crate::state) mod ecr_pull_through_cache_rule;
/// `AWS::ECS::TaskDefinition`: the revision a stack declares, registered in Amazon ECS
/// itself ([`ecs_task_definition`]).
mod ecs_task_definition;
/// The five `AWS::ElasticLoadBalancingV2::*` types a stack declares — the load balancer, the
/// target group traffic is routed to, the listener on the load balancer, the rule that
/// routes a request and the trust store client certificates are authenticated against:
/// created in Elastic Load Balancing itself
/// ([`elastic_load_balancing`]).
pub(super) mod elastic_load_balancing;
/// `AWS::ElasticLoadBalancingV2::ListenerCertificate`: the certificates a stack adds to the
/// certificate list of a listener, added in Elastic Load Balancing itself
/// ([`elbv2_listener_certificate`]).
mod elbv2_listener_certificate;
mod iam_policy;
/// `AWS::IoT::Authorizer`: the custom authorizer a stack declares, created in AWS IoT
/// itself ([`iot_authorizer`]).
pub(super) mod iot_authorizer;
/// `AWS::IoT::ProvisioningTemplate` and `AWS::IoT::RoleAlias`: the fleet-provisioning
/// template a stack declares and the role alias a device assumes credentials through, each
/// created in AWS IoT itself ([`iot_provisioning_resources`]).
pub(super) mod iot_provisioning_resources;
/// `AWS::IoT::Thing`: the thing a stack declares, registered in AWS IoT's own thing
/// registry ([`iot_thing`]).
pub(super) mod iot_thing;
/// `AWS::IoT::TopicRule`: the topic rule a stack declares, created in AWS IoT itself
/// ([`iot_topic_rule`]).
pub(super) mod iot_topic_rule;
mod lambda_alias;
mod lambda_event_invoke_config;
/// `AWS::Lambda::EventSourceMapping`: the mapping a stack declares, created in AWS Lambda
/// itself. Visible to the rest of this service's state because the type's NAME and the
/// properties a mapping cannot take a new value for live here, beside the arm that
/// provisions it ([`lambda_event_source_mapping`]).
pub(super) mod lambda_event_source_mapping;
mod lambda_function;
/// `AWS::Lambda::LayerVersion`: the layer version a stack declares, published in AWS Lambda
/// itself. Visible for the reason [`lambda_event_source_mapping`] is
/// ([`lambda_layer_version`]).
pub(super) mod lambda_layer_version;
mod lambda_permission;
/// The member catalogue every `AWS::Lambda::*` arm reads its declaration through, so a
/// quoted number or boolean reaches AWS Lambda as the type that service models it as
/// ([`lambda_scalars`]).
mod lambda_scalars;
use lambda_scalars::lambda_declaration;
mod lambda_version;
/// The resources of ONE level of a stack's dependency relation, asked about together, so a
/// stack operation costs about one coming-up window per LEVEL rather than one per resource
/// ([`level_readiness`]).
mod level_readiness;
/// The Amazon CloudWatch Logs types a stack declares — a log group, a stream inside one and
/// a metric filter on one: visible to the rest of this service's state because the types'
/// own NAMES live here, beside the arms that provision them, and more than one module answers
/// about them ([`super::template`] reads a metric filter's own name property).
pub(super) mod logs_resources;
/// The three `AWS::MediaConvert::*` types a transcoding estate is declared with — the queue
/// jobs run in, the job template they are created from and the preset an output is written
/// with: created in AWS Elemental MediaConvert itself ([`mediaconvert_resources`]).
mod mediaconvert_resources;
/// `AWS::CloudFormation::Stack`: the NESTED STACK a stack declares, created in
/// CloudFormation itself ([`nested_stack`]).
mod nested_stack;
/// Whether the request being served is a read THIS service made while provisioning, and which
/// stack it is about — what keeps one client call to one pass per stack ([`progress_probe`]).
mod progress_probe;
/// `AWS::S3::Bucket`: the halves of that type's provider the catalogue cannot carry — how a
/// changed declaration reaches a bucket that exists, and how the provider words a refusal.
mod s3_bucket;
/// `AWS::SecretsManager::ResourcePolicy`: the resource-based policy a stack attaches to a
/// secret, attached in AWS Secrets Manager itself
/// ([`secretsmanager_resource_policy`]).
mod secretsmanager_resource_policy;
/// `AWS::SecretsManager::RotationSchedule`: the rotation a stack binds to a secret, put in
/// force in AWS Secrets Manager itself ([`secretsmanager_rotation_schedule`]).
mod secretsmanager_rotation_schedule;
/// `AWS::SecretsManager::Secret`: the secret a stack declares, created in AWS Secrets
/// Manager itself ([`secretsmanager_secret`]).
mod secretsmanager_secret;
/// `AWS::SSM::PatchBaseline`: the patch baseline a stack declares and the patch groups it
/// registers that baseline for, created in AWS Systems Manager itself
/// ([`ssm_patch_baseline`]). Visible to the rest of this service's state because the type's
/// own NAME lives here, beside the arm that provisions it (the return-value rules name the
/// type whose identity its create answers).
pub(in crate::state) mod ssm_patch_baseline;
mod step_functions_state_machine;
/// The two `AWS::StepFunctions::*` types a stack declares beside a state machine — the
/// VERSION it publishes of one and the ALIAS that routes traffic to versions: created in AWS
/// Step Functions itself ([`step_functions_versions_and_aliases`]). Visible to the rest of
/// this service's state because the types' own NAMES live here, beside the arms that
/// provision them (the naming rules fit the alias's generated name to that service's bound,
/// and the return-value rules name the two whose identity their create answers).
pub(in crate::state) mod step_functions_versions_and_aliases;

/// Custom resources: the provider a template names in a resource's `ServiceToken`, the
/// request CloudFormation sends it on every stack operation touching the resource, and the
/// answer the provider PUTs to the `ResponseURL` the request carries.
pub mod custom_resource;
/// `AWS::CloudFormation::WaitConditionHandle`: the pre-signed write target such a resource
/// IS, which CloudFormation itself mints as the handle's physical id, and the signal document
/// a client PUTs to it ([`wait_condition_handle`]).
pub mod wait_condition_handle;
/// Calling another emulated service in-process: the wires a resource provider reaches an
/// owning service's own handlers over, and how a refusal it answers is read
/// ([`in_process_calls`]).
mod in_process_calls;
pub use in_process_calls::*;

/// The second half of a deployment's walk: the resources it WITHHELD because a resource
/// they read had not published its attributes yet, created once that resource has.
mod deferred;

/// The rest of a TEARDOWN: the resources a deletion left alone because something that
/// depends on them was still being removed, asked about once that removal has answered
/// ([`outstanding_teardown`]).
mod outstanding_teardown;

/// Where each of this algorithm's questions is PUT to the module that owns a resource type
/// ([`Owner`]'s own methods, moved out so adding a type's arms does not grow this file).
mod owner;

/// The MODEL VALIDATION a resource provider runs on the declaration it is handed before it
/// calls the service that owns the type, and the schema validator's own sentence for a
/// property that type's schema marks required which the declaration does not carry
/// ([`model_validation`]).
mod model_validation;

/// The success signals the INSTANCES of a signalled resource send: what releases the
/// resource-signal wait of a resource whose signals come from the instances it consists of
/// ([`instance_signals`]).
mod instance_signals;
/// Carrying out the BATCHED replacement of an Auto Scaling group's instances an
/// `UpdatePolicy`'s `AutoScalingRollingUpdate` asks an update for ([`rolling_update`]).
pub mod rolling_update;
/// The DEPLOYMENT WALK's own helpers — what a create or an update reads about each
/// declaration before it asks the service that owns it ([`deployment_walk`]). Their own file
/// so this module stays inside the length the build holds every file to.
mod deployment_walk;
use deployment_walk::*;
/// Re-pointing a deployment's stand-ins at the identifiers the owning services MINTED: what
/// brings a declaration, and the identity of the resource it names, into step with what each
/// create really answered ([`repointing`]).
mod repointing;
/// How an owning service's own refusal is WORDED on the resource's event: the diagnostic its
/// SDK would have carried, the handler error class it falls in, and the two refusals
/// CloudFormation words itself (a name another stack manages, a call the service role is
/// denied).
mod sdk_diagnostics;
/// What a deployment does to ONE resource whose change the service that owns it refused:
/// whose work putting it back is, and the events that report it ([`failed_resource`]).
mod failed_resource;
use failed_resource::*;
pub(crate) use sdk_diagnostics::*;
use repointing::*;

/// AWS's reserved tag-key namespace: "the `aws:` prefix is reserved for AWS use". Matched
/// case-insensitively, as the services that refuse it match it.
pub(super) const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is one of the three tags CloudFormation records on every resource it
/// provisions ([`STACK_NAME_TAG`], [`STACK_ID_TAG`], [`LOGICAL_ID_TAG`]).
///
/// They sit inside the namespace [`is_reserved_tag_key`] names, and that is the point: they
/// are AWS's own tags, which CloudFormation — not a caller — sets. So a provider whose owning
/// service admits the reserved namespace on the in-process path it is called over sends these
/// three and drops every OTHER reserved key, which is a caller's to have supplied.
pub(super) fn is_stack_provenance_tag(key: &str) -> bool {
    [STACK_NAME_TAG, STACK_ID_TAG, LOGICAL_ID_TAG].contains(&key)
}

/// Whether `key` is in that namespace — the ONE reading of the rule for the per-type providers
/// beside this algorithm, so a second provider cannot come to read it differently.
///
/// A service refuses such a key on every call that takes a CALLER's tag set ("Your TagKey
/// cannot be prefixed with aws:"), and the three `aws:cloudformation:*` tags AWS sets from
/// inside itself have no public call to set them through — so a provider whose owning service
/// does not tell a caller's request from a provisioning one leaves them off rather than sending
/// them to be refused: a deployment must not fail over tags CloudFormation adds of its own
/// accord. A provider whose owning service DOES tell the two apart sends them, and the
/// resource carries them ([`is_stack_provenance_tag`]).
/// ([`super::provisioner::Provisioner::applicable_tags`] is the catalogue's own reading of it,
/// for the types it carries.)
pub(super) fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Those of a propagated tag set an owning service's own tagging call will TAKE: every key
/// outside the reserved namespace ([`is_reserved_tag_key`]).
pub(super) fn applicable_tags(propagated: &[(String, String)]) -> Vec<(String, String)> {
    propagated
        .iter()
        .filter(|(key, _)| !is_reserved_tag_key(key))
        .cloned()
        .collect()
}

/// `name` with its initial character LOWER-cased, which is the one rule that turns a resource
/// type's property name into the API member it is (`LoggingConfiguration` ->
/// `loggingConfiguration`, `FunctionArn` -> `functionArn`, `DynamoDBv2` -> `dynamoDBv2`).
///
/// A CloudFormation resource type's properties are the API members of the resource it wraps with
/// their initial upper-cased, so the two halves of that rule are the whole of the mapping for a
/// type whose API takes its own members as the declaration spells them. They live here, ONCE,
/// because every per-type arm that maps a declaration this way needs them and a second copy can
/// case one name differently: `iot_topic_rule` and `step_functions_state_machine` take them from
/// here, and the copies in `ecs_task_definition` and `appsync_resources` are to be migrated onto
/// them, each in the change that owns its type.
pub(super) fn lower_camel(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// `name` with its initial character UPPER-cased — the inverse of [`lower_camel`], which is how
/// an answer an owning service gave in its own member names reads back in the type's property
/// vocabulary.
pub(super) fn upper_camel(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The tags a declared resource's owning service will STORE, for a service that admits the
/// reserved namespace on the in-process path this module calls it over: the ones the
/// declaration asks for and the ones CloudFormation propagates
/// ([`super::provisioner::tags_of`]), less every key in that namespace which is not
/// CloudFormation's own.
///
/// The three records CloudFormation keeps about a resource it provisioned
/// ([`is_stack_provenance_tag`]) TRAVEL, so the owning service's own read of the handle a
/// stack publishes reports them. Every OTHER reserved key is filtered out here as well,
/// though no declaration reaches this point carrying one: a declaration claiming a key of
/// that namespace fails the resource before any of this
/// ([`super::stack_tags::declared_resource_tag_refusal`]), so what this filter keeps out is a
/// key that got into the propagated set by some other route.
///
/// The ONE reading of that split for the per-type providers beside this algorithm, so a later
/// change to it cannot be applied to some of them and not the rest. A provider whose owning
/// service refuses the namespace to EVERY caller drops all of it instead
/// ([`applicable_tags`], and [`dynamodb_table`]'s own reading of the same rule).
pub(super) fn storable_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    crate::state::provisioner::tags_of(properties, propagated)
        .into_iter()
        .filter(|(key, _)| is_stack_provenance_tag(key) || !is_reserved_tag_key(key))
        .collect()
}

/// One tag set as the `[{Key, Value}]` LIST most AWS models spell a `Tag` with — Amazon
/// DynamoDB's `CreateTable$Tags` and AWS Secrets Manager's `CreateSecret$Tags` among them.
///
/// The ONE rendering of that form for the per-type providers beside this algorithm, so two of
/// them cannot come to write the same shape differently. A service whose model spells the
/// members otherwise (AWS Step Functions' `key`/`value`) renders its own.
pub(super) fn key_value_tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Whether two tag sets are the same SET: a tag set is keyed by its keys, so the order the two
/// were read in says nothing about whether they agree.
///
/// Read by every provider that RECONCILES a tag set — one that asks the owning service what
/// the resource carries before writing ([`s3_bucket::retag`], [`nested_stack::retag`]) — so
/// one comparison serves them all.
pub(super) fn tag_sets_agree(
    in_force: &[(String, String)],
    wanted: &[(String, String)],
) -> bool {
    in_force.len() == wanted.len()
        && wanted.iter().all(|(key, value)| {
            in_force
                .iter()
                .any(|(held, carried)| held == key && carried == value)
        })
}

/// The awsJson media types the services this module calls speak, named once here because
/// [`call_json_rpc`] takes the one the called service's own model declares: an answer is
/// rendered in it, and a call written in the other version is not the wire that service
/// publishes.
pub(super) const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";
pub(super) const AWS_JSON_1_1: &str = "application/x-amz-json-1.1";

/// The media type a REST-JSON service's requests and answers carry
/// ([`call_rest_json`]): a `restJson1` service publishes `application/json`, not one of
/// the awsJson versions above.
pub(super) const REST_JSON: &str = "application/json";

/// The media type a REST-XML service's requests and answers carry ([`call_rest_xml`]): a
/// `restXml` service (Amazon Route 53) takes an XML document rather than a JSON one.
pub(super) const REST_XML: &str = "application/xml";

/// AWS Secrets Manager's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: secretsmanager`, service shape `secretsmanager`), which speaks `awsJson1_1`.
///
/// Here rather than in one of the modules that call it, because more than one provisions a
/// resource of that service: the SECRET itself ([`secretsmanager_secret`]), the rotation
/// configured on it ([`secretsmanager_rotation_schedule`]) and the resource policy attached
/// to it ([`secretsmanager_resource_policy`]), which have to address the same service the
/// same way.
pub(super) const SECRETSMANAGER_SIGNING_NAME: &str = "secretsmanager";
/// See [`SECRETSMANAGER_SIGNING_NAME`].
pub(super) const SECRETSMANAGER_TARGET_PREFIX: &str = "secretsmanager";

/// Call one AWS Secrets Manager operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_secretsmanager(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SECRETSMANAGER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SECRETSMANAGER_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// What the service that owns a resource answered when it created it.
///
/// `physical_resource_id` is set only by a type whose id that service ASSIGNS (an FIS
/// experiment template's `EXT…`, an AWS WAF Classic IPSet's `IPSetId`), which no naming rule
/// could have computed: it replaces the computed id, because it is what `Ref` resolves to
/// and what a client calls the service with. For every other type the computed id IS the one
/// the resource was created under (a queue's name, a cluster's name), so the arm leaves it
/// alone.
#[derive(Debug, Clone, Default)]
pub(super) struct Created {
    pub(super) physical_resource_id: Option<String>,
    pub(super) attributes: BTreeMap<String, String>,
}

impl Created {
    /// What a create that reported only ATTRIBUTES answered: the resource keeps the
    /// physical id the naming rules computed.
    pub(super) fn attributes(attributes: BTreeMap<String, String>) -> Self {
        Self {
            physical_resource_id: None,
            attributes,
        }
    }
}

/// The tag key CloudFormation records the NAME of the stack a resource belongs to under,
/// on the resource itself.
pub const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
/// The tag key it records that stack's ID under.
pub const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
/// The tag key it records the resource's LOGICAL id in that stack under.
pub const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// The stack a deployment is provisioning for, as far as the resources it creates are
/// concerned: the identity CloudFormation records ON each of them, and the stack-level tags
/// it propagates TO each of them. The Region those resources live in travels beside this as
/// [`reconcile`]'s own argument, because it is also what the credential scope of every call
/// this module makes names.
///
/// ## The rule this carries
///
/// CloudFormation does not only create a stack's resources — it TAGS them. The stack-level
/// `Tags` a stack operation supplies are "propagated to the resources that CloudFormation
/// creates in the stack", and CloudFormation adds three tags of its own naming the stack
/// and the resource's place in it ([`STACK_NAME_TAG`], [`STACK_ID_TAG`],
/// [`LOGICAL_ID_TAG`]). So the owning service's own tag read on the handle a stack
/// publishes for a resource answers with those tags — which is what tag-based cost
/// allocation, tag-based access control and anything keying off `aws:cloudformation:*` see.
///
/// Applying them is PER TYPE, in [`super::provisioner`], because the tag set reaches each
/// owning API its own way (a `Tags` member of the create, a map, a tagging call of its own);
/// what is the same for every type is the set itself, which is [`Self::tags_for`].
#[derive(Debug, Clone, Default)]
pub struct StackProvenance {
    /// `Stack$StackName`.
    pub stack_name: String,
    /// `Stack$StackId`.
    pub stack_id: String,
    /// The stack-level `Tags` in force, in the order the stack records them.
    pub tags: Vec<(String, String)>,
    /// The `Capabilities` in force on the stack — what the operation deploying it
    /// ACKNOWLEDGED. Carried because a resource that is itself a STACK is created under the
    /// parent operation's acknowledgement ("if you have IAM resources in nested stacks, you
    /// must specify CAPABILITY_IAM"), which CloudFormation does not ask for a second time.
    pub capabilities: Vec<String>,
    /// Whether the deployment in force on the stack runs in the EXPRESS deployment mode,
    /// which is part of how a failure of these resources is REPORTED: the mode names itself
    /// in the summary of what could not be created
    /// ([`super::create_action`]).
    pub express: bool,
    /// Which physical resource names the live stacks of the Region already manage, and which
    /// stack manages each ([`super::physical_name::ManagedName`]).
    ///
    /// CloudFormation holds that record itself, so a create the owning service refused
    /// because the name is already in use is reported in CLOUDFORMATION's own words, naming
    /// the stack that holds the name, rather than as that service's report about its own
    /// namespace ([`already_managed_reason`]). The operation that deploys the stack reads the
    /// index under the store lock
    /// ([`super::ComAmazonawsCloudformationCloudformationState::managed_physical_names`]),
    /// since the walk itself holds none.
    ///
    /// EMPTY where the deployment re-creates only resources the stack ALREADY owned — its own
    /// rollback, its continued rollback, a revert — because those names are this stack's and
    /// so collide with no other stack's.
    ///
    /// SHARED: one read per Region answers every deployment one request sets off
    /// ([`super::physical_name::ManagedNames`]).
    pub managed_names: super::physical_name::ManagedNames,
    /// What each resource of the template being deployed WAITS FOR
    /// ([`super::template::direct_dependencies`]).
    ///
    /// The walk reads it to decide whether a resource can be created AT ALL YET: a resource
    /// that depends on one whose own wait is still open — a CUSTOM RESOURCE whose provider
    /// has not answered, whose `Data` is what an `Fn::GetAtt` on it resolves against — is
    /// not asked of anybody in this pass, because the declaration that would be sent is not
    /// resolved yet ([`deferred`]). EMPTY is a relation the caller does not know, and then
    /// nothing is held back.
    pub dependencies: Dependencies,
    /// Whether a resource whose change this deployment could not carry through is rolled
    /// back INDIVIDUALLY and reported as such ([`roll_back_failed_resource`]).
    ///
    /// CloudFormation puts a resource whose change its provider refused back the way it was
    /// either way. What differs is WHOSE work that is: a deployment whose stack goes on to
    /// roll back reports the revert as part of that rollback, as the resource's own update
    /// reaching its terminal status; one whose stack RESTS in the failed status
    /// (`DisableRollback`) has no such rollback to report it, so the resource's own
    /// `UPDATE_ROLLBACK_*` transitions are what say the failed resource was put back.
    ///
    /// Set by the operation that deploys a CHANGED template (`UpdateStack`,
    /// `ExecuteChangeSet`), from the setting in force on the stack. Every other deployment
    /// reverts nothing ([`FailedResourceRevert::None`]): a rollback, a resumption and a
    /// revert ARE the putting back, and a create has no previous configuration of a resource
    /// to put back.
    pub failed_resource_revert: FailedResourceRevert,
    /// Whether the DECLARATIONS of this deployment can be resolved AGAIN once a value they
    /// read has moved — which is what lets the walk WITHHOLD a resource whose declaration
    /// reads an attribute this deployment's own change to another resource has just moved, and
    /// deploy it with the values in force then ([`deferred::resume`] resolves them through
    /// [`super::StoredStack::resolve_deferred_declarations`]).
    ///
    /// Set by the operation that deploys a TEMPLATE over resources the stack already has
    /// (`UpdateStack`, `ExecuteChangeSet`): the pending deployment it holds is what that
    /// re-resolution reads. Every other deployment deploys RECORDS rather than declarations —
    /// a rollback, a resumption and a revert put back the configuration the stack already had,
    /// which no re-resolution could produce — so nothing is withheld for one of those: a
    /// resource withheld there would be a resource nothing ever deploys.
    pub declarations_resolve_again: bool,
    /// The pre-condition on PUBLISHING this deployment's exports, where the deploying
    /// operation has one ([`super::exports::ExportPublication`]).
    ///
    /// Asked by this walk, once its changes have been made and BEFORE it takes down anything
    /// the deployment supersedes, because a deployment the pre-condition stops is one
    /// CloudFormation carries nothing of through — that file records the whole of why. `None`
    /// is a deployment with no such pre-condition (a create has no exports in force to
    /// withdraw, a rollback and a revert put back what was published), and then nothing is
    /// asked here.
    pub publication: Option<::std::sync::Arc<super::exports::ExportPublication>>,
    /// The resources of this deployment whose DECLARATION reads an attribute nothing publishes
    /// yet ([`super::StoredStack::declarations_reading_unpublished_attributes`]).
    ///
    /// A declaration is resolved into the document the owning service is asked for BEFORE the
    /// walk starts, and a read with nothing to resolve against leaves no member at all
    /// ([`super::template::resolve_properties`] drops a value it cannot evaluate). The value is
    /// not missing, though — it is the one a resource this same deployment creates goes on to
    /// publish — so creating the reader in the pass that publishes it would ask its service for
    /// a resource configured with less than the declaration states. Such a resource is
    /// therefore WITHHELD exactly as one waiting on a dependency still coming up is
    /// ([`awaits_a_dependency`]) and created by the pass that resolves the declarations again
    /// ([`deferred::resume`]).
    ///
    /// EMPTY for a deployment that deploys RECORDS rather than declarations — a rollback, a
    /// resumption, a revert — which has no declaration to resolve a second time, so nothing is
    /// withheld for one of those.
    pub reads_unpublished_attributes: BTreeSet<String>,
    /// Whether this deployment puts a DIFFERENT stack-level tag set in force than the one the
    /// resources it holds carry — so the tags above are a change to every one of them.
    ///
    /// The stack's tags are the tags of the resources it manages ("CloudFormation also
    /// propagates these tags to resources in the stack"), so a deployment that states them
    /// differently CHANGES every resource the stack holds whose type a service tags, under the
    /// `Tags` attribute rather than under `Properties` — which is why such a resource reports
    /// its own update even where its declaration is untouched ([`reconcile`]). This is the
    /// same comparison the PLAN of such a deployment makes
    /// ([`super::change_plan::tag_changes`]), read off the stack's own tag sets
    /// ([`super::same_tags`]), so a plan and its execution cannot disagree about which
    /// resources a tag change touches.
    ///
    /// Set by the operations that deploy a client's tag set (`UpdateStack`,
    /// `ExecuteChangeSet`). `false` everywhere else: a create tags the resources it creates as
    /// it creates them, and a rollback, a resumption and a revert put back a configuration the
    /// stack's resources already carry.
    pub tags_changed: bool,
    /// The logical ids this deployment ADOPTS: resources that already exist in the services
    /// that own them, which this deployment brings under the stack's management
    /// ([`super::resource_import`]).
    ///
    /// Nothing CREATED such a resource, so the tags CloudFormation gives a resource it
    /// provisions were never applied to it: a resource comes under the stack's management with
    /// the stack's own tags and the three records naming the stack and its place in it
    /// ([`StackProvenance::tags_for`]), so that set is applied to it as it is adopted — and
    /// since adopting it is what makes the resource the STACK's, applying that set is a change
    /// to it, which its own status and events report as the update it is, while the operation
    /// as a whole stays the import it is ([`reconcile`]).
    ///
    /// Set by `ExecuteChangeSet`, the one call that adopts anything; EMPTY for every other
    /// deployment, which creates and changes resources rather than adopting them — and for
    /// every later deployment over the adopted resource, which is one the stack HAS by then.
    pub adopted: BTreeSet<String>,
    /// The resources this deployment asks the owning service for the declared configuration
    /// of AGAIN, even where the stack's record already states it.
    ///
    /// A change to a resource that exists is not one call but several — a function's
    /// configuration and its deployment package are two — so a change the owning service
    /// refused PART WAY leaves the resource carrying some of it while the stack's record
    /// still states the configuration in force before it ([`carry_forward`]). CloudFormation
    /// puts such a resource back the only way it can: by declaring the previous
    /// configuration again in that service. That re-declaration reads everything the
    /// configuration points at — an `AWS::Lambda::Function`'s `Code` is fetched from Amazon
    /// S3 again — so a rollback the owning service will not take is a rollback that FAILED,
    /// reported as such, rather than one reported complete over a call nobody made.
    ///
    /// Filled by the ROLLBACK of a failed update and by the resumption of a rollback that
    /// failed, from the resources the walk that failed reported as part-changed
    /// ([`Materialised::part_changed`]). EMPTY for every deployment that deploys a
    /// TEMPLATE, which asks those services for what its declarations state and compares
    /// that with the records to decide what changed.
    pub reapplied: BTreeSet<String>,
}

/// Whether THIS deployment is what puts back a resource whose change it could not carry
/// through ([`roll_back_failed_resource`]).
///
/// CloudFormation does not leave such a resource half-changed: the configuration in force
/// before the deployment is declared again in the service that owns it. WHOSE work that is
/// is what the variants separate, because that follows from what becomes of the stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FailedResourceRevert {
    /// Not this deployment's work. Either the stack ROLLS BACK, and that rollback is what
    /// declares the previous configuration again in the owning services
    /// ([`StackProvenance::reapplied`] carries the resources it has to ask for) — so putting
    /// the resource back here would make that call twice and report it before the rollback it
    /// belongs to; or this deployment IS the putting back (a rollback, a resumption, a
    /// revert); or it is a CREATE, where a resource that failed has no earlier configuration
    /// of its own to go back to.
    #[default]
    None,
    /// The revert is the RESOURCE's alone: the stack rests in the failed status
    /// (`DisableRollback`), so there is no rollback of the stack's own to report it and the
    /// resource's `UPDATE_ROLLBACK_IN_PROGRESS`/`UPDATE_ROLLBACK_COMPLETE` transitions are
    /// what say it was put back.
    OfTheResourceAlone,
}

impl StackProvenance {
    /// The tags CloudFormation applies to the resource `logical_id` names: the stack's own,
    /// then the three CloudFormation records itself — which are CloudFormation's to set, so
    /// a stack tag spelled with one of those reserved keys does not displace them.
    ///
    /// A tag with an empty key is not a tag and is left out, and a key stated twice keeps
    /// its first position with the later value, so what reaches an owning service is a
    /// well-formed tag set however the stack itself was tagged.
    pub fn tags_for(&self, logical_id: &str) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = Vec::new();
        let mut put = |key: &str, value: &str| {
            if key.is_empty() {
                return;
            }
            match tags
                .iter_mut()
                .find(|(existing, _)| existing.as_str() == key)
            {
                Some((_, held)) => *held = value.to_string(),
                None => tags.push((key.to_string(), value.to_string())),
            }
        };
        for (key, value) in &self.tags {
            put(key, value);
        }
        put(STACK_NAME_TAG, &self.stack_name);
        put(STACK_ID_TAG, &self.stack_id);
        put(LOGICAL_ID_TAG, logical_id);
        tags
    }
}

/// One transition a stack operation made ONE of its resources go through, as
/// CloudFormation records a `StackEvent` for it.
///
/// A stack operation is not one transition but a walk over the template's resources, and
/// CloudFormation records the walk: an `*_IN_PROGRESS` event when it starts acting on a
/// resource, a second one that says the create was initiated in the service that owns it,
/// and a `*_COMPLETE` or `*_FAILED` event when that service has answered.
/// That is what names the resource a deployment failed on, and what carries the owning
/// service's own refusal ([`Self::reason`]) — neither of which the STACK's own events can
/// say, since they describe the operation as a whole.
///
/// WHEN each of those happened is not recorded here. The calls below run in-process, one
/// after another, so the wall clock while they return says nothing about the walk: the
/// instant a resource was scheduled and the instant it finished follow from the dependency
/// graph and from the signals a wait condition waited for, which is
/// [`super::timeline`]'s to compute.
///
/// Collected here rather than written straight to the store because the calls that make a
/// resource exist run with the store UNLOCKED: the operation that drove them records them
/// on the stack ([`super::StoredStack::schedule_resource_events`]) when it takes the lock
/// again.
#[derive(Debug, Clone)]
pub struct ResourceTransition {
    /// `StackEvent$LogicalResourceId`: the name the template gives the resource.
    pub logical_id: String,
    /// `StackEvent$ResourceType`.
    pub resource_type: String,
    /// `StackEvent$PhysicalResourceId`, which a resource nothing has created yet — and one
    /// whose create was refused — does not have.
    pub physical_resource_id: Option<String>,
    /// `StackEvent$ResourceStatus`, from the model's `ResourceStatus` enum
    /// ([`super::resource_status`]).
    pub status: String,
    /// `StackEvent$ResourceStatusReason`: "Success/failure message associated with the
    /// resource" — the owning service's refusal for a failed transition, and nothing for
    /// one that succeeded.
    pub reason: Option<String>,
    /// `StackEvent$ResourceProperties`: "BLOB of the properties used to create the
    /// resource" — the properties document the template declares for it, which is what the
    /// deployment asked the owning service for.
    pub resource_properties: Option<String>,
}

impl ResourceTransition {
    /// The transition a resource is STARTING: `*_IN_PROGRESS`, reporting the identifier the
    /// resource has at that point — the one it already carries for a change to, or the
    /// removal of, a resource that exists.
    ///
    /// The walk opens a transition this way only for a resource that IS there: a create's own
    /// opening transition names no instance yet ([`Self::creating`]).
    fn starting(resource: &StoredResource, status: &str) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: super::physical_resource_id(resource),
            status: status.to_string(),
            reason: None,
            resource_properties: resource.properties_blob(),
        }
    }

    /// The transition that OPENS a resource's create before anything exists: `status` is
    /// `CREATE_IN_PROGRESS` for a resource the stack does not have, and the UPDATE's own
    /// in-progress status for one this deployment REPLACES ([`Replacing`]), with that
    /// replacement's `reason` beside it.
    ///
    /// `StackEvent$PhysicalResourceId` is "the name or unique identifier associated with
    /// the physical instance of the resource", and there is no such instance yet, so this
    /// event names an EMPTY one — the member is reported, and what it reports is that
    /// nothing has been created under it so far.
    fn creating(resource: &StoredResource, status: &str, reason: Option<&'static str>) -> Self {
        Self {
            physical_resource_id: Some(String::new()),
            reason: reason.map(ToString::to_string),
            ..Self::starting(resource, status)
        }
    }

    /// The transition that reports the create of a resource was INITIATED: the second
    /// in-progress event CloudFormation records for it, once the request to create it
    /// has gone to the service that owns it, naming the identifier it is being created
    /// under. `status` is the one the opening transition carried.
    fn initiated(resource: &StoredResource, status: &str) -> Self {
        Self {
            reason: Some(super::RESOURCE_CREATION_INITIATED.to_string()),
            ..Self::finished(resource, status, None)
        }
    }

    /// The transition a resource FINISHED: the status it reached, the identifier it carries
    /// now, and the reason a failure came with.
    fn finished(resource: &StoredResource, status: &str, reason: Option<String>) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: super::physical_resource_id(resource),
            status: status.to_string(),
            reason,
            resource_properties: resource.properties_blob(),
        }
    }

    /// The transition a resource failed: as [`Self::finished`], with no identifier —
    /// nothing was created, so there is no physical id to report.
    fn failed(resource: &StoredResource, status: &str, reason: String) -> Self {
        Self {
            physical_resource_id: None,
            ..Self::finished(resource, status, Some(reason))
        }
    }

    /// The same transition, carrying the sentence CloudFormation words for what it is DOING
    /// — as distinct from a refusal, which is the reason a failed transition carries.
    ///
    /// `StackEvent$ResourceStatusReason` is "Success/failure message associated with the
    /// resource", and the events that report a resource being put back carry such a message
    /// on both halves of the pair ([`failed_resource`]).
    fn because(mut self, reason: &str) -> Self {
        self.reason = Some(reason.to_string());
        self
    }

    /// The same transition, reporting the properties the deployment ASKED the owning service
    /// for rather than the ones the resource carries now.
    ///
    /// `StackEvent$ResourceProperties` is "BLOB of the properties used to create the
    /// resource": what each event reports is the declaration THIS transition applied. For a
    /// resource the stack already has, the record carries the configuration really in force
    /// until the owning service has taken the new one ([`carry_forward`]) — so a transition of
    /// a change in flight, and the failed transition of a change that service refused, would
    /// otherwise report the document the deployment was replacing instead of the one it
    /// submitted, and a client reading the event stream of a failed update could not see what
    /// was asked for. The revert of such a change reports the configuration IT applies, which
    /// is the one in force, so nothing there goes through here.
    fn applying(mut self, properties: &::serde_json::Value) -> Self {
        self.resource_properties = super::template::properties_blob(properties);
        self
    }
}

/// The statuses one resource's transitions are reported under in the walk that CREATES it,
/// the verb its failure is summarised with, and the reason its opening transition carries.
///
/// A resource the stack does not have is CREATED, and its events are the `CREATE_*` ones. A
/// resource the stack HAS, under an identity this deployment no longer asks for, is being
/// REPLACED: CloudFormation creates the new physical resource as part of the update that
/// asked for it, so the events are that UPDATE's own and the one that opens them says why
/// the resource is being created at all ([`super::RESOURCE_REPLACEMENT_IN_PROGRESS`]) — which
/// is what tells a client polling the stack that the resource it had was replaced rather than
/// created for the first time. The old physical resource's removal is reported separately,
/// when the update's cleanup takes it down.
#[derive(Debug, Clone, Copy)]
struct Reporting {
    /// The status the resource's opening and initiated transitions carry.
    in_progress: &'static str,
    /// The status it reaches once the owning service has created it.
    completed: &'static str,
    /// The status it reaches when nothing created it.
    failed: &'static str,
    /// The verb the stack-level summary names this resource's failure under
    /// ([`super::resource_failure_summary`]).
    action: &'static str,
    /// The reason the opening transition carries, which only a replacement has.
    opening_reason: Option<&'static str>,
}

impl Reporting {
    /// How a resource of the creating walk is reported: `replacing` is whether the stack
    /// already has this logical resource under an identity this declaration no longer asks
    /// for, and `express` the deployment mode a create's verb depends on
    /// ([`super::create_action`]).
    fn of(replacing: bool, express: bool) -> Self {
        if replacing {
            Self {
                in_progress: super::resource_status::UPDATE_IN_PROGRESS,
                completed: super::resource_status::UPDATE_COMPLETE,
                failed: super::resource_status::UPDATE_FAILED,
                action: super::failed_action::UPDATE,
                opening_reason: Some(super::RESOURCE_REPLACEMENT_IN_PROGRESS),
            }
        } else {
            Self {
                in_progress: super::resource_status::CREATE_IN_PROGRESS,
                completed: super::resource_status::CREATE_COMPLETE,
                failed: super::resource_status::CREATE_FAILED,
                action: super::create_action(express),
                opening_reason: None,
            }
        }
    }
}

/// What deploying a stack's template left its resources as.
#[derive(Debug, Clone, Default)]
pub struct Materialised {
    /// The stack's resources, in the order the template declares them: each one as
    /// [`super::template::provision`] computed it, with `materialised` set and any
    /// attribute the owning service reported for it applied.
    pub resources: Vec<StoredResource>,
    /// The reason the stack cannot reach its completed status: an owning service refused
    /// to create one of the resources, or to delete one the deployment replaced, or
    /// CloudFormation itself refused the deployment before any service was asked anything.
    /// `None` when every resource this emulator provisions was created.
    ///
    /// This is the STACK-level reason, and WHOSE failure it was decides what it says. A
    /// resource an owning service refused is reported as the summary of which logical
    /// resources failed ([`super::resource_failure_summary`]), with why each one failed on
    /// that resource's own transition ([`ResourceTransition::reason`]). A deployment
    /// CLOUDFORMATION refused — a dynamic reference it could not resolve
    /// ([`super::dynamic_references::resolve_declared`]) — is its own diagnostic: no
    /// resource was asked of anybody, so there is no resource whose failure it could be
    /// summarising.
    pub failure: Option<String>,
    /// Whether that failure is one CLOUDFORMATION itself reached in the stage that resolves
    /// the declarations' dynamic references, before anything was asked of any owning service
    /// ([`super::dynamic_references::resolve_declared`]).
    ///
    /// It decides how the failure is REPORTED, which is why it travels beside the reason
    /// rather than being re-derived from its text: no resource of this deployment has an
    /// event that could carry a diagnostic about the template, so the stack's own failed
    /// event is the one report of it
    /// ([`super::StoredStack::fail_transition_on_template`]) — the same report the stages
    /// that process the template and validate it before provisioning already get. `false`
    /// for a failure the resources are what reached, whose own refusals are on those
    /// resources' events.
    pub diagnosed: bool,
    /// Every resource transition this deployment made, oldest first — the stack's
    /// per-resource event records.
    pub transitions: Vec<ResourceTransition>,
    /// The resources this deployment CREATED whose owning service does not report them
    /// ready yet, and the ones it DELETED whose owning service still holds them: the call
    /// was accepted and has not been carried out, so CloudFormation holds the resource — and
    /// the operation — open until that service says otherwise
    /// ([`super::stabilising`]).
    pub stabilising: Vec<Stabilising>,
    /// The resources whose configuration change the owning service was ASKED for and
    /// REFUSED, so the resource may be PART-CHANGED: a change reaches that service as
    /// several calls (a function's configuration and its deployment package are two), and a
    /// refusal part way through leaves the resource carrying some of the change while the
    /// stack's record still states what it had ([`carry_forward`]).
    ///
    /// What the ROLLBACK of this deployment has to declare again in those services whatever
    /// the records say ([`StackProvenance::reapplied`]). A resource this deployment never
    /// asked about — one whose type has no in-place update path, one whose declaration
    /// CloudFormation itself refused — cannot be part-changed and is not here: there is
    /// nothing in any service to put back for it.
    pub part_changed: BTreeSet<String>,
}

/// One resource whose creation an owning service accepted and has not finished, or whose
/// deletion it accepted and has not finished.
///
/// What the stack needs in order to go on asking that service about it: which resource it
/// is, which answer ends the wait, and the state that service reported when the call
/// answered.
#[derive(Debug, Clone)]
pub struct Stabilising {
    /// Which of that service's answers ends this wait
    /// ([`super::stabilising::Awaited`]): the resource it created is ready, the resource it
    /// changed is ready, or the resource it deleted is gone — which is also what says which
    /// action a wait that does not come through reports as failed.
    pub awaited: super::stabilising::Awaited,
    pub logical_id: String,
    pub resource_type: String,
    /// The identifier the owning service assigned, which the read that ends the wait
    /// addresses the resource by.
    pub physical_resource_id: String,
    /// That service's own word for the state the resource is in, as its API spells it.
    pub reported_state: String,
    /// The PROVIDER REQUEST this wait is for, where the resource is a CUSTOM RESOURCE
    /// ([`custom_resource`]): such a resource is backed by no API of this emulator's, so
    /// what ends its wait is the answer its provider PUTs to the request's `ResponseURL`
    /// rather than a read of an owning service. `None` for every other resource.
    pub provider_request: Option<custom_resource::ProviderRequest>,
}

/// What the service that owns a resource says about a resource it has CREATED: whether the
/// resource has reached the state that service reports it ready in.
///
/// Answered by the type's own arm ([`super::provisioner::Provisioner::readiness`]) from
/// that service's own read, so it is never a status this crate guessed. A type whose create
/// leaves the resource ready — which is most of them — answers [`Self::Ready`] without
/// asking anything.
#[derive(Debug, Clone)]
pub(super) enum Readiness {
    /// The resource is in the state that service reports it ready in, so its creation is
    /// finished.
    Ready,
    /// Not yet: the state that service reports the resource in right now.
    Pending(String),
    /// That service reports the resource FAILED: nothing is going to bring it up, so the
    /// resource's creation failed with this reason rather than waiting the window out.
    ///
    /// The reason is the PROVIDER's own sentence about the resource, as that service worded
    /// what it reported — never an envelope: the reporting site is what quotes it in the one
    /// envelope a provider's answer is reported in
    /// ([`handler_reported_failure_reason`]), so an arm that built one here would be
    /// quoted twice.
    Failed(String),
    /// The owning service REFUSED a call the provider made while bringing the resource up — an
    /// association a declared property asks for, which that service takes only once the
    /// resource is available — so the resource's creation failed on that refusal.
    ///
    /// The refusal travels AS the refusal rather than as a sentence, because what is reported
    /// for it is what is reported for a refused create: the message quoted in the provider
    /// envelope, under the `HandlerErrorCode` class THAT code falls in
    /// ([`handler_failure_reason`]) — which only the refusal itself can say.
    Refused(Refusal),
}

/// The `ResourceStatusReason` a readiness answer that FAILED the resource is recorded with.
///
/// The ONE place the two failure answers are turned into a reason, so the create path, the
/// update path and the stabilisation pass cannot word the same failure differently: the
/// provider's own report about the resource is quoted in the provider envelope
/// ([`handler_reported_failure_reason`]), and a call the owning service REFUSED is reported
/// exactly as a refused create is — the same envelope, under the `HandlerErrorCode` class that
/// refusal's own code falls in ([`handler_failure_reason`]).
///
/// `Ready` and `Pending` are no failure and carry no reason; every caller matches them ahead of
/// this, so neither reaches it.
fn provider_failure_reason(
    owner: Option<Owner>,
    physical_resource_id: &str,
    failure: &Readiness,
) -> String {
    match failure {
        Readiness::Failed(reason) => handler_reported_failure_reason(owner, reason),
        Readiness::Refused(refusal) => {
            handler_failure_reason(owner, refusal, physical_resource_id)
        }
        Readiness::Ready | Readiness::Pending(_) => String::new(),
    }
}

/// What the service that owns a resource says about a resource it has been asked to DELETE:
/// whether it still holds the resource.
///
/// Answered by the type's own arm ([`super::provisioner::Provisioner::removal`]) from that
/// service's own read, so it is never a status this crate guessed. A type whose delete takes
/// the resource away — which is most of them — answers [`Self::Gone`] without asking
/// anything.
#[derive(Debug, Clone)]
pub(super) enum Removal {
    /// That service no longer holds the resource, so its deletion is finished.
    Gone,
    /// Not yet: the state that service reports the resource being removed in right now.
    Pending(String),
}

/// What asking the service that OWNS a resource for that resource's configuration
/// established about it — the ACTUAL side of the comparison [`super::drift`] carries out.
///
/// The two ways a read answers with no configuration are different answers, and only one of
/// them says anything about the resource: a read that ANSWERED that the service no longer
/// holds the resource found it gone, while a type no configuration read is served for here
/// was never asked anything at all, so nothing about that resource was established and its
/// configuration is UNCHECKED. Collapsing the two would report a resource its owning service
/// still holds as deleted, and so its stack as drifted, on every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ActualConfiguration {
    /// The configuration that service reports the resource in now.
    Reported(Value),
    /// That service answered that it no longer holds the resource — judged on its own
    /// not-found code, never on a message.
    Gone,
    /// Nothing was asked of any service: no read of this type's configuration is served
    /// here, so there is nothing to compare the declaration against.
    Unchecked,
}

impl ActualConfiguration {
    /// One owning-service READ as this answer, in the convention every per-type arm answers
    /// in ([`super::provisioner::Provisioner::actual_configuration`]): `None` is that
    /// service answering that it no longer holds the resource.
    pub(super) fn read(reported: Option<Value>) -> Self {
        match reported {
            Some(configuration) => Self::Reported(configuration),
            None => Self::Gone,
        }
    }
}

/// WHO owns a template resource type — the one question this algorithm asks before it
/// creates, updates, re-tags or deletes a resource, and the only thing it needs to know
/// about a type.
///
/// Most types' per-type half lives in [`super::provisioner`]'s own catalogue
/// ([`Self::Builtin`]). A type added since that file passed the length a change to this
/// repository may touch brings its own module instead ([`Self::LambdaFunction`], whose arm
/// is [`lambda_function`]), and this is where the two meet: every call below is the same
/// question put to whichever module owns the type, so the algorithm itself is unchanged by
/// where an arm lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    /// A type whose create, update and delete arms are entries of
    /// [`provisioner_for`](super::provisioner::provisioner_for)'s catalogue.
    Builtin(Provisioner),
    /// `AWS::DynamoDB::Table`, whose arms are [`dynamodb_table`]'s.
    DynamodbTable,
    /// `AWS::DynamoDB::GlobalTable`, whose arms are [`dynamodb_table::global_table`]'s: the
    /// same table with the replicas of its group, created by the same service's calls.
    DynamodbGlobalTable,
    /// One of the inline-policy types [`iam_policy`] embeds in IAM (`AWS::IAM::Policy` and
    /// the per-entity `AWS::IAM::RolePolicy`, `UserPolicy` and `GroupPolicy`). Which of them
    /// it is travels with the owner, so the algorithm resolves the type once.
    IamPolicy(iam_policy::Kind),
    /// `AWS::Lambda::Alias`, whose arms are [`lambda_alias`]'s.
    LambdaAlias,
    /// `AWS::Lambda::EventInvokeConfig`, whose arms are
    /// [`lambda_event_invoke_config`]'s.
    LambdaEventInvokeConfig,
    /// `AWS::Lambda::EventSourceMapping`, whose arms are
    /// [`lambda_event_source_mapping`]'s.
    LambdaEventSourceMapping,
    /// `AWS::Lambda::Function`, whose arms are [`lambda_function`]'s.
    LambdaFunction,
    /// `AWS::Lambda::LayerVersion`, whose arms are [`lambda_layer_version`]'s.
    LambdaLayerVersion,
    /// `AWS::Lambda::Permission`, whose arms are [`lambda_permission`]'s.
    LambdaPermission,
    /// `AWS::Lambda::Version`, whose arms are [`lambda_version`]'s.
    LambdaVersion,
    /// One of the three Amazon CloudWatch Logs types [`logs_resources`] creates: the log
    /// group, the stream inside one, and the metric filter on one. Which of them it is
    /// travels with the owner, so the algorithm resolves the type once.
    LogsResource(logs_resources::Kind),
    /// `AWS::IoT::Authorizer`, whose arms are [`iot_authorizer`]'s.
    IotAuthorizer,
    /// One of the two AWS IoT types [`iot_provisioning_resources`] creates: the
    /// fleet-provisioning template, and the role alias a device assumes credentials through.
    /// Which of them it is travels with the owner, so the algorithm resolves the type once.
    IotProvisioningResource(iot_provisioning_resources::Kind),
    /// `AWS::IoT::Thing`, whose arms are [`iot_thing`]'s: the record of the AWS IoT thing
    /// registry every later control-plane call about the device resolves by name.
    IotThing,
    /// `AWS::IoT::TopicRule`, whose arms are [`iot_topic_rule`]'s.
    IotTopicRule,
    /// One of the Amazon EC2 VPC-network types [`ec2_vpc_network`] creates: the route table,
    /// the subnet association that puts a subnet on one, the VPC endpoint reached through it,
    /// the internet and virtual private gateways, the attachment that puts one of those on a
    /// VPC, and the Elastic IP address. Which of them it is travels with the owner, so the
    /// algorithm resolves the type once and every call below is about that same resource.
    Ec2VpcNetwork(ec2_vpc_network::Kind),
    /// `AWS::ECS::TaskDefinition`, whose arms are [`ecs_task_definition`]'s: the revision
    /// Amazon ECS registers for the resource, whose ARN — revision and all — is the identity
    /// that service assigned it.
    EcsTaskDefinition,
    /// `AWS::EC2::Instance`, whose arms are [`ec2_instance`]'s: the instance Amazon EC2
    /// launches for the resource, which is a resource of that service's own rather than part
    /// of the VPC network above.
    Ec2Instance,
    /// `AWS::EC2::SpotFleet`, whose arms are [`ec2_spot_fleet`]'s: the Spot Fleet request
    /// Amazon EC2 makes for the resource, which holds the capacity the declaration asked for.
    Ec2SpotFleet,
    /// One of the three Amazon Cognito Identity types [`cognito_identity_pool`] creates: the
    /// identity pool, the role attachment that binds the pool's IAM roles to it, and the
    /// principal-tag attribute map of one of its providers. Which of them it is travels with
    /// the owner, as for the Amazon EC2 types above.
    CognitoIdentityPool(cognito_identity_pool::Kind),
    /// One of the three Amazon Cognito user-pool types [`cognito_user_pool`] creates: the
    /// pool, the app client that signs in against it, and the third-party identity provider
    /// it federates with. Which of them it is travels with the owner, as for the types above.
    CognitoUserPool(cognito_user_pool::Kind),
    /// `AWS::CloudFormation::Stack`, whose arms are [`nested_stack`]'s: the owning service is
    /// CloudFormation itself, so a nested stack is created by the same `CreateStack` a
    /// client calls.
    NestedStack,
    /// `AWS::ECR::PullThroughCacheRule`, whose arms are [`ecr_pull_through_cache_rule`]'s: the
    /// rule Amazon ECR's own `CreatePullThroughCacheRule` registers under the prefix the stack
    /// decided — the prefix this type's `Ref` returns — and `DeletePullThroughCacheRule` takes
    /// off, with `UpdatePullThroughCacheRule` applying the two properties a rule may take anew.
    EcrPullThroughCacheRule,
    /// `AWS::SecretsManager::RotationSchedule`, whose arms are
    /// [`secretsmanager_rotation_schedule`]'s.
    SecretsManagerRotationSchedule,
    /// `AWS::SecretsManager::ResourcePolicy`, whose arms are
    /// [`secretsmanager_resource_policy`]'s.
    SecretsManagerResourcePolicy,
    /// `AWS::SecretsManager::Secret`, whose arms are [`secretsmanager_secret`]'s.
    SecretsManagerSecret,
    /// `AWS::StepFunctions::StateMachine`, whose arms are
    /// [`step_functions_state_machine`]'s.
    StepFunctionsStateMachine,
    /// One of the two Step Functions types [`step_functions_versions_and_aliases`] creates
    /// beside a state machine: the published VERSION of one, and the ALIAS that routes traffic
    /// to versions. Which of them it is travels with the owner, as it does for the types
    /// above.
    StepFunctionsVersionOrAlias(step_functions_versions_and_aliases::Kind),
    /// One of the `AWS::ApiGateway::*` types of that service's REST control plane, which
    /// [`apigateway_rest_api`] creates in Amazon API Gateway: the api, a resource of it, a
    /// method on that resource, a deployment of them, the stage that serves it, and the four
    /// that govern who reaches that stage — an authorizer, an api key, a usage plan and the
    /// association that puts a key in a plan. Which of them it is travels with the owner, as
    /// it does for the Amazon EC2 types above.
    ApiGatewayRestApi(apigateway_rest_api::Kind),
    /// One of the five `AWS::ElasticLoadBalancingV2::*` types [`elastic_load_balancing`]
    /// creates in Elastic Load Balancing: the load balancer, a target group, a listener on
    /// the load balancer, a rule of that listener and the trust store its listeners
    /// authenticate client certificates against. Which of them it is travels with the
    /// owner, as it does for the types above.
    ElasticLoadBalancing(elastic_load_balancing::Kind),
    /// `AWS::ElasticLoadBalancingV2::ListenerCertificate`, whose arms are
    /// [`elbv2_listener_certificate`]'s: the resource is no resource of Elastic Load Balancing's
    /// own but one or more ENTRIES of the certificate list of the listener it names, which
    /// `AddListenerCertificates` adds and `RemoveListenerCertificates` removes.
    ElbListenerCertificate,
    /// One of the four `AWS::AppConfig::*` types [`appconfig_resources`] creates in AWS
    /// AppConfig: the application, an environment of it, a configuration profile of it and the
    /// deployment strategy a rollout follows. Which of them it is travels with the owner, as
    /// it does for the types above.
    AppConfigResource(appconfig_resources::Kind),
    /// One of the two `AWS::AppConfig::*` types [`appconfig_rollout`] creates in AWS AppConfig
    /// beyond that estate: the hosted configuration version a profile serves, and the deployment
    /// that rolls one of its versions into an environment. Which of them it is travels with the
    /// owner, as it does for the types above.
    AppConfigRollout(appconfig_rollout::Kind),
    /// One of the six `AWS::AppSync::*` types [`appsync_resources`] creates in AWS AppSync:
    /// the GraphQL API, the schema it serves, a data source it reads through, a resolver on
    /// one of its fields, a pipeline function and an API key. Which of them it is travels with
    /// the owner, as it does for the types above.
    AppSyncResource(appsync_resources::Kind),
    /// One of the two Amazon CloudFront types [`cloudfront_resources`] creates: the
    /// distribution, and the origin access control its origins reach a private bucket with.
    /// Which of them it is travels with the owner, as it does for the types above.
    CloudFrontResource(cloudfront_resources::Kind),
    /// One of the two `AWS::ApplicationAutoScaling::*` types
    /// [`application_auto_scaling`] registers in Application Auto Scaling: the scalable
    /// target, and the scaling policy that steers one. Which of them it is travels with the
    /// owner, as it does for the types above.
    ApplicationAutoScaling(application_auto_scaling::Kind),
    /// One of the two AppSync custom-domain types [`appsync_domain_names`] creates in
    /// AppSync: the custom domain, and the mapping that makes a declared API answer on it.
    /// Which of them it is travels with the owner, as it does for the types above.
    AppSyncDomainName(appsync_domain_names::Kind),
    /// One of the three `AWS::AppSync::*` types [`appsync_event_and_merged_apis`] creates in
    /// AppSync beyond a GraphQL API's own estate: the Event API, a channel namespace of one,
    /// and the link of a source API into a Merged API. Which of them it is travels with the
    /// owner, as it does for the types above.
    AppSyncEventOrMergedApi(appsync_event_and_merged_apis::Kind),
    /// One of the five `AWS::ServiceDiscovery::*` types [`service_discovery`] creates in AWS
    /// Cloud Map: an HTTP, a public DNS or a private DNS namespace, a service of one, and an
    /// instance registered in a service. Which of them it is travels with the owner, as it
    /// does for the types above.
    ServiceDiscovery(service_discovery::Kind),
    /// One of the four Amazon Athena types [`athena_resources`] creates: the workgroup a
    /// stack's queries run in, the data catalog they are routed through, the query it saves
    /// in that workgroup and the capacity reservation they run on. Which of them it is
    /// travels with the owner, as it does for the types above.
    Athena(athena_resources::Kind),
    /// One of the three `AWS::MediaConvert::*` types [`mediaconvert_resources`] creates in AWS
    /// Elemental MediaConvert: the queue a stack's jobs run in, the job template they are
    /// created from, and the preset an output is written with. Which of them it is travels with
    /// the owner, as it does for the types above.
    MediaConvert(mediaconvert_resources::Kind),
    /// `AWS::SSM::PatchBaseline`, whose arms are [`ssm_patch_baseline`]'s: the patch baseline
    /// Systems Manager mints for the resource, together with the patch groups the declaration
    /// registers it for.
    SsmPatchBaseline,
    /// One of the three `AWS::SageMaker::*` Studio types [`sagemaker_studio`] creates in the
    /// Amazon SageMaker AI control plane: the domain, a user profile of one, and a shared space
    /// in one. Which of them it is travels with the owner, so the algorithm resolves the type
    /// once.
    SageMakerStudio(sagemaker_studio::Kind),
    /// One of the three `AWS::EFS::*` types [`efs_resources`] creates in Amazon EFS: the file
    /// system, an access point of one, and the mount target a VPC's subnet reaches it through.
    /// Which of them it is travels with the owner, so the algorithm resolves the type once.
    EfsResource(efs_resources::Kind),
}

/// The owner of a template resource type, or `None` for a type this emulator leaves
/// declared-only.
pub(super) fn owner_for(resource_type: &str) -> Option<Owner> {
    if resource_type == dynamodb_table::DYNAMODB_TABLE {
        return Some(Owner::DynamodbTable);
    }
    if resource_type == dynamodb_table::global_table::DYNAMODB_GLOBAL_TABLE {
        return Some(Owner::DynamodbGlobalTable);
    }
    if let Some(kind) = iam_policy::kind_of(resource_type) {
        return Some(Owner::IamPolicy(kind));
    }
    if resource_type == lambda_alias::LAMBDA_ALIAS {
        return Some(Owner::LambdaAlias);
    }
    if resource_type == lambda_event_invoke_config::LAMBDA_EVENT_INVOKE_CONFIG {
        return Some(Owner::LambdaEventInvokeConfig);
    }
    if resource_type == lambda_event_source_mapping::LAMBDA_EVENT_SOURCE_MAPPING {
        return Some(Owner::LambdaEventSourceMapping);
    }
    if resource_type == lambda_function::LAMBDA_FUNCTION {
        return Some(Owner::LambdaFunction);
    }
    if resource_type == lambda_layer_version::LAMBDA_LAYER_VERSION {
        return Some(Owner::LambdaLayerVersion);
    }
    if resource_type == lambda_permission::LAMBDA_PERMISSION {
        return Some(Owner::LambdaPermission);
    }
    if resource_type == lambda_version::LAMBDA_VERSION {
        return Some(Owner::LambdaVersion);
    }
    if let Some(kind) = logs_resources::kind_of(resource_type) {
        return Some(Owner::LogsResource(kind));
    }
    if resource_type == iot_authorizer::IOT_AUTHORIZER {
        return Some(Owner::IotAuthorizer);
    }
    if let Some(kind) = iot_provisioning_resources::kind_of(resource_type) {
        return Some(Owner::IotProvisioningResource(kind));
    }
    if resource_type == iot_thing::IOT_THING {
        return Some(Owner::IotThing);
    }
    if resource_type == iot_topic_rule::IOT_TOPIC_RULE {
        return Some(Owner::IotTopicRule);
    }
    if let Some(kind) = ec2_vpc_network::kind_of(resource_type) {
        return Some(Owner::Ec2VpcNetwork(kind));
    }
    if resource_type == ec2_instance::EC2_INSTANCE {
        return Some(Owner::Ec2Instance);
    }
    if resource_type == ec2_spot_fleet::EC2_SPOT_FLEET {
        return Some(Owner::Ec2SpotFleet);
    }
    if resource_type == super::template::ECS_TASK_DEFINITION {
        return Some(Owner::EcsTaskDefinition);
    }
    if let Some(kind) = cognito_identity_pool::kind_of(resource_type) {
        return Some(Owner::CognitoIdentityPool(kind));
    }
    if let Some(kind) = cognito_user_pool::kind_of(resource_type) {
        return Some(Owner::CognitoUserPool(kind));
    }
    if resource_type == nested_stack::CLOUDFORMATION_STACK {
        return Some(Owner::NestedStack);
    }
    if resource_type == ecr_pull_through_cache_rule::ECR_PULL_THROUGH_CACHE_RULE {
        return Some(Owner::EcrPullThroughCacheRule);
    }
    if resource_type == secretsmanager_rotation_schedule::SECRETSMANAGER_ROTATION_SCHEDULE {
        return Some(Owner::SecretsManagerRotationSchedule);
    }
    if resource_type == secretsmanager_resource_policy::SECRETSMANAGER_RESOURCE_POLICY {
        return Some(Owner::SecretsManagerResourcePolicy);
    }
    if resource_type == secretsmanager_secret::SECRETSMANAGER_SECRET {
        return Some(Owner::SecretsManagerSecret);
    }
    if resource_type == step_functions_state_machine::STEP_FUNCTIONS_STATE_MACHINE {
        return Some(Owner::StepFunctionsStateMachine);
    }
    if let Some(kind) = step_functions_versions_and_aliases::kind_of(resource_type) {
        return Some(Owner::StepFunctionsVersionOrAlias(kind));
    }
    if let Some(kind) = apigateway_rest_api::kind_of(resource_type) {
        return Some(Owner::ApiGatewayRestApi(kind));
    }
    if let Some(kind) = application_auto_scaling::kind_of(resource_type) {
        return Some(Owner::ApplicationAutoScaling(kind));
    }
    if let Some(kind) = appconfig_resources::kind_of(resource_type) {
        return Some(Owner::AppConfigResource(kind));
    }
    if let Some(kind) = appconfig_rollout::kind_of(resource_type) {
        return Some(Owner::AppConfigRollout(kind));
    }
    if let Some(kind) = appsync_resources::kind_of(resource_type) {
        return Some(Owner::AppSyncResource(kind));
    }
    if let Some(kind) = elastic_load_balancing::kind_of(resource_type) {
        return Some(Owner::ElasticLoadBalancing(kind));
    }
    if resource_type == elbv2_listener_certificate::ELB_LISTENER_CERTIFICATE {
        return Some(Owner::ElbListenerCertificate);
    }
    if let Some(kind) = cloudfront_resources::kind_of(resource_type) {
        return Some(Owner::CloudFrontResource(kind));
    }
    if let Some(kind) = appsync_domain_names::kind_of(resource_type) {
        return Some(Owner::AppSyncDomainName(kind));
    }
    if let Some(kind) = appsync_event_and_merged_apis::kind_of(resource_type) {
        return Some(Owner::AppSyncEventOrMergedApi(kind));
    }
    if let Some(kind) = service_discovery::kind_of(resource_type) {
        return Some(Owner::ServiceDiscovery(kind));
    }
    if let Some(kind) = athena_resources::kind_of(resource_type) {
        return Some(Owner::Athena(kind));
    }
    if let Some(kind) = mediaconvert_resources::kind_of(resource_type) {
        return Some(Owner::MediaConvert(kind));
    }
    if resource_type == ssm_patch_baseline::SSM_PATCH_BASELINE {
        return Some(Owner::SsmPatchBaseline);
    }
    if let Some(kind) = efs_resources::kind_of(resource_type) {
        return Some(Owner::EfsResource(kind));
    }
    if let Some(kind) = sagemaker_studio::kind_of(resource_type) {
        return Some(Owner::SageMakerStudio(kind));
    }
    provisioner_for(resource_type).map(Owner::Builtin)
}

/// Ask the services that own a Region's still-creating and still-deleting resources whether
/// they are done yet, and record what they answer.
///
/// This is the other half of [`super::stabilising`]: a resource whose owning service
/// accepted the create without reporting the resource ready, or accepted the delete while
/// still holding the resource, holds its stack's operation open, and what ENDS that wait is
/// that service's own answer about the resource — which only a call to it can give. `settle`
/// cannot make that call (it runs under the store lock, and a read of this store cannot reach
/// another service), so the operations that REPORT a stack's progress ask here, before they
/// take the lock: a client polling a stack's resources is exactly the caller that needs the
/// answer.
///
/// What this records was owed at the instant the owning service reached it, whether or not
/// anybody called: the resource became ready, or went away, then, and a wait that has run out
/// ([`super::stabilising::StabilisationBarrier::has_expired`]) failed then. The one thing it
/// changes in another service is what that failure owes: a failed create is rolled back, so
/// the resources it had made are removed again — the same work
/// [`crate::ops::create_stack`]'s own failure path does, and for the same reason
/// [`super::settle_due_rollbacks`] performs a failed update's rollback here rather than under
/// the store lock. Every other call this makes is that service's own READ.
///
/// Only the stacks of the Region the request was signed for are advanced: that is the set
/// the operation calling this reports on, and the Region is what the credential scope of
/// every call made here names.
///
/// The other things an operation in flight can only learn by CALLING another service are
/// driven from here for the same reason: the waits a `AWS::CloudFormation::WaitCondition`
/// holds it at ([`settle_expired_waits`]) and the rollback its `RollbackConfiguration`'s
/// triggers force ([`super::rollback_triggers::settle_triggered_rollbacks`]).
pub async fn advance_stabilisations(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsCloudformationCloudformationState>,
) {
    let region = super::request_region(ctx.parts());
    advance_stabilisations_in(ctx, state, region.as_str()).await;
}

/// [`advance_stabilisations`] for one named Region, for a caller that has the Region to
/// advance rather than a request signed for it.
///
/// The Region is what every call this makes is scoped to and what selects the stacks to
/// advance, so a caller that is not serving a client's request — the task a landed provider
/// answer starts ([`super::provider_driver`]) — passes the Region the answer was about
/// instead of a context for it to be read off.
pub async fn advance_stabilisations_in(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    // WHICH stacks this pass is for: every stack of the Region for a client's own call, and
    // the ONE stack being read for a read THIS service made while provisioning a nested stack
    // — so a hierarchy settles from the bottom up inside one call, without each level asking
    // about every other level ([`progress_probe`]).
    let scope = progress_probe::Scope::of(ctx);
    // How this emulator is reached, kept for the work a provider's answer owes once it lands
    // out of band ([`super::provider_driver`]): the answer arrives at a plane that is not a
    // handler, so the handle a handler has is what that plane publishes the next request
    // through.
    state.lock().remember_provisioning_reach(ctx);
    // The signals the INSTANCES of a signalled resource send, which is what releases a wait
    // whose signals come from them ([`instance_signals`]): read before the waits that have run
    // out are failed, so a gate its instances have signalled is released rather than failed.
    //
    // The pass holds every call it makes in a box of its own, so what this function keeps on
    // the stack for it while the walks below run is the pass's own bookkeeping and not another
    // service's request chain.
    instance_signals::settle_instance_signals(ctx, state, region).await;
    // ...and the BATCHED replacement of an Auto Scaling group's instances an `UpdatePolicy`
    // asks an update for ([`rolling_update`]): read after the signals a batch waits for have
    // been recorded, so a batch its replacements have signalled is through in this pass, and
    // before the waits that have run out are failed.
    rolling_update::settle_rolling_updates(ctx, state, region).await;
    settle_expired_waits(ctx, state, region).await;
    // ...and the rollback the stack's own ROLLBACK TRIGGERS force: an alarm a monitored
    // operation's configuration names, read in CloudWatch before this handler takes the lock,
    // exactly as the waits below are ([`super::rollback_triggers`]).
    super::rollback_triggers::settle_triggered_rollbacks(ctx, state, region).await;
    // Which resources are still waiting, read under a lock of its own: the readiness calls
    // below reach other services' handlers, which take those services' locks, so a client
    // reading THIS service must not be blocked behind them.
    let waiting: Vec<(String, Vec<OpenWait>)> = {
        let store = state.lock();
        store
            .stacks
            .iter()
            .filter(|stack| stack.region == region && scope.covers(stack))
            .filter_map(|stack| {
                let open = stack.open_stabilisations();
                if open.is_empty() {
                    None
                } else {
                    // The properties each resource was DECLARED with travel with its
                    // barrier: the deployment in flight holds them for the resources it is
                    // acting on, and the stack's own records for the ones it already had.
                    // An arm whose readiness depends on what the declaration asked for needs
                    // them, and nothing else about the stack may be read once the lock is
                    // released.
                    let asked = open
                        .into_iter()
                        .map(|barrier| {
                            let declared = declared_properties(stack, &barrier.logical_id);
                            (barrier, declared)
                        })
                        .collect();
                    Some((stack.stack_id.clone(), asked))
                }
            })
            .collect()
    };
    for (stack_id, barriers) in waiting {
        for (barrier, declared) in barriers {
            // A resource whose creation has NOT STARTED is no question for anybody: what it
            // waits for is the resources it depends on, and the pass that creates it once
            // they have answered is [`deferred::resume`].
            if barrier.awaited == super::stabilising::Awaited::Deferred {
                continue;
            }
            // A CUSTOM RESOURCE's wait is no question for an owning service: the resource is
            // backed by the PROVIDER its `ServiceToken` names, and what ends the wait is the
            // answer that provider PUT to the request's `ResponseURL`, which the plane that
            // received it recorded on this barrier
            // ([`crate::service::custom_resource_plane`]). So nothing is asked of anybody
            // for it — the answer is either there or the wait holds.
            let provider_answer = barrier
                .provider_request
                .as_ref()
                .map(|request| (request.request_id.as_str(), barrier.provider_response.as_ref()));
            // WHICH of the owning service's answers ends this wait: that the resource it
            // created is ready, or that the resource it deleted is gone. A removal that is
            // still in flight holds the operation exactly as a creation that is still coming
            // up does, so the two answers are read as the one thing they both are — whether
            // the call that service accepted has been carried out.
            let readiness = match provider_answer {
                Some((_, Some(response))) if response.succeeded() => Ok(Readiness::Ready),
                // The provider says it did not carry the request out, so nothing is going to
                // bring the resource up — or take it away — and what CloudFormation reports
                // is the FAILED response it received from the resource, carrying the message
                // that response stated and the request it answered — or its own diagnostic of
                // a document it would not read
                // ([`custom_resource::ProviderResponse::reported_failure`]).
                Some((request_id, Some(response))) => {
                    Ok(Readiness::Failed(response.reported_failure(request_id)))
                }
                Some((_, None)) => Ok(Readiness::Pending(barrier.reported_state.clone())),
                None => match owner_for(&barrier.resource_type) {
                    Some(provisioner) => match barrier.awaited {
                        // A create's wait and a change's wait ask that service the one
                        // question: is the resource in the state it serves traffic in?
                        super::stabilising::Awaited::Ready
                        | super::stabilising::Awaited::Updated
                        // Unreachable: a wait for a creation that has not started is not
                        // asked of anybody ([`deferred`]) and left this loop above.
                        | super::stabilising::Awaited::Deferred => {
                            // BOXED, like the walk's own provider calls: the read is another
                            // service's whole handler, and this function holds its state while
                            // the passes below it run.
                            Box::pin(provisioner.readiness(
                                ctx,
                                region,
                                &barrier.physical_resource_id,
                                declared.as_ref(),
                            ))
                            .await
                        }
                        super::stabilising::Awaited::Gone => Box::pin(provisioner.removal(
                            ctx,
                            region,
                            &barrier.physical_resource_id,
                        ))
                        .await
                        .map(|removal| match removal {
                            Removal::Gone => Readiness::Ready,
                            Removal::Pending(state) => Readiness::Pending(state),
                        }),
                    },
                    // Nothing owns the type any more, so there is nothing to ask: the wait is
                    // over, because what opened it was that provisioner's answer.
                    None => Ok(Readiness::Ready),
                },
            };
            let now = SystemTime::now();
            // The identifiers the owning service publishes for a resource that has only NOW
            // come up — an endpoint a still-creating resource did not have — read before the
            // lock is taken, like every other call this module makes. Asked only where that
            // service has just reported the resource ready; a read it refuses leaves the
            // records as they stand, since one unanswered read is no evidence about the
            // resource.
            //
            // For a custom resource those identifiers are the `Data` its provider answered
            // with, which is exactly what `Fn::GetAtt` resolves against for the type: the
            // answer is already in hand, so there is nothing to read.
            let ready_attributes = if !matches!(readiness, Ok(Readiness::Ready))
                || !barrier.awaited.awaits_readiness()
            {
                BTreeMap::new()
            } else if let Some((_, Some(response))) = provider_answer {
                response.data.clone()
            } else {
                match owner_for(&barrier.resource_type) {
                    Some(provisioner) => Box::pin(provisioner.ready_attributes(
                        ctx,
                        region,
                        &barrier.resource_type,
                        &barrier.physical_resource_id,
                    ))
                    .await
                    .unwrap_or_default(),
                    None => BTreeMap::new(),
                }
            };
            // A provider that answered an `Update` with a DIFFERENT `PhysicalResourceId` has
            // REPLACED the resource: the new physical resource is the one the stack keeps,
            // and the OLD one is cleaned up by a further `Delete` carrying the old id and the
            // old properties. That request is sent here, with the lock released, exactly as
            // every other call this module makes is — and it opens a wait of its own, so the
            // operation is not reported complete while the replaced resource is still there.
            //
            // A cleanup the provider's own service REFUSED opens no wait: nothing is going to
            // answer a request that was not delivered. The update itself has already been
            // applied, so what that refusal costs is the removal of the resource the update
            // replaced — reported as that resource's own failed removal (`Err` below), never
            // dropped.
            let mut refused_cleanup: Option<(ReplacedResource, Refusal)> = None;
            let cleanup = match custom_resource_cleanup(&stack_id, &barrier, &readiness) {
                Some(call) => match custom_resource::send(ctx, region, &call.as_provider_call())
                    .await
                {
                    Ok(response_url) => Some(call.awaited_removal(&response_url)),
                    Err(refusal) => {
                        refused_cleanup = Some((call, refusal));
                        None
                    }
                },
                None => None,
            };
            // What a failure the answer makes due owes besides the stack's own failed
            // status: the resources a rolled-back CREATE has to remove again. Taken under
            // the lock and performed with it RELEASED, as every other call this module
            // makes is.
            let unwinding = {
                let mut store = state.lock();
                let Some(stack) = store.resolve_mut(region, &stack_id) else {
                    continue;
                };
                // What the PROVIDER of a custom resource answered is that resource's real
                // identity: the `PhysicalResourceId` it reports is what `Ref` resolves to
                // from here on, whatever identifier the naming rules had computed for the
                // record — and whichever `Status` the answer carried. A provider that could
                // not carry the request out still names the physical resource it is
                // responsible for, and that is the resource CloudFormation then addresses:
                // the `Delete` a rollback sends about it carries the id the provider chose,
                // never one of CloudFormation's own. An answer naming NO physical resource
                // changes nothing ([`super::StoredStack::adopt_provider_identity`]), which
                // is the case of a document this service did not read at all
                // ([`custom_resource::ProviderResponse::too_long`]).
                if barrier.awaited.awaits_readiness() {
                    if let Some(response) = barrier.provider_response.as_ref() {
                        stack.adopt_provider_identity(
                            &barrier.logical_id,
                            &response.physical_resource_id,
                        );
                    }
                }
                let taken = match readiness {
                    Ok(Readiness::Ready) => {
                        stack.record_ready_attributes(&barrier.logical_id, &ready_attributes);
                        // The cleanup of a replaced resource holds the operation exactly as
                        // the update itself did — recorded BEFORE this wait is released, so
                        // the operation's completion is never made due in between.
                        if let Some(cleanup) = cleanup {
                            stack.record_stabilisations(
                                ::std::slice::from_ref(&cleanup),
                                now,
                            );
                            stack.report_update_cleanup(now);
                        }
                        // ...and a cleanup the provider's service refused is the replaced
                        // resource's own failed removal: the stack records WHICH physical
                        // resource was left behind and why, rather than reporting the update
                        // complete with no trace of it.
                        if let Some((replaced, refusal)) = &refused_cleanup {
                            stack.record_failed_removal(
                                now,
                                &replaced.logical_id,
                                &replaced.resource_type,
                                &replaced.physical_resource_id,
                                &handler_failure_reason(
                                    None,
                                    refusal,
                                    &replaced.physical_resource_id,
                                ),
                            );
                        }
                        stack.release_stabilisation(&barrier, now);
                        None
                    }
                    // Still coming up, or still being removed: the wait holds, unless it has
                    // run out — which is the instant the call the wait was for failed.
                    Ok(Readiness::Pending(state)) => {
                        if barrier.has_expired(now) {
                            stack.fail_stabilisation(
                                now,
                                &barrier,
                                &barrier.not_stabilised_reason(&state),
                            )
                        } else {
                            None
                        }
                    }
                    // That service says the resource will not come up, so waiting the window
                    // out would report a resource as creating that nothing is creating —
                    // whether what it reported is its own account of the resource or a call it
                    // refused while bringing the resource up.
                    Ok(ref failure @ (Readiness::Failed(_) | Readiness::Refused(_))) => stack
                        .fail_stabilisation(
                            now,
                            &barrier,
                            &provider_failure_reason(
                                owner_for(&barrier.resource_type),
                                &barrier.physical_resource_id,
                                failure,
                            ),
                        ),
                    // The service could not be asked at all. One unanswered read is not
                    // evidence about the resource — a service is allowed to throttle or be
                    // briefly unreachable — so the wait holds and the next read asks again;
                    // only the window running out ends it.
                    Err(refusal) => {
                        if barrier.has_expired(now) {
                            stack.fail_stabilisation(
                                now,
                                &barrier,
                                &handler_failure_reason(
                                    owner_for(&barrier.resource_type),
                                    &refusal,
                                    &barrier.physical_resource_id,
                                ),
                            )
                        } else {
                            None
                        }
                    }
                };
                // The records the stack holds are what the rollback takes down and reports
                // ([`unwind`]), read off the stack under the same lock the resources were —
                // with what that rollback does with their `DeletionPolicy`.
                taken.map(|resources| {
                    (
                        resources,
                        stack.held_resource_ids(),
                        dependencies_in_flight(stack),
                        stack.rolled_back_resource_policies(),
                    )
                })
            };
            // "ROLLBACK — Roll back the stack creation. This is the default": the resources
            // the failed create had already made are removed again in the services that own
            // them, exactly as `crate::ops::create_stack`'s own failure path removes them,
            // and only a delete one of those services REFUSES changes the status the
            // rollback settles into.
            let Some((resources, held, needs, policies)) = unwinding else {
                continue;
            };
            let deleted = unwind(
                ctx,
                region,
                &resources,
                &Teardown {
                    stack_id: &stack_id,
                    held: &held,
                    needs: &needs,
                    on_refusal: OnRefusal::Stop,
                    policies,
                    left_alone: NOTHING_LEFT_ALONE,
                },
            )
            .await;
            let mut store = state.lock();
            let Some(stack) = store.resolve_mut(region, &stack_id) else {
                continue;
            };
            stack.schedule_resource_events(&deleted.transitions);
            stack.record_materialised_resources(deleted.resources);
            // A resource whose owning service accepted the delete and still holds it holds
            // this rollback open, exactly as the resource that failed to come up held the
            // operation this rollback follows.
            stack.record_stabilisations(&deleted.removing, SystemTime::now());
            if let Some(failure) = deleted.failure {
                // Reported once this teardown has asked about every resource it still can
                // ([`super::StoredStack::hold_teardown_failure`]).
                if !stack.hold_teardown_failure(
                    &deleted.transitions,
                    &deleted.removing,
                    &deleted.refused,
                    super::status::ROLLBACK_FAILED,
                    &failure,
                ) {
                    stack.fail_transition(
                        SystemTime::now(),
                        super::status::ROLLBACK_FAILED,
                        failure,
                    );
                }
            }
        }
    }
    // ...and the resources this operation WITHHELD because a resource they read had not
    // published its attributes yet: created now that those waits have answered
    // ([`deferred`]), which is what finishes the walk.
    //
    // BOXED: that pass runs the deployment walk again ([`reconcile`]), which is the deepest
    // work a READ of this service performs, so its whole state is held off this function's
    // own future rather than on the stack of the request that asked.
    Box::pin(deferred::resume(ctx, state, region, &scope)).await;
    // ...and the resources a TEARDOWN in flight left alone because something that depends on
    // them was still being removed: asked about now that those removals have answered
    // ([`outstanding_teardown`]), which is what finishes the teardown.
    //
    // BOXED for the same reason: the pass asks the services that own those resources.
    Box::pin(outstanding_teardown::settle_outstanding_teardowns(
        ctx, state, region,
    ))
    .await;
    // Last: the notifications the stack events OWE to the stack's `NotificationARNs`. Here
    // for the same reason everything above is — the publish was due the instant the event
    // was recorded, and it reaches another service, which nothing under this store's lock
    // can do ([`super::stack_notifications`]) — and last, so the events this call has just
    // made due are among the ones it publishes.
    super::stack_notifications::deliver(ctx, state, region).await;
    // ...and the status-change events those same stack events owe the account's default
    // EventBridge bus, for the same reason and in the same place: the publication was due the
    // instant the event was recorded, and it reaches another service
    // ([`super::stack_status_events`]).
    super::stack_status_events::deliver(ctx, state, region).await;
}

/// The properties the resource `logical_id` of `stack` was DECLARED with, or `None` where
/// neither the deployment in flight nor the stack's own records hold a record for it.
///
/// The deployment in flight is asked first: its records are the declaration the operation
/// holding the barrier is acting on, while the stack's own records are the declaration in
/// force before it. `None` is "not known here", which is not the same as "declared nothing" —
/// a reader of this holds to whatever the owning service reports instead.
fn declared_properties(stack: &super::StoredStack, logical_id: &str) -> Option<Value> {
    stack
        .pending_deployment
        .as_ref()
        .and_then(|pending| {
            pending
                .resources
                .iter()
                .find(|resource| resource.logical_id == logical_id)
        })
        .map(|resource| resource.properties.clone())
        .or_else(|| {
            stack
                .resources
                .iter()
                .find(|deployed| deployed.resource.logical_id == logical_id)
                .map(|deployed| deployed.resource.properties.clone())
        })
}

/// One resource of a stack whose wait is still open: the barrier that holds it, and the
/// properties its resource was DECLARED with where the records say what they are
/// ([`declared_properties`]).
type OpenWait = (super::stabilising::StabilisationBarrier, Option<Value>);

/// The resource an `Update` a provider answered REPLACED: what the cleanup `Delete` of the
/// old physical resource is sent with.
///
/// Owned rather than borrowed, because it outlives the barrier reading that produced it: the
/// request is sent with the store's lock released.
#[derive(Debug, Clone)]
struct ReplacedResource {
    /// The stack the resource belongs to, which the request names.
    stack_id: String,
    logical_id: String,
    resource_type: String,
    /// The physical resource the provider replaced — the id this `Delete` addresses.
    physical_resource_id: String,
    /// The properties that resource was declared with, which this `Delete` carries: the
    /// `OldResourceProperties` of the `Update` the provider replaced it in answer to.
    properties: Value,
    /// The `RequestId` the cleanup is answered under.
    request_id: String,
}

impl ReplacedResource {
    /// The `Delete` request this cleanup sends.
    fn as_provider_call(&self) -> custom_resource::ProviderCall<'_> {
        custom_resource::ProviderCall {
            request_type: custom_resource::request_type::DELETE,
            request_id: &self.request_id,
            stack_id: &self.stack_id,
            logical_resource_id: &self.logical_id,
            resource_type: &self.resource_type,
            properties: &self.properties,
            physical_resource_id: Some(&self.physical_resource_id),
            old_properties: None,
        }
    }

    /// The wait that cleanup opens: the replaced resource is gone when its provider answers,
    /// at the `ResponseURL` the `Delete` was published under.
    fn awaited_removal(&self, response_url: &str) -> Stabilising {
        Stabilising {
            awaited: super::stabilising::Awaited::Gone,
            logical_id: self.logical_id.clone(),
            resource_type: self.resource_type.clone(),
            physical_resource_id: self.physical_resource_id.clone(),
            reported_state: super::resource_status::DELETE_IN_PROGRESS.to_string(),
            provider_request: Some(self.as_provider_call().remembered(response_url)),
        }
    }
}

/// The cleanup `barrier`'s answer owes, or `None` where it owes none.
///
/// "If the custom resource provider returns a new `PhysicalResourceId`, CloudFormation
/// treats the update as a replacement" and sends a `Delete` for the resource that was
/// replaced; the SAME id is an in-place update and owes no `Delete` at all. Every other
/// wait — a create, a delete, a resource of any other type — owes none either.
fn custom_resource_cleanup(
    stack_id: &str,
    barrier: &super::stabilising::StabilisationBarrier,
    readiness: &Result<Readiness, Refusal>,
) -> Option<ReplacedResource> {
    if !matches!(readiness, Ok(Readiness::Ready)) || !barrier.awaited.awaits_readiness() {
        return None;
    }
    let request = barrier.provider_request.as_ref()?;
    if request.request_type != custom_resource::request_type::UPDATE {
        return None;
    }
    let replaced = request.physical_resource_id.as_deref()?;
    let response = barrier.provider_response.as_ref()?;
    if response.physical_resource_id == replaced {
        return None;
    }
    Some(ReplacedResource {
        stack_id: stack_id.to_string(),
        logical_id: barrier.logical_id.clone(),
        resource_type: barrier.resource_type.clone(),
        physical_resource_id: replaced.to_string(),
        // The OLD properties: what the resource that is being cleaned up was declared with,
        // which the `Update` carried as its `OldResourceProperties`.
        properties: request
            .old_properties
            .clone()
            .unwrap_or_else(|| request.properties.clone()),
        request_id: cleanup_request_id(stack_id, barrier, replaced),
    })
}

/// Read the values the service that owns `resource` publishes for it NOW THAT IT IS UP, and
/// merge them into the record the stack keeps.
///
/// Not every `Fn::GetAtt` return value is known when a create is accepted: some are what the
/// owning service fills in as it brings the resource up — the outputs the child of a nested
/// stack publishes, the addresses an instance is assigned, the host a load balancer serves
/// on. The pass that releases a resource's readiness wait reads them for exactly that reason
/// ([`advance_stabilisations_in`]), so a resource that service reports READY AS THE CALL
/// ANSWERS is read the same way: whether a resource was already up when its create answered
/// or came up a moment later is not what decides which of its values a template can read.
///
/// A read that service refuses leaves the record as it stands — one unanswered read is no
/// evidence about the resource — and the merge is the record's own
/// ([`super::StoredResource::publish_attributes`]), shared with the pass above, so the ceiling
/// every resource's attributes are held to lives in one place.
async fn merge_published_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &mut StoredResource,
) {
    let Some(provisioner) = owner_for(&resource.resource_type) else {
        return;
    };
    // BOXED, like every other call this walk makes: the read is another service's whole
    // handler.
    let Ok(published) = Box::pin(provisioner.ready_attributes(
        ctx,
        region,
        &resource.resource_type,
        &resource.physical_resource_id,
    ))
    .await
    else {
        return;
    };
    let _changed = resource.publish_attributes(&published);
}

/// The `RequestId` the cleanup `Delete` of ONE replaced resource is answered under.
///
/// Derived from what identifies that cleanup — the stack, the resource, the physical
/// resource being cleaned up and the request whose answer replaced it — rather than drawn
/// fresh, so two passes that read the same answered barrier build the SAME request: the
/// cleanup is owed once, and a provider that receives it twice is receiving one request
/// twice rather than two cleanups. It is what the wait for that cleanup is identified by
/// ([`super::stabilising::StabilisationBarrier::is_the_same_wait`]), so the stack holds one
/// wait for it however many passes read the answer.
///
/// The form is the 8-4-4-4-12 one every other `RequestId` this service hands out carries
/// ([`custom_resource::new_request_id`] mints a fresh one in the same form).
fn cleanup_request_id(
    stack_id: &str,
    barrier: &super::stabilising::StabilisationBarrier,
    replaced: &str,
) -> String {
    let answered = barrier
        .provider_request
        .as_ref()
        .map(|request| request.request_id.as_str())
        .unwrap_or_default();
    let digits = super::stable_identifier_digits(
        &format!(
            "custom-resource-cleanup|{stack_id}|{logical}|{replaced}|{answered}",
            logical = barrier.logical_id,
        ),
        32,
    );
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// Materialise the failures the WAIT CONDITIONS of this Region's stacks have run out on, and
/// perform the work each one owes the services that own those stacks' resources.
///
/// "If the wait condition doesn't receive the specified number of success signals before the
/// `Timeout` period expires, CloudFormation assumes that the wait condition has failed", and
/// a failed operation is rolled back: a failed CREATE's rollback removes what the create had
/// already made and a failed UPDATE's puts the previous stack configuration back
/// ([`super::wait_conditions::ExpiredWaitOwes`]). Both are CALLS to the services that own
/// those resources, which neither the store's own clock nor a read under its lock can make —
/// so they are made here, by the next operation of this service to be called, exactly as the
/// waits for a resource to come up above are read here.
///
/// What this records was owed at the instant the wait ran out, whether or not anybody called.
async fn settle_expired_waits(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    // The failures the clock has made due, and what each one owes — taken under the lock and
    // performed with it RELEASED, like every other call this module makes.
    let owed: Vec<(String, super::wait_conditions::ExpiredWaitOwes)> = {
        let mut store = state.lock();
        let now = SystemTime::now();
        store
            .stacks
            .iter_mut()
            .filter(|stack| stack.region == region)
            .filter_map(|stack| {
                let owes = stack.fail_expired_wait(now)?;
                Some((stack.stack_id.clone(), owes))
            })
            .collect()
    };
    for (stack_id, owes) in owed {
        // A failed UPDATE's rollback is the one path every rollback this service owes goes
        // through, which the operation calling this drives for itself
        // ([`super::settle_due_rollbacks`]).
        let super::wait_conditions::ExpiredWaitOwes::Removal {
            resources,
            held,
            needs,
            policies,
        } = owes
        else {
            continue;
        };
        // The removal's calls are made exactly as the ones the create's own failure path
        // makes them, and as the waits above are read: through this request's own context.
        let deleted = unwind(
            ctx,
            region,
            &resources,
            &Teardown {
                stack_id: &stack_id,
                held: &held,
                needs: &needs,
                on_refusal: OnRefusal::Stop,
                policies,
                left_alone: NOTHING_LEFT_ALONE,
            },
        )
        .await;
        let mut store = state.lock();
        let Some(stack) = store.resolve_mut(region, &stack_id) else {
            continue;
        };
        stack.schedule_resource_events(&deleted.transitions);
        stack.record_materialised_resources(deleted.resources);
        // A resource whose owning service accepted the delete and still holds it holds this
        // rollback open: it is not reported complete while a resource it removed is there.
        stack.record_stabilisations(&deleted.removing, SystemTime::now());
        if let Some(failure) = deleted.failure {
            // Reported once this teardown has asked about every resource it still can
            // ([`super::StoredStack::hold_teardown_failure`]).
            if !stack.hold_teardown_failure(
                &deleted.transitions,
                &deleted.removing,
                &deleted.refused,
                super::status::ROLLBACK_FAILED,
                &failure,
            ) {
                stack.fail_transition(SystemTime::now(), super::status::ROLLBACK_FAILED, failure);
            }
        }
    }
}

/// [`settle_expired_waits`] for a handler that is about to read or write the store: the
/// failures the wait conditions of this Region's stacks have run out on are materialised, and
/// the work each one owes the owning services performed, BEFORE that handler takes the lock.
///
/// Every operation of this service that settles the store calls this first, so the expiry is
/// concluded in ONE place — the one that can make those calls — whichever operation a client
/// happens to call after the wait ran out. Call it with the handler's `ctx` and
/// `&*self.engine`, holding no store lock.
pub async fn settle_expired_wait_failures<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let region = super::request_region(ctx.parts());
    settle_expired_waits(ctx, &state, region.as_str()).await;
}

/// Reconcile what a stack's newly deployed template declares with what the stack already
/// has: create what is new in the service that owns it, carry forward what is unchanged,
/// and delete what the deployment replaced or removed.
///
/// `region` is the stack's own Region: the resources live there, and it is what the
/// credential scope of every call this module makes names. `previous` is the set the
/// stack's last completed deployment left (empty for a create), which is what makes a
/// re-deploy of the same template a no-op rather than a second create — and what keeps a
/// resource another deployment created from being forgotten, so the stack's deletion still
/// deletes it.
///
/// Every dynamic reference the declarations carry whose store stands OUTSIDE the deployment
/// is resolved FIRST, by CloudFormation itself
/// ([`super::dynamic_references::resolve_declared`]): one it cannot honour refuses
/// the whole deployment before any owning service is asked anything, so nothing is created
/// and [`Materialised::failure`] is the resolver's own diagnostic rather than a summary of
/// resources that failed. A reference whose store can hold a value the same deployment
/// creates is resolved as the resource carrying it is deployed
/// ([`super::dynamic_references::resolve_as_deployed`]), so it fails that resource rather
/// than the deployment as a whole.
///
/// `refused` is a refusal CLOUDFORMATION itself reached before this walk — the deployment's
/// template imports an export nothing publishes
/// ([`super::StoredStack::unresolved_cross_stack_failure`]) — and it is answered exactly as the
/// gate above answers one of its own: the walk is not entered, nothing is asked of any
/// owning service, and [`Materialised::failure`] is that diagnostic. `None` is a deployment
/// CloudFormation has nothing against, which is every rollback and every revert.
///
/// The resources are created in the order the template declares them. The first refusal
/// from an owning service stops the walk and is reported as [`Materialised::failure`]: the
/// resources created before it are still reported (with their `materialised` flag set), so
/// the stack's deletion can clean them up, and the ones after it are reported as the
/// declaration left them.
///
/// A resource the stack already has and this deployment re-declares with DIFFERENT
/// properties is UPDATED IN PLACE, through the owning service's own update API
/// ([`super::provisioner::Provisioner::update`]): the resource keeps its identity, so
/// nothing is created and nothing is deleted, and the stack records the declared
/// configuration because that is the one now in force. Only a type with no update path
/// there, and a change the owning service refuses, leave the deployment with a failure —
/// never a success that dropped the caller's change.
///
/// `stack` is the stack the resources belong to: every resource this CREATES is created
/// carrying the tags CloudFormation gives it ([`StackProvenance::tags_for`]), so the
/// owning service's own tag read on the handle the stack publishes answers with the stack's
/// tags and the `aws:cloudformation:*` ones. A resource the deployment carries FORWARD is
/// brought back into step with that same set
/// ([`super::provisioner::Provisioner::retag`]), since a deployment can state the stack's
/// tags differently with no resource's declaration changing at all; nothing is called about
/// a resource whose tags already agree with it, and a resource whose configuration change
/// the owning service refused is not re-tagged either.
///
/// Every resource the walk ACTS ON is recorded as a pair of transitions
/// ([`Materialised::transitions`]) — the one that opens it and the one that ends it — which
/// is what lets the stack's event stream name the resource a deployment failed on and carry
/// the owning service's own refusal. A resource the deployment leaves exactly as it found it
/// transitions through nothing and is recorded nowhere, and neither is one the walk never
/// reached.
///
/// `removal_policies` is what this walk does with the `DeletionPolicy` of a resource it
/// REMOVES — one the stack has and this deployment no longer declares ([`Policies`]). An
/// ordinary deployment reads the attribute, which is where CloudFormation stops managing the
/// resource; a deployment that is a stack's ROLLBACK passes what the operation being rolled
/// back was asked for ([`super::StoredStack::rolled_back_resource_policies`]), since
/// `RetainExceptOnCreate` is documented to override that attribute for a resource the
/// operation itself created. The OLD physical resource of a replacement is governed by
/// `UpdateReplacePolicy` instead, so that walk reads no deletion policy either way.
pub async fn reconcile(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    stack: &StackProvenance,
    declared: Vec<StoredResource>,
    previous: &[StoredResource],
    refused: Option<String>,
    removal_policies: Policies,
) -> Materialised {
    let mut outcome = Materialised {
        // A deployment CLOUDFORMATION itself refused before this walk: nothing is asked of
        // any owning service for it, and every declaration is recorded as the declaration
        // left it by the tail below — the same treatment the gate just below gives a
        // refusal of its own.
        failure: refused,
        ..Materialised::default()
    };
    // The identifiers the owning services MINTED in this walk, each against the stand-in
    // the naming rules had computed for that resource ([`repoint_references`]).
    let mut minted: Vec<Minted> = Vec::new();
    // The resources of this walk whose own attributes are NOT PUBLISHED yet: a resource the
    // owning service is still bringing up, a custom resource held for its provider's answer,
    // and every resource withheld behind one of them ([`awaits_a_dependency`]).
    let mut unpublished: BTreeSet<String> = BTreeSet::new();
    // Every resource of this walk whose own provider REFUSED to carry the declaration out,
    // in the order the walk reached them, each against the verb its failure is summarised
    // under ([`failed_resource_summary`]).
    //
    // A deployment attempts every resource whose dependencies allow it, so one broken
    // resource does not stop the walk: each of them reports its own transitions, the ones
    // that depend on a failed resource are left alone ([`depends_on_a_failure`]), and the
    // stack-level reason names the whole set rather than the first of it.
    let mut failed: Vec<(String, &'static str)> = Vec::new();
    // The physical resources this walk took down BEFORE creating what replaced them, for a
    // type whose replacement cannot hold two resources at once
    // ([`super::template::replacement_deletes_first`]): each is gone already, so the cleanup
    // phase that takes down what this deployment replaced owes no call about it.
    let mut taken_down: BTreeSet<String> = BTreeSet::new();
    // The resources this walk has STARTED and not yet asked the owning service about: a
    // level of the stack's dependency relation is brought up TOGETHER, so the question
    // "does that service report this resource ready" is put where its answer is needed
    // rather than before the walk reaches the next declaration
    // ([`level_readiness::settle`]).
    let mut coming_up: Vec<level_readiness::ComingUp> = Vec::new();
    let mut declared = declared.into_iter();
    // Every `{{resolve:...}}` DYNAMIC REFERENCE the declarations carry whose store stands
    // OUTSIDE the deployment is resolved by CLOUDFORMATION ITSELF, here, before anything is
    // asked of any owning service: a reference no store can satisfy has no value, so there
    // is nothing to ask any service for. Such a reference is therefore CloudFormation's OWN
    // refusal of the deployment — no provider is called for ANY of the resources, and the
    // reason the stack reports is the resolver's diagnostic (which reference could not be
    // honoured, and why) rather than a summary of resources that tried and failed
    // ([`super::resource_failure_summary`]).
    //
    // A reference whose store can hold a value THIS deployment creates — a secret declared
    // beside the resource configured with it — is resolved further down instead, as the
    // resource carrying it is deployed.
    //
    // A deployment already refused above is not resolved against any store at all: that
    // refusal stands, and nothing is read for a deployment nothing will deploy.
    let mut resolutions = if outcome.failure.is_some() {
        BTreeMap::new()
    } else {
        match super::dynamic_references::resolve_declared(ctx, region, declared.as_slice())
            .await
        {
            Ok(resolutions) => resolutions,
            Err(reason) => {
                // CLOUDFORMATION's own refusal, reached before any owning service was asked
                // anything: the stack's own failed event is what reports it, since no
                // resource of this deployment has one.
                outcome.failure = Some(reason);
                outcome.diagnosed = true;
                BTreeMap::new()
            }
        }
    };
    // A deployment the gate above refused deploys nothing: the walk is not entered, and
    // every declaration is recorded as the declaration left it by the tail below.
    while outcome.failure.is_none() {
        let Some(mut resource) = declared.next() else {
            break;
        };
        // The resource's record carries the document the stack RECORDS — a secure
        // reference's value is never one of them — while the owning service is asked for
        // the resource with the resolved one.
        let resolved = resolutions
            .remove(&resource.logical_id)
            .unwrap_or_else(|| {
                super::dynamic_references::Resolution::verbatim(&resource.properties)
            });
        resource.properties = resolved.recorded;
        let mut deployed_properties = resolved.deployed;
        // A property of this resource naming one created EARLIER in this walk carries the
        // stand-in id that resource had when the declaration was resolved, which is not the
        // id its owning service went on to mint. The reference is re-pointed at the minted
        // one before anything is asked about this resource, so the create carries the
        // identifier its service knows the dependency by — and so the comparison below
        // reads an UNCHANGED declaration as unchanged. Both documents the resolution left
        // are re-pointed, since both name that dependency.
        //
        // Only with the pairs of the resources this one READS ([`pairs_for`]): a value that
        // merely coincides with another resource's stand-in is the value this declaration
        // itself asked for.
        let pairs = pairs_for(&resource.logical_id, &stack.dependencies, &minted);
        repoint_references(&mut resource.properties, &pairs);
        repoint_references(&mut deployed_properties, &pairs);
        // ...and so is the resource's OWN IDENTITY, for a type whose physical id is the NAME
        // its declaration states. A name the template COMPOSES out of a dependency's
        // identifier (`/aws/<service>/<kind>/${Api.ApiId}`) was computed before that
        // dependency's create had minted the identifier, so the name the naming rules
        // produced carries the stand-in. The resource has to be created — and reported — under
        // the name its declaration resolves to NOW: that is the name every other reader of
        // the same dependency builds, so a group created under the stand-in is one a child
        // resolved later (a resource withheld behind a dependency still coming up, an output,
        // a client's own call) addresses in vain.
        repoint_identity(&mut resource, &pairs);
        // THIS is where the level the walk has just started is asked about: the declaration
        // in hand READS a resource the walk started and has not had an answer about yet, and
        // "a resource that depends on another is scheduled only once that one has finished",
        // so the answers are needed now. Asking here rather than as each resource was
        // created is what makes the resources of one level come up TOGETHER — a level of
        // several costs about one coming-up window, not one each
        // ([`level_readiness::settle`]). Everything below judges the answers exactly as it
        // did when they were read inline: a resource that failed holds back what reads it,
        // and one still coming up is withheld behind a wait of its own.
        if !coming_up.is_empty()
            && awaits_a_dependency(&resource.logical_id, &stack.dependencies, &unpublished)
        {
            let reads = needed_by(&stack.dependencies, &resource.logical_id);
            Box::pin(level_readiness::settle(
                ctx,
                region,
                &mut coming_up,
                Some(&reads),
                &mut outcome,
                &mut failed,
                &mut unpublished,
            ))
            .await;
        }
        // A resource the walk could not carry out holds back every resource that DEPENDS on
        // it: what such a declaration reads is the resource that failed, so there is nothing
        // its owning service could be asked for, and CloudFormation records no transition for
        // a resource it never attempted. The record is the declaration — carried forward
        // where the stack already had the resource, which still exists in its owning service.
        if depends_on_a_failure(&resource.logical_id, &stack.dependencies, &failed) {
            if let Some(existing) = previous.iter().find(|existing| {
                already_deployed(existing) && is_same_resource(existing, &resource)
            }) {
                carry_forward(existing, &mut resource);
            }
            outcome.resources.push(resource);
            continue;
        }
        // A resource whose declaration READS something a resource it depends on has not
        // published yet is not acted on in this pass at all — whether this deployment would
        // create it or the stack already has it. A CUSTOM RESOURCE's attributes are the `Data`
        // its provider answers with, so a declaration naming one through `Fn::GetAtt` carries
        // no value until THAT request's answer is in hand: acting on it now would ask the
        // owning service for the previous answer's value, or for a document the template does
        // not describe. It is held at a wait of its own
        // ([`super::stabilising::Awaited::Deferred`]) and acted on by the pass that runs when
        // those waits release ([`deferred::resume`]) — which is the order CloudFormation
        // deploys a stack's resources in: "a resource that depends on another is scheduled only
        // once that one has finished".
        //
        // The same hold covers a declaration whose read had nothing to resolve against when
        // this deployment's documents were computed — an attribute a resource of this same
        // deployment only publishes once its owning service has created it, which the resolved
        // document therefore carries no member for at all
        // ([`StackProvenance::reads_unpublished_attributes`]). Such a resource is withheld even
        // where the resource it reads is up already, because what the walk holds for it is a
        // document resolved before that value existed.
        if awaits_a_dependency(&resource.logical_id, &stack.dependencies, &unpublished)
            || stack
                .reads_unpublished_attributes
                .contains(&resource.logical_id)
        {
            unpublished.insert(resource.logical_id.clone());
            // The record the stack already has, so a withheld resource keeps its identity and
            // what its owning service reported about it while it waits; the declaration it is
            // acted on with is resolved again by the pass that resumes it
            // ([`super::StoredStack::resolve_deferred_declarations`]).
            let held = previous
                .iter()
                .find(|existing| already_deployed(existing) && is_same_resource(existing, &resource));
            let reported_state = match held {
                Some(existing) => {
                    carry_forward(existing, &mut resource);
                    super::resource_status::UPDATE_IN_PROGRESS
                }
                None => super::resource_status::CREATE_IN_PROGRESS,
            };
            outcome.stabilising.push(Stabilising {
                awaited: super::stabilising::Awaited::Deferred,
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                physical_resource_id: resource.physical_resource_id.clone(),
                reported_state: reported_state.to_string(),
                provider_request: None,
            });
            outcome.resources.push(resource);
            continue;
        }
        if let Some(existing) = previous.iter().find(|existing| {
            already_deployed(existing) && is_the_declared_resource(existing, &resource)
        }) {
            // The stack already has exactly this resource and the deployment did not change
            // its identity, so nothing is created again; what the owning service reported
            // about it, and the configuration it really has, are carried forward.
            let declared_properties = resource.properties.clone();
            // What THIS declaration publishes for the resource: the attributes it states
            // itself ([`super::stack::published_from_a_declared_property`] is the one reading of
            // which those are), which are what a reference to the resource reads once the
            // owning service has applied the change below. Read before the record in force is
            // carried forward over them.
            let declared_attributes: ::std::collections::BTreeMap<String, String> = {
                let states_them = super::stack::published_from_a_declared_property(&resource);
                resource
                    .attributes
                    .iter()
                    .filter(|(name, _)| states_them.contains(name.as_str()))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            };
            // What the record states is not always what the owning service holds: a change
            // that service refused PART WAY left the resource carrying some of it, and the
            // deployment that puts such a resource back declares its configuration again
            // whatever the records say ([`StackProvenance::reapplied`]).
            let changed = declared_properties != existing.properties
                || stack.reapplied.contains(&resource.logical_id);
            carry_forward(existing, &mut resource);
            if changed {
                // ...but this deployment asks for a DIFFERENT configuration of a resource
                // that already exists. CloudFormation applies such a change THROUGH the
                // owning service's own update API, and the stack completes: the resource
                // keeps its identity, so nothing is created and nothing is deleted. The
                // call that applies it is the type's own entry
                // ([`super::provisioner::Provisioner::update`]); a type with no update
                // path there has nothing asked of its service, and the change is reported
                // as NOT applied rather than as a success that dropped the caller's
                // change.
                outcome.transitions.push(
                    ResourceTransition::starting(
                        &resource,
                        super::resource_status::UPDATE_IN_PROGRESS,
                    )
                    .applying(&declared_properties),
                );
                // A CUSTOM RESOURCE's changed declaration is applied by its PROVIDER, not by
                // an owning service: the `Update` request carries the resource's current
                // `PhysicalResourceId` and the `OldResourceProperties` it was last declared
                // with, and the resource is held until the provider answers. An answer naming
                // a DIFFERENT physical id has REPLACED the resource, whose cleanup `Delete`
                // of the old one is sent when that answer lands
                // ([`advance_stabilisations`]).
                if custom_resource::is_custom_resource(&resource.resource_type) {
                    // The same declaration rule the create is held to: a changed
                    // declaration whose `ServiceTimeout` is outside the documented range is
                    // refused before the provider is sent the `Update`.
                    if let Some(reason) =
                        custom_resource::service_timeout_refusal(&deployed_properties)
                    {
                        failed.push((
                            resource.logical_id.clone(),
                            super::failed_action::UPDATE,
                        ));
                        outcome.transitions.push(
                            ResourceTransition::finished(
                                &resource,
                                super::resource_status::UPDATE_FAILED,
                                Some(reason.to_string()),
                            )
                            .applying(&declared_properties),
                        );
                        outcome.resources.push(resource);
                        continue;
                    }
                    let request_id = custom_resource::new_request_id();
                    let call = custom_resource::ProviderCall {
                        request_type: custom_resource::request_type::UPDATE,
                        request_id: &request_id,
                        stack_id: &stack.stack_id,
                        logical_resource_id: &resource.logical_id,
                        resource_type: &resource.resource_type,
                        properties: &deployed_properties,
                        physical_resource_id: Some(&resource.physical_resource_id),
                        old_properties: Some(&existing.properties),
                    };
                    let sent = custom_resource::send(ctx, region, &call).await;
                    match sent {
                        Ok(response_url) => {
                            let request = call.remembered(&response_url);
                            // The configuration the provider was asked for is the DECLARED
                            // one, which is what the stack records — exactly as it records an
                            // owning service's applied change.
                            resource.properties = declared_properties.clone();
                            outcome.stabilising.push(Stabilising {
                                awaited: super::stabilising::Awaited::Updated,
                                logical_id: resource.logical_id.clone(),
                                resource_type: resource.resource_type.clone(),
                                physical_resource_id: resource.physical_resource_id.clone(),
                                reported_state: super::resource_status::UPDATE_IN_PROGRESS
                                    .to_string(),
                                provider_request: Some(request),
                            });
                            // The attributes in force are the `Data` the answer to THIS
                            // request will carry, so nothing that reads one of them is
                            // asked for with the previous answer's value
                            // ([`awaits_a_dependency`]).
                            unpublished.insert(resource.logical_id.clone());
                            outcome.transitions.push(ResourceTransition::finished(
                                &resource,
                                super::resource_status::UPDATE_COMPLETE,
                                None,
                            ));
                            outcome.resources.push(resource);
                            continue;
                        }
                        Err(refusal) => {
                            failed.push((
                                resource.logical_id.clone(),
                                super::failed_action::UPDATE,
                            ));
                            outcome.transitions.push(
                                ResourceTransition::finished(
                                    &resource,
                                    super::resource_status::UPDATE_FAILED,
                                    Some(handler_failure_reason(
                                        None,
                                        &refusal,
                                        &resource.physical_resource_id,
                                    )),
                                )
                                .applying(&declared_properties),
                            );
                            outcome.resources.push(resource);
                            continue;
                        }
                    }
                }
                // The dynamic references resolved as the resource is DEPLOYED, resolved
                // here: the document the owning service is asked to change the resource
                // with carries the value the store holds now, while the record the stack
                // keeps of it still carries the reference
                // ([`super::dynamic_references::resolve_as_deployed`]). A reference that
                // cannot be honoured leaves nothing to ask that service for, so the change
                // fails carrying the resolver's own diagnostic. After the custom-resource
                // arm above, and so never applied to one, for the reason the create's own
                // call records.
                if let Err(reason) = super::dynamic_references::resolve_as_deployed(
                    ctx,
                    region,
                    &resource.resource_type,
                    &mut deployed_properties,
                )
                .await
                {
                    failed.push((resource.logical_id.clone(), super::failed_action::UPDATE));
                    outcome.transitions.push(
                        ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(reason),
                        )
                        .applying(&declared_properties),
                    );
                    Box::pin(roll_back_failed_resource(
                        ctx,
                        region,
                        stack,
                        &mut resource,
                        existing,
                        &mut outcome,
                    ))
                    .await;
                    outcome.resources.push(resource);
                    continue;
                }
                // A changed declaration may no more claim a key in the reserved namespace
                // than the one that created the resource could: the change fails carrying the
                // same sentence and the owning service is not asked to apply it, for the
                // reason the create's own gate records
                // ([`super::stack_tags::declared_resource_tag_refusal`]).
                if let Some(reason) =
                    super::stack_tags::declared_resource_tag_refusal(&deployed_properties)
                {
                    failed.push((resource.logical_id.clone(), super::failed_action::UPDATE));
                    outcome.transitions.push(
                        ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(reason.to_string()),
                        )
                        .applying(&declared_properties),
                    );
                    Box::pin(roll_back_failed_resource(
                        ctx,
                        region,
                        stack,
                        &mut resource,
                        existing,
                        &mut outcome,
                    ))
                    .await;
                    outcome.resources.push(resource);
                    continue;
                }
                let applied = match owner_for(&resource.resource_type) {
                    Some(provisioner) => {
                        // A change that hands an IAM ROLE to this service is applied only if
                        // the principal that asked for the deployment may pass that role, for
                        // the reason the create's own gate records ([`super::pass_role`]).
                        match super::pass_role::refusal(&deployed_properties) {
                            Some(refused) => Err(refused),
                            // The call is the stack's, made under its service role and about
                            // THIS resource ([`super::service_role`]).
                            // BOXED, for the reason the create's own call below records: the
                            // call's future is the owning service's whole handler, and holding
                            // one of those inline in this walk's state machine is what takes a
                            // deployment of a wide template past the thread's stack.
                            None => Box::pin(super::service_role::on_resource(
                                super::service_role::target_of(&resource),
                                provisioner.update(
                                    ctx,
                                    region,
                                    &resource,
                                    &deployed_properties,
                                    &stack.tags_for(&resource.logical_id),
                                    &stack.capabilities,
                                ),
                            ))
                            .await,
                        }
                    }
                    // Nothing exists in any owning service for this type, so there is
                    // nothing anywhere to change: the stack's own record IS the resource,
                    // and the declared configuration is applied by recording it.
                    None => Ok(Some(Created::default())),
                };
                match applied {
                    // The owning service applied it, so the configuration in force is the
                    // DECLARED one — which is what the stack records, and what a later
                    // deployment and drift detection compare against. The transition that
                    // ENDS the resource is pushed after the tags are in step too, below, so
                    // `UPDATE_COMPLETE` means both halves of the change landed.
                    Ok(Some(updated)) => {
                        // An update that answers with a DIFFERENT physical id did not
                        // change the resource that was there: it REPLACED it. The stack
                        // publishes the id the owning service just minted, and what becomes
                        // of the resource it replaced is that resource's own
                        // `UpdateReplacePolicy` — the attribute that governs the OLD physical
                        // resource of a replacement, read here exactly as the cleanup walk of
                        // a replacement the stack itself created reads it
                        // ([`Policies::Replaced`]), whose documented default is to delete it.
                        let superseded = match &updated.physical_resource_id {
                            Some(minted) if *minted != resource.physical_resource_id => {
                                let replaced = resource.clone();
                                resource.physical_resource_id = minted.clone();
                                Some(replaced)
                            }
                            _ => None,
                        };
                        // The change is in force, so what the resource publishes follows from
                        // the declaration that was applied: "Fn::GetAtt returns the value of
                        // an attribute from a resource in the template", and an attribute
                        // whose value is one of that resource's declared properties moves with
                        // it.
                        resource.attributes.extend(declared_attributes.clone());
                        // The values the resource published BEFORE the owning service
                        // answered, which are the ones every declaration of this deployment
                        // was resolved against: the ones the declaration itself publishes,
                        // just installed, over what the record in force carried.
                        let published_before = resource.attributes.clone();
                        // ...and the values that service reports for the resource, which are
                        // the ones it decides rather than the template.
                        resource.attributes.extend(updated.attributes);
                        resource.properties = declared_properties.clone();
                        if let Some(replaced) = superseded {
                            // `Retain` "keeps the resource without deleting" it, so nothing at
                            // all is asked of the owning service about the one this update
                            // replaced: it stays exactly as it is and goes on serving its own
                            // clients, which is the whole of what the attribute asks for.
                            let retained = policy_of(&replaced, Policies::Replaced)
                                == super::deletion_policy::RETAIN;
                            if let Some(owner) =
                                owner_for(&replaced.resource_type).filter(|_| !retained)
                            {
                                // What the owning service answers changes nothing about
                                // this deployment: the replacement is already in force, and
                                // a resource it could not take down is not a resource the
                                // stack still publishes.
                                let _taken_down =
                                    Box::pin(owner.delete(ctx, region, &replaced, None)).await;
                            }
                        }
                        // ...and whether the change has been CARRIED OUT is that service's
                        // to say, exactly as it is for a create: a change it accepted
                        // without having applied it yet holds the resource — and so the
                        // operation — open until it reports the resource ready again
                        // ([`super::stabilising::Awaited::Updated`]). A type whose change
                        // takes effect as the call answers reports `Ready` without asking
                        // anything, so nothing waits for it.
                        let readiness = match owner_for(&resource.resource_type) {
                            Some(provisioner) => {
                                // Boxed for the reason the change's own call above is.
                                Box::pin(provisioner.readiness(
                                    ctx,
                                    region,
                                    &resource.physical_resource_id,
                                    Some(&resource.properties),
                                ))
                                .await
                            }
                            None => Ok(Readiness::Ready),
                        };
                        match readiness {
                            // The change is in force as the call answered, so the values the
                            // owning service publishes for the resource now that it is up are
                            // read here ([`merge_published_attributes`]) — the same read the
                            // pass that releases a readiness wait makes.
                            Ok(Readiness::Ready) => {
                                Box::pin(merge_published_attributes(
                                    ctx,
                                    region,
                                    &mut resource,
                                ))
                                .await;
                            }
                            Ok(Readiness::Pending(state)) => {
                                outcome.stabilising.push(Stabilising {
                                    awaited: super::stabilising::Awaited::Updated,
                                    logical_id: resource.logical_id.clone(),
                                    resource_type: resource.resource_type.clone(),
                                    physical_resource_id: resource
                                        .physical_resource_id
                                        .clone(),
                                    reported_state: state,
                                    provider_request: None,
                                });
                                // The change is NOT in force yet, so this resource has not
                                // published it: nothing that depends on it is acted on in this
                                // pass either, exactly as nothing depending on a resource that
                                // is still being CREATED is ([`awaits_a_dependency`], and the
                                // custom-resource change above, which holds its dependents for
                                // the same reason). "A resource that depends on another is
                                // scheduled only once that one has finished", and a change the
                                // owning service has accepted without carrying out has not
                                // finished — so a dependent acted on now would be asked for
                                // against the configuration the resource still has.
                                unpublished.insert(resource.logical_id.clone());
                            }
                            // That service says the change is not going to take effect, so
                            // waiting it out would report a resource as changed that is not:
                            // the update of this resource failed, carrying what that service
                            // reported — its own account of the resource, or the call of this
                            // provider's it refused.
                            Ok(ref failure @ (Readiness::Failed(_) | Readiness::Refused(_))) => {
                                failed.push((
                                    resource.logical_id.clone(),
                                    super::failed_action::UPDATE,
                                ));
                                outcome.transitions.push(
                                    ResourceTransition::finished(
                                        &resource,
                                        super::resource_status::UPDATE_FAILED,
                                        Some(provider_failure_reason(
                                            owner_for(&resource.resource_type),
                                            &resource.physical_resource_id,
                                            failure,
                                        )),
                                    )
                                    .applying(&declared_properties),
                                );
                                Box::pin(roll_back_failed_resource(
                                    ctx,
                                    region,
                                    stack,
                                    &mut resource,
                                    existing,
                                    &mut outcome,
                                ))
                                .await;
                                outcome.resources.push(resource);
                                continue;
                            }
                            // The service could not be asked at all. One unanswered read is
                            // no evidence about the resource, and the change WAS accepted,
                            // so the wait is opened and the next read asks again — exactly
                            // as a create's is.
                            Err(_) => {
                                outcome.stabilising.push(Stabilising {
                                    awaited: super::stabilising::Awaited::Updated,
                                    logical_id: resource.logical_id.clone(),
                                    resource_type: resource.resource_type.clone(),
                                    physical_resource_id: resource
                                        .physical_resource_id
                                        .clone(),
                                    reported_state: super::resource_status::UPDATE_IN_PROGRESS
                                        .to_string(),
                                    provider_request: None,
                                });
                            }
                        }
                        // The values this resource publishes NOW are the ones a declaration
                        // that reads it has to carry: an update that MOVED one of them — a
                        // launch template's version numbers, which `CreateLaunchTemplateVersion`
                        // decides — leaves every declaration this deployment resolved against
                        // the old value out of date, so nothing that reads one of them is
                        // deployed with the value in force BEFORE this change. The resource is
                        // marked exactly as a custom resource whose own `Update` is outstanding
                        // is ([`awaits_a_dependency`]): what reads it is withheld and resolved
                        // once more against the records as they then stand
                        // ([`deferred::resume`]) — which only a deployment whose declarations
                        // can be resolved again can do
                        // ([`StackProvenance::declarations_resolve_again`]).
                        if stack.declarations_resolve_again
                            && resource.attributes != published_before
                        {
                            unpublished.insert(resource.logical_id.clone());
                        }
                    }
                    Ok(None) => {
                        let unsupported = format!(
                            "NotImplemented: this emulator does not change the properties of \
                             an existing {} in place; the resource was left as it is.",
                            resource.resource_type
                        );
                        failed.push((
                            resource.logical_id.clone(),
                            super::failed_action::UPDATE,
                        ));
                        outcome.transitions.push(
                            ResourceTransition::finished(
                                &resource,
                                super::resource_status::UPDATE_FAILED,
                                Some(unsupported),
                            )
                            .applying(&declared_properties),
                        );
                        Box::pin(roll_back_failed_resource(
                            ctx,
                            region,
                            stack,
                            &mut resource,
                            existing,
                            &mut outcome,
                        ))
                        .await;
                        outcome.resources.push(resource);
                        continue;
                    }
                    Err(refusal) => {
                        // The owning service was asked to apply the change and refused. A
                        // change reaches it as several calls, so what it has carried out of
                        // this one is its own to say: the resource may be PART-CHANGED, and
                        // the rollback of this deployment declares its previous
                        // configuration again there rather than reading the record and
                        // concluding there is nothing to put back
                        // ([`Materialised::part_changed`]).
                        outcome.part_changed.insert(resource.logical_id.clone());
                        failed.push((
                            resource.logical_id.clone(),
                            super::failed_action::UPDATE,
                        ));
                        outcome.transitions.push(
                            ResourceTransition::finished(
                                &resource,
                                super::resource_status::UPDATE_FAILED,
                                Some(handler_failure_reason(
                                    owner_for(&resource.resource_type),
                                    &refusal,
                                    &resource.physical_resource_id,
                                )),
                            )
                            .applying(&declared_properties),
                        );
                        Box::pin(roll_back_failed_resource(
                            ctx,
                            region,
                            stack,
                            &mut resource,
                            existing,
                            &mut outcome,
                        ))
                        .await;
                        outcome.resources.push(resource);
                        continue;
                    }
                }
            }
            // The tags CloudFormation gives the resource are brought back into step AFTER
            // the configuration change, whether or not the declaration changed: a stack
            // operation's `Tags` are propagated to the resources in the stack, and a
            // deployment can state them differently with no resource's declaration changing
            // at all — while a configuration change the owning service REFUSED has already
            // left the loop, so a refused deployment does not re-tag what it could not
            // change. Only a type with a re-tagging arm has anything asked of its service
            // ([`super::provisioner::Provisioner::retag`]), and that arm is idempotent, so a
            // resource whose tags already agree with the set has nothing called about it.
            //
            // A resource this deployment ADOPTS is tagged here too, and that is where its tags
            // come from at all: nothing created it, so the set CloudFormation gives a resource
            // it manages was never applied to it ([`StackProvenance::adopted`]).
            let adopting = stack.adopted.contains(&resource.logical_id);
            // ...and a deployment that states the stack's tags DIFFERENTLY is a change to this
            // resource even where its declaration is untouched: the stack's tags are the tags
            // of the resources it manages, so such a resource is UPDATED by this operation —
            // which is what its status and its events report — exactly as the plan of the same
            // deployment reports it ([`super::change_plan::tag_changes`]). So is a resource
            // being ADOPTED, whatever the stack's tags are: what changes there is WHOSE the
            // resource now is — it comes under the stack's management, under this logical id,
            // which is what CloudFormation's own three records state
            // ([`StackProvenance::tags_for`]) — so adopting a resource a service tags is that
            // resource's own update, which is what the recorded adoption reports it as. Which
            // of those keys the owning service's tagging path will carry is that service's own
            // rule ([`super::provisioner::Provisioner::applicable_tags`]) and says nothing
            // about whether the resource changed hands. A type no service here tags holds no
            // tags for either change to change, so nothing about it is
            // reported, which is the set that plan leaves out as well. The pair opens BEFORE
            // the tagging call, as every transition pair of this walk does.
            let retags_only = !changed
                && (stack.tags_changed || adopting)
                && owner_for(&resource.resource_type).is_some();
            if retags_only {
                outcome.transitions.push(ResourceTransition::starting(
                    &resource,
                    super::resource_status::UPDATE_IN_PROGRESS,
                ));
            }
            let retagged = match owner_for(&resource.resource_type) {
                Some(provisioner) => {
                    let tags = stack.tags_for(&resource.logical_id);
                    // Boxed for the reason the configuration change's own call is.
                    Box::pin(super::service_role::on_resource(
                        super::service_role::target_of(&resource),
                        provisioner.retag(ctx, region, &resource, &deployed_properties, &tags),
                    ))
                    .await
                }
                None => Ok(()),
            };
            match retagged {
                // Both halves of the change landed, which is what ends a resource this
                // deployment acted on.
                Ok(()) => {
                    if changed || retags_only {
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_COMPLETE,
                            None,
                        ));
                    }
                }
                Err(refusal) => {
                    if !changed && !retags_only {
                        // A tag-only change opens the resource's transition pair here: the
                        // configuration half never asked anything of the owning service.
                        outcome.transitions.push(ResourceTransition::starting(
                            &resource,
                            super::resource_status::UPDATE_IN_PROGRESS,
                        ));
                    }
                    failed.push((resource.logical_id.clone(), super::failed_action::UPDATE));
                    outcome.transitions.push(ResourceTransition::finished(
                        &resource,
                        super::resource_status::UPDATE_FAILED,
                        Some(handler_failure_reason(
                            owner_for(&resource.resource_type),
                            &refusal,
                            &resource.physical_resource_id,
                        )),
                    ));
                    Box::pin(roll_back_failed_resource(
                        ctx,
                        region,
                        stack,
                        &mut resource,
                        existing,
                        &mut outcome,
                    ))
                    .await;
                    outcome.resources.push(resource);
                    continue;
                }
            }
            outcome.resources.push(resource);
            continue;
        }
        // Whether this create REPLACES the resource the stack already has under this logical
        // id: the record did not match one it has ([`is_same_resource`]), and yet the stack
        // holds a deployed resource of the same logical id and type — so the declaration asks
        // for a value the resource that exists cannot take, and what the deployment does is
        // create a new physical resource for it. That is reported as the UPDATE's own
        // transitions rather than as a first create ([`Reporting`]).
        let superseded = previous
            .iter()
            .find(|existing| {
                already_deployed(existing)
                    && existing.logical_id == resource.logical_id
                    && existing.resource_type == resource.resource_type
            })
            .cloned();
        let replacing = superseded.is_some();
        let reporting = Reporting::of(replacing, stack.express);
        // The deployment is CREATING this resource: the stack records it as one it created
        // once this is through, whether the create reaches an owning service or not, so the
        // transition that opens it is recorded here.
        outcome.transitions.push(ResourceTransition::creating(
            &resource,
            reporting.in_progress,
            reporting.opening_reason,
        ));
        // ...and for a type whose replacement must be DELETE-BEFORE-CREATE, the resource it
        // replaces is taken down right after that opening transition, BEFORE the create: the
        // owning service cannot hold both at once — a subnet has exactly one route-table
        // association — so a create made while the old resource is still there is one that
        // service refuses. CloudFormation deletes such a resource whatever its
        // `UpdateReplacePolicy` says, `Retain` included: there is no window in which both
        // exist, so the old physical resource cannot be kept while the new one is created
        // ([`super::template::replacement_deletes_first`]).
        //
        // Every other replacement is CREATE-before-delete, and the resource it replaced is
        // taken down in the update's cleanup phase instead, after the replacement is in force
        // ([`Policies::Replaced`]).
        let delete_first = superseded
            .filter(|_| super::template::replacement_deletes_first(&resource.resource_type));
        if let Some(mut replaced) = delete_first {
            match Box::pin(take_down_replaced_resource(
                ctx,
                region,
                &mut replaced,
                &mut outcome,
            ))
            .await
            {
                Ok(()) => {
                    // The old physical resource is gone, so the cleanup phase owes no call
                    // about it: its record is held out of the walk that takes down what this
                    // deployment replaced.
                    taken_down.insert(replaced.physical_resource_id.clone());
                }
                // The owning service would not take the old resource down, so the replacement
                // cannot be created at all: this resource fails carrying that refusal, and the
                // resource that exists is left exactly as it is.
                Err(reason) => {
                    failed.push((resource.logical_id.clone(), reporting.action));
                    outcome.transitions.push(ResourceTransition::failed(
                        &resource,
                        reporting.failed,
                        reason,
                    ));
                    outcome.resources.push(resource);
                    continue;
                }
            }
        }
        // A physical name identifies ONE resource, and CLOUDFORMATION itself holds the
        // record of which of this stack's resources holds which name: a declaration that
        // resolved to a name another resource of this stack already holds — the one the
        // stack had before this deployment, or one this very walk has just created — cannot
        // be created, so nothing is asked of the owning service and the failure is reported
        // in CloudFormation's own words rather than as a handler's refusal
        // ([`super::physical_name`]). The same reason is what a create the OWNING service
        // refuses as a name already in use is reported with where this Region's stacks are
        // what hold it ([`already_managed_reason`]).
        let collision = super::physical_name::taken_in_the_stack(
            &resource,
            outcome
                .resources
                .iter()
                .chain(previous.iter().filter(|existing| already_deployed(existing))),
        )
        .map(|existing| {
            super::physical_name::already_exists_in_stack(
                &existing.physical_resource_id,
                &stack.stack_id,
            )
        });
        if let Some(reason) = collision {
            failed.push((resource.logical_id.clone(), reporting.action));
            outcome.transitions.push(ResourceTransition::failed(
                &resource,
                reporting.failed,
                reason,
            ));
            outcome.resources.push(resource);
            continue;
        }
        // A CUSTOM RESOURCE is backed by no API of this emulator's: it is backed by the
        // PROVIDER its own `ServiceToken` names. CloudFormation SENDS that provider a
        // request and holds the resource until the provider answers at the request's
        // `ResponseURL` ([`custom_resource`]) — so this arm is not a create in an owning
        // service at all, and the wait it opens is released by the provider's answer rather
        // than by a service's own read.
        if custom_resource::is_custom_resource(&resource.resource_type) {
            // A property rule of the DECLARATION, applied before anything is sent: a
            // resource whose `ServiceTimeout` is outside the range the property documents
            // is one CloudFormation refuses itself, so the provider is never asked about it
            // and the resource fails with the rule as its reason.
            if let Some(reason) = custom_resource::service_timeout_refusal(&deployed_properties) {
                failed.push((resource.logical_id.clone(), reporting.action));
                outcome.transitions.push(ResourceTransition::failed(
                    &resource,
                    reporting.failed,
                    reason.to_string(),
                ));
                outcome.resources.push(resource);
                continue;
            }
            let request_id = custom_resource::new_request_id();
            let call = custom_resource::ProviderCall {
                request_type: custom_resource::request_type::CREATE,
                request_id: &request_id,
                stack_id: &stack.stack_id,
                logical_resource_id: &resource.logical_id,
                resource_type: &resource.resource_type,
                // Asked for with the DEPLOYED document, exactly as an owning service's
                // create is: the provider is sent the resolved properties while the record
                // keeps the ones the stack reports.
                properties: &deployed_properties,
                // Nothing exists yet, so the request names no physical resource and no
                // properties the resource was declared with before.
                physical_resource_id: None,
                old_properties: None,
            };
            match custom_resource::send(ctx, region, &call).await {
                Ok(response_url) => {
                    let request = call.remembered(&response_url);
                    // The request is with the provider, so the stack is responsible for the
                    // resource from here on: the record carries `materialised`, which is what
                    // makes a later teardown send the provider the `Delete` it owes.
                    resource.materialised = true;
                    outcome
                        .transitions
                        .push(ResourceTransition::initiated(&resource, reporting.in_progress));
                    outcome.stabilising.push(Stabilising {
                        awaited: super::stabilising::Awaited::Ready,
                        logical_id: resource.logical_id.clone(),
                        resource_type: resource.resource_type.clone(),
                        physical_resource_id: resource.physical_resource_id.clone(),
                        reported_state: reporting.in_progress.to_string(),
                        provider_request: Some(request),
                    });
                    // Its attributes are the `Data` that answer will carry, so nothing that
                    // READS one of them can be created yet ([`awaits_a_dependency`]).
                    unpublished.insert(resource.logical_id.clone());
                    outcome.transitions.push(ResourceTransition::finished(
                        &resource,
                        reporting.completed,
                        None,
                    ));
                    outcome.resources.push(resource);
                    continue;
                }
                // The provider could not be SENT the request — the declaration names none,
                // or the service its token names refused to carry it — so nothing is going
                // to answer for this resource and the deployment stops here, exactly as it
                // does on a refused create.
                Err(refusal) => {
                    failed.push((resource.logical_id.clone(), reporting.action));
                    outcome.transitions.push(ResourceTransition::failed(
                        &resource,
                        reporting.failed,
                        handler_failure_reason(None, &refusal, &resource.physical_resource_id),
                    ));
                    outcome.resources.push(resource);
                    continue;
                }
            }
        }
        // The dynamic references resolved as the resource is DEPLOYED, resolved here: a
        // `{{resolve:secretsmanager:…}}` reference can name a secret THIS deployment
        // created, which exists — under the ARN Secrets Manager minted for it — only now
        // that the resource it depends on has been created
        // ([`super::dynamic_references::resolve_as_deployed`]). The resolved value reaches
        // only the document the owning service is asked for the resource with; the record
        // the stack keeps of it carries the reference the template wrote. A reference that
        // cannot be honoured has no value, so there is nothing to ask that service for and
        // THIS resource fails carrying the resolver's own diagnostic.
        //
        // After the custom-resource arm above, and so never applied to one: a reference of
        // this form is "not currently supported in custom resources", whose provider is
        // therefore sent the reference exactly as the template wrote it rather than a
        // secret CloudFormation resolved for it.
        if let Err(reason) = super::dynamic_references::resolve_as_deployed(
            ctx,
            region,
            &resource.resource_type,
            &mut deployed_properties,
        )
        .await
        {
            failed.push((resource.logical_id.clone(), reporting.action));
            outcome.transitions.push(ResourceTransition::failed(
                &resource,
                reporting.failed,
                reason,
            ));
            outcome.resources.push(resource);
            continue;
        }
        // A tag key inside the namespace AWS reserves for its own tags is not one a
        // DECLARATION may claim either: the resource fails carrying the sentence this service
        // answers a caller-supplied reserved key with, and nothing is asked of the service
        // that owns the type — so no resource of it is created anywhere
        // ([`super::stack_tags::declared_resource_tag_refusal`], which admits the three keys
        // CloudFormation records itself).
        if let Some(reason) = super::stack_tags::declared_resource_tag_refusal(&deployed_properties)
        {
            failed.push((resource.logical_id.clone(), reporting.action));
            outcome.transitions.push(ResourceTransition::failed(
                &resource,
                reporting.failed,
                reason.to_string(),
            ));
            outcome.resources.push(resource);
            continue;
        }
        // Which service owns the type, resolved ONCE here: the create below and the
        // readiness read that follows it are two questions for the same provisioner, and
        // one lookup is what keeps them from ever being asked of different ones.
        let owner = owner_for(&resource.resource_type);
        // An `AWS::CloudFormation::WaitConditionHandle` is backed by no API of any service:
        // what it IS is the pre-signed URL a client signals the wait conditions declared with
        // it at, which CLOUDFORMATION itself mints and publishes as the handle's physical id
        // ([`wait_condition_handle`]). So this arm is not a create in an owning service
        // either: the handle exists once that URL is published and this service serves it
        // ([`crate::service::wait_condition_plane`]).
        if wait_condition_handle::is_handle(&resource.resource_type) {
            let published =
                wait_condition_handle::signal_url(ctx, region, &stack.stack_id, &resource.logical_id);
            // The identifier the naming rules computed for the record is only the stand-in a
            // declaration was resolved against: a wait condition naming this handle through a
            // `Ref` has to carry the URL it is really signalled at, exactly as a property
            // naming a resource whose identity its owning service minted does
            // ([`repoint_references`]).
            let computed =
                ::std::mem::replace(&mut resource.physical_resource_id, published.clone());
            if !computed.is_empty() && computed != published {
                minted.push(Minted {
                    of: resource.logical_id.clone(),
                    // The stand-in is an identifier CloudFormation COMPUTED — a handle
                    // declares no name of its own — so a longer value carrying it carries it
                    // because a reference to this handle put it there ([`Minted::composed`]).
                    composed: true,
                    stand_in: computed,
                    assigned: published,
                });
            }
            // Nothing was created in any owning service for it, so the record stays
            // DECLARED-ONLY exactly as every other such type's does: what makes a later
            // deployment of the same declaration keep the URL already published — rather than
            // mint a second one for a resource that was never replaced — is that the identity
            // of this type is one no deployment can recompute
            // ([`super::StoredStack::deploy_template`]).
            outcome
                .transitions
                .push(ResourceTransition::initiated(&resource, reporting.in_progress));
            outcome.transitions.push(ResourceTransition::finished(
                &resource,
                reporting.completed,
                None,
            ));
            outcome.resources.push(resource);
            continue;
        }
        let created = match owner {
            // The resource is created WITH the tags CloudFormation gives it: the stack's
            // own, plus the three that name the stack and this resource's place in it.
            Some(provisioner) => {
                let tags = stack.tags_for(&resource.logical_id);
                // Asked for with the DEPLOYED document: the record keeps the one the stack
                // reports, so the create carries a resolved dynamic reference's value
                // while nothing the stack reports does.
                let asked = StoredResource {
                    properties: deployed_properties,
                    ..resource.clone()
                };
                // A declaration that hands an IAM ROLE to this service is created only if the
                // principal that asked for the deployment may pass that role: the owning
                // service refuses the call otherwise, so the refusal is answered here and the
                // call is not made at all ([`super::pass_role`]).
                match super::pass_role::refusal(&asked.properties) {
                    Some(refused) => Err(refused),
                    // BOXED, for the reason [`roll_back_failed_resource`]'s own call records:
                    // the call's future is the owning service's whole handler, and holding one
                    // of those inline in this walk's state machine is what takes a deployment
                    // of a wide template past the thread's stack — the more so for a walk a
                    // READ resumes, which is already several frames down
                    // ([`deferred::resume`]).
                    None => Box::pin(super::service_role::on_resource(
                        super::service_role::target_of(&asked),
                        provisioner.create(ctx, region, &asked, &tags, &stack.capabilities),
                    ))
                    .await,
                }
            }
            // No provisioner for this type: nothing is asked of any service, and the
            // resource stays DECLARED-ONLY — which the stack still records as a resource it
            // created, under the identifier the naming rules computed, so the completion of
            // that record is recorded too.
            None => {
                outcome
                    .transitions
                    .push(ResourceTransition::initiated(&resource, reporting.in_progress));
                outcome.transitions.push(ResourceTransition::finished(
                    &resource,
                    reporting.completed,
                    None,
                ));
                outcome.resources.push(resource);
                continue;
            }
        };
        match created {
            // The owning service created it, and what IT answered wins over the computed
            // value: that is the resource's real identity — including the physical id
            // itself, where that service ASSIGNS the id rather than taking it from a name
            // the template or CloudFormation decided.
            Ok(created) => {
                let computed = resource.physical_resource_id.clone();
                let computed_attributes = resource.attributes.clone();
                adopt_minted_identity(&mut resource, created.physical_resource_id);
                // An EMPTY stand-in is no identifier a reference could have resolved to,
                // and matching one would rewrite every empty value — and every empty
                // segment of a longer one — so there is nothing to re-point for it, exactly
                // as for an attribute the naming rules left empty below.
                if !computed.is_empty() && resource.physical_resource_id != computed {
                    minted.push(Minted {
                        of: resource.logical_id.clone(),
                        // The stand-in is an identifier CloudFormation COMPUTED whenever it
                        // is not the name the declaration itself asked for, so a longer
                        // value carrying it carries it because a reference to this resource
                        // put it there — see [`Minted::composed`].
                        composed: super::declared_name(&resource).as_deref() != Some(&*computed),
                        stand_in: computed.clone(),
                        assigned: resource.physical_resource_id.clone(),
                    });
                }
                resource.attributes.extend(created.attributes);
                // An ATTRIBUTE the owning service reports differently from the value the
                // naming rules computed is re-pointed exactly as the physical id is: a
                // later resource's property that `Fn::GetAtt` resolved to the stand-in has
                // to reach its own service as the value this resource REALLY has — a key
                // ARN carrying the key id KMS minted, say — or that service is handed an
                // identifier nothing answers to.
                for (attribute, stand_in) in &computed_attributes {
                    let Some(assigned) = resource.attributes.get(attribute) else {
                        continue;
                    };
                    // An attribute whose computed value IS the identifier `Ref` resolved
                    // to is NOT re-pointed: the uniform convention gives every resource an
                    // `Id`/`<Type>Id` and a `Name`/`<Type>Name` read off its physical id,
                    // so for a type whose id is a NAME those stand-ins are that name — and
                    // another resource's property carrying it (an `AWS::IAM::ManagedPolicy`
                    // naming the role it is attached to) carries the name on purpose. Only
                    // the physical id's own pair above may rewrite it, which it does when
                    // the owning service assigned a different identity.
                    if stand_in == &computed {
                        continue;
                    }
                    if !stand_in.is_empty() && assigned != stand_in {
                        minted.push(Minted {
                            of: resource.logical_id.clone(),
                            stand_in: stand_in.clone(),
                            assigned: assigned.clone(),
                            // An attribute's stand-in is matched inside a longer value for
                            // the same reason a computed identifier's is: a value this
                            // resource's own declaration does not state is one CloudFormation
                            // DERIVED, so another declaration carries it because a reference
                            // to this resource put it there — the api id a log-group name is
                            // built around, the ARN or the endpoint an `Fn::Sub` wraps in a
                            // longer string. A value the declaration itself states (a port it
                            // asked for) is a value of its own wherever it appears, so that
                            // one is matched whole only, exactly as a declared NAME is.
                            composed: !states_value(&resource.properties, stand_in, 0),
                        });
                    }
                }
                resource.materialised = true;
                outcome
                    .transitions
                    .push(ResourceTransition::initiated(&resource, reporting.in_progress));
                // The create was ACCEPTED; whether the resource is usable is the owning
                // service's to say. A type whose create leaves it coming up answers the
                // state it is in, and the resource's creation is held open until that
                // service reports it ready ([`super::stabilising`]) — CloudFormation does
                // not report a resource created on the strength of an accepted call.
                //
                // That question is put where its ANSWER is needed — when the walk reaches a
                // declaration reading this resource, and once more when it is through
                // ([`level_readiness`]) — and not before the walk reaches the next
                // declaration: a service that answers it only once the resource has left its
                // own creation window answers for as long as that window has left to run, so
                // asking here would cost a level of several independent resources one window
                // EACH instead of one for the level.
                unpublished.insert(resource.logical_id.clone());
                coming_up.push(level_readiness::ComingUp {
                    logical_id: resource.logical_id.clone(),
                    resource_type: resource.resource_type.clone(),
                    physical_resource_id: resource.physical_resource_id.clone(),
                    declared: resource.properties.clone(),
                    reporting,
                });
                outcome.resources.push(resource);
                continue;
            }
            Err(refusal) => {
                failed.push((resource.logical_id.clone(), reporting.action));
                // A name already in use that THIS REGION's stacks are what hold is
                // reported in CloudFormation's own words, naming the stack that manages
                // it; everything else the owning service refused is reported as that
                // service worded it.
                let reason = already_managed_reason(&resource, stack, &refusal).unwrap_or_else(
                    || handler_failure_reason(owner, &refusal, &resource.physical_resource_id),
                );
                outcome
                    .transitions
                    .push(ResourceTransition::failed(&resource, reporting.failed, reason));
                forget_unassigned_identity(&mut resource, reporting);
                outcome.resources.push(resource);
                continue;
            }
        }
    }
    // The LAST level of the walk: every resource it started and has not had an answer about
    // yet is asked about here, so no resource is ever reported created without its owning
    // service having said it is ready ([`level_readiness::settle`]). This runs before the
    // stack-level reason below is composed, so a resource that level reports failed is named
    // in it exactly as one whose create was refused is.
    Box::pin(level_readiness::settle(
        ctx,
        region,
        &mut coming_up,
        None,
        &mut outcome,
        &mut failed,
        &mut unpublished,
    ))
    .await;
    // What this walk could not carry through, named in ONE stack-level reason: the field a
    // client parses to recover the set of failed logical ids reports every resource that
    // failed, not only the first of them ([`failed_resource_summary`]). A refusal
    // CloudFormation itself reached stands as it is — no resource of that deployment was
    // attempted at all.
    if outcome.failure.is_none() {
        outcome.failure = failed_resource_summary(&failed);
    }
    // Whatever the failure stopped: nothing was created for it, so it is recorded as the
    // declaration left it — except where the stack ALREADY had that resource, which still
    // exists in its owning service and keeps the record that says so, so the stack's
    // deletion still deletes it.
    for mut resource in declared {
        let pairs = pairs_for(&resource.logical_id, &stack.dependencies, &minted);
        repoint_references(&mut resource.properties, &pairs);
        if let Some(existing) = previous
            .iter()
            .find(|existing| already_deployed(existing) && is_same_resource(existing, &resource))
        {
            carry_forward(existing, &mut resource);
        }
        outcome.resources.push(resource);
    }

    // The exports this deployment would publish are installed as the operation COMPLETES, so
    // the two rules about them are decided at this stage — and HERE within it: the walk's own
    // changes have been made, so the question is asked over the identifiers the owning
    // services really assigned, and nothing the deployment supersedes has been taken down
    // yet. A deployment either rule stops is one CloudFormation carries nothing of through,
    // and what it would have taken down below is removed in the operation's CLEANUP phase —
    // after the change has gone through — so a deployment refused here takes nothing down and
    // its rollback has the resources that were in force to put back
    // ([`super::exports::ExportPublication`] records the whole of why).
    if outcome.failure.is_none() {
        if let Some(publication) = &stack.publication {
            outcome.failure = publication.refusal(&outcome.resources);
        }
    }

    // The resources this deployment replaced or removed exist in their owning services
    // and the stack no longer records them, so the deployment deletes them — a resource
    // the stack stops recording must not be left behind. Judged against what the stack
    // now records (not against the walk above, which a failed create may have stopped
    // early), so one logical resource is never recorded twice.
    let replaced: Vec<StoredResource> = previous
        .iter()
        .filter(|existing| {
            existing.materialised
                // One this walk already took down to make room for its replacement is gone
                // ([`taken_down`]), so there is nothing left to ask its owning service for.
                && !taken_down.contains(&existing.physical_resource_id)
                && !outcome
                    .resources
                    .iter()
                    .any(|recorded| is_same_resource(existing, recorded))
        })
        .cloned()
        .collect();
    if replaced.is_empty() {
        return outcome;
    }
    if outcome.failure.is_some() {
        // The deployment is not being carried through — a resource failed, or the exports it
        // would publish cannot be installed — so it is not the one that replaces anything:
        // the resources it would have deleted are still there, and the stack keeps their
        // records, so the rollback of this operation puts its own record back onto them and a
        // later deletion still deletes them.
        outcome.resources.extend(replaced);
        return outcome;
    }
    // The stack held every one of these records before this deployment stopped recording
    // it, so each one's removal is reported.
    let held: BTreeSet<String> = replaced
        .iter()
        .map(|resource| resource.logical_id.clone())
        .collect();
    // WHY the stack stopped recording a resource is what decides the fate of the physical
    // resource. One whose logical id the deployment no longer declares at all is REMOVED
    // from the stack, which is the event its own `DeletionPolicy` governs; one whose logical
    // id the stack still records, under a new identity, is the OLD half of a REPLACEMENT,
    // whose fate is the separate `UpdateReplacePolicy` attribute's — so the two are taken
    // down as two walks, each reading the attribute that governs it ([`Policies`]).
    let (replacements, removals): (Vec<StoredResource>, Vec<StoredResource>) = replaced
        .into_iter()
        .partition(|existing| {
            outcome
                .resources
                .iter()
                .any(|recorded| recorded.logical_id == existing.logical_id)
        });
    // The deletions already opened, carried across the two walks: a logical id cannot be in
    // both, and sharing the set is what keeps that true of the records as well.
    let mut opened = BTreeSet::new();
    // A deployment is not a `DeleteStack`, so no `DeletionMode` chose for it. The two walks
    // are judged differently because they happen in different PHASES: the old half of a
    // REPLACEMENT is taken down in the update's cleanup phase, after the replacement is
    // already in force — "the stack update was successful, but CloudFormation is cleaning up
    // the old resources" — so a delete the owning service refuses there is reported on that
    // resource and leaves the update complete, with the old physical resource orphaned
    // ([`OnRefusal::Orphan`]); a resource the deployment no longer declares at all is part of
    // the change itself, so a refusal of its delete is this deployment's failure. The relation
    // these two walks are judged against is EMPTY: what they take down is what this deployment
    // replaced or dropped, and the dependency relation of a template being deployed is not
    // the one in force on the stack ([`Dependencies`]).
    let no_relation = Dependencies::new();
    let replacement_walk = Teardown {
        stack_id: &stack.stack_id,
        held: &held,
        needs: &no_relation,
        on_refusal: OnRefusal::Orphan,
        // ...and the two read different attributes for the same reason: what becomes of the
        // OLD physical resource of a replacement is its `UpdateReplacePolicy`
        // ([`Policies::Replaced`]), while a resource the deployment no longer declares is
        // governed by the `DeletionPolicy` the caller's own walk chose.
        policies: Policies::Replaced,
        left_alone: NOTHING_LEFT_ALONE,
    };
    let mut deleted = delete_pass(ctx, region, &replacements, &replacement_walk, &mut opened).await;
    name_no_instance_where_the_identifier_was_kept(&mut deleted.transitions, &outcome.resources);
    if deleted.failure.is_some() {
        // A refusal is the deployment's failure, so the resources the second walk would have
        // taken down are not asked about: their records are kept exactly as the stack held
        // them, so a later deletion still deletes them.
        deleted.resources.extend(removals);
    } else {
        let mut removed = delete_pass(
            ctx,
            region,
            &removals,
            &Teardown {
                policies: removal_policies,
                // A resource the deployment no longer declares is removed BY the change, not
                // after it: a delete its owning service refuses is this deployment's failure.
                on_refusal: OnRefusal::Stop,
                ..replacement_walk
            },
            &mut opened,
        )
        .await;
        deleted.resources.append(&mut removed.resources);
        deleted.transitions.append(&mut removed.transitions);
        deleted.removing.append(&mut removed.removing);
        deleted.failure = removed.failure;
    }
    // A resource an owning service refused to delete still exists there, so the stack
    // keeps its record: the deployment failed, and a later deletion is what has to delete
    // it. The ones that were deleted carry `materialised = false` and are not kept.
    outcome.resources.extend(
        deleted
            .resources
            .into_iter()
            .filter(|resource| resource.materialised),
    );
    outcome.failure = deleted.failure;
    // The removals happened after the creates above, which is the order they are reported
    // in.
    outcome.transitions.extend(deleted.transitions);
    // ...and a resource an owning service has accepted the delete of without having removed
    // it yet holds this deployment open exactly as a resource it is still bringing up does:
    // the update is not complete while the resource it replaced is still there
    // ([`super::stabilising`]).
    outcome.stabilising.extend(deleted.removing);
    outcome
}

/// Report NO instance on the cleanup events of a replaced resource whose identifier the
/// REPLACEMENT KEPT.
///
/// `StackEvent$PhysicalResourceId` is "the name or unique identifier associated with the
/// physical instance of the resource", so an event names the instance it is ABOUT. A
/// replacement is created under an identifier of its own, and the cleanup of the resource it
/// replaced names that resource; where the two carry ONE identifier — a child resource whose
/// name moved to another parent, a name that is unique only inside one
/// ([`super::physical_name::scope_of`]) — that identifier now names the resource the stack
/// RECORDS, so naming it on the old half's event would name another resource's instance.
/// Such an event names an empty one, exactly as the event that opens a create does
/// ([`ResourceTransition::creating`]): the member is reported, and what it reports is that
/// the event names no instance of its own.
///
/// Applied to the transitions of the walk that takes down what this deployment REPLACED
/// ([`Policies::Replaced`]) and to no other, so a removal of a resource the deployment no
/// longer declares — which the stack records nothing else under — still names what it took
/// down. `recorded` is what the stack records after the deployment's own changes, which is
/// the set the identifier would otherwise be read against.
fn name_no_instance_where_the_identifier_was_kept(
    transitions: &mut [ResourceTransition],
    recorded: &[StoredResource],
) {
    for transition in transitions {
        let kept = recorded.iter().any(|resource| {
            resource.logical_id == transition.logical_id
                && resource.resource_type == transition.resource_type
                && Some(&resource.physical_resource_id) == transition.physical_resource_id.as_ref()
        });
        if kept {
            transition.physical_resource_id = Some(String::new());
        }
    }
}

/// Forget the identifier the naming rules computed for a resource whose FIRST create the
/// owning service REFUSED, where the identity of its type is one that service assigns.
///
/// `StackResource$PhysicalResourceId` is "the name or unique identifier associated with the
/// physical instance of the resource", and for such a type the only value that is ever is the
/// one the owning service answered with: the computed value is a STAND-IN a reference was
/// resolved against until the create answered ([`super::template::return_values`]), and the
/// create was refused, so no instance exists and nothing assigned an identifier for it. The
/// resource is therefore reported with none — which is exactly what its failed EVENT already
/// reports ([`ResourceTransition::failed`]) — rather than with an identifier that resolves to
/// nothing in the service that would own it.
///
/// Only a FIRST create, judged by the status the resource's failure is reported under: a
/// refused REPLACEMENT leaves the resource the stack already has in place, under the identity
/// it really carries, and that identity is not this deployment's to forget. A type whose
/// identity CloudFormation itself decides keeps its computed name, which is the name the
/// create was attempted under and the one a client looks for.
fn forget_unassigned_identity(resource: &mut StoredResource, reporting: Reporting) {
    if reporting.failed != super::resource_status::CREATE_FAILED {
        return;
    }
    if !super::template::identity_is_service_assigned(&resource.resource_type) {
        return;
    }
    resource.physical_resource_id.clear();
}

/// Record what an owning service already created: the resource exists there with the
/// identity and the configuration it was created with, so the new record carries both
/// rather than the deployment's own idea of them.
///
/// For a resource whose configuration this deployment CHANGES, [`reconcile`] replaces the
/// properties again with the declared ones once the owning service has applied them — so
/// the record still states the configuration the resource really has, never one that was
/// only asked for.
fn carry_forward(existing: &StoredResource, resource: &mut StoredResource) {
    // Whether the resource exists in a service that owns it is what the EXISTING record
    // says: carrying one resource forward cannot make a declared-only resource — one of a
    // type no service here provisions — into a resource some service holds.
    resource.materialised = existing.materialised;
    for (name, value) in &existing.attributes {
        resource.attributes.insert(name.clone(), value.clone());
    }
    // The identifier the resource really has. For a type whose physical id CloudFormation
    // decides, the two are already equal ([`is_same_resource`] compared them); for one its
    // owning service ASSIGNED, this is the only place the minted identifier comes from, so
    // a re-deploy keeps reporting the `j-` job-flow id EMR gave the cluster rather than the
    // name CloudFormation computed for the record.
    resource.physical_resource_id = existing.physical_resource_id.clone();
    // The properties the resource really has are the ones it was created with. For a
    // resource this deployment keeps unchanged they are the same document; where they
    // differ, this is what keeps the stack from recording a configuration nothing applied.
    resource.properties = existing.properties.clone();
}

/// Whether two records are the same resource: the same logical id, of the same type, with
/// the same physical id — the three things that decide whether a deployment keeps a
/// resource or replaces it. The one place that rule is written, so the record a re-deploy
/// carries forward ([`super::StoredStack::deploy_template`]) and the resource
/// [`reconcile`] leaves alone cannot drift apart.
///
/// A resource whose physical id its owning service ASSIGNED is matched by the same rule:
/// the deploying record already carries the id that service minted, because
/// [`super::StoredStack::deploy_template`] carried it forward — nothing here could
/// recompute it. One whose id is an ADDRESS that service REPORTED is matched by what that
/// identifier names rather than by its spelling ([`names_the_same_instance`]), because a
/// deployment recomputes such a value down to the endpoint it was reported at.
///
/// For a type whose resource lives INSIDE a parent, the physical name identifies one resource
/// only together with that parent ([`super::physical_name::scope_of`], which is the scope the
/// owning service addresses one of its resources in), so two records carrying one name under
/// DIFFERENT parents are two resources: a declaration that moves such a resource to another
/// parent asks for one the resource that exists cannot become, which is a resource this
/// deployment creates rather than a name it asks that service to re-point.
pub fn is_same_resource(left: &StoredResource, right: &StoredResource) -> bool {
    left.logical_id == right.logical_id
        && left.resource_type == right.resource_type
        && names_the_same_instance(
            &left.resource_type,
            &left.physical_resource_id,
            &right.physical_resource_id,
        )
        && super::physical_name::scope_of(left) == super::physical_name::scope_of(right)
}

/// Whether two physical identifiers of a resource of `resource_type` name the SAME instance.
///
/// Identical values do, which is the whole answer for every type whose identifier is a name, an
/// id or an ARN. For a type whose identifier is an ADDRESS of the endpoint this emulator is
/// served at ([`super::template::identity_is_a_reported_address`]), two DIFFERENT values can
/// name one resource as well: a queue url names its queue by its `/<accountId>/<QueueName>`
/// path, its authority naming the endpoint the url was reported at, and "which authority a url
/// carries decides nothing about WHICH queue it addresses" — that is the reading Amazon SQS
/// itself resolves a `QueueUrl` by. So the value the naming rules computed against that
/// service's published regional host and the one it ANSWERED with at this emulator's own
/// endpoint are one identifier, while two different paths are two queues: a declaration whose
/// name — generated or stated — resolves to another path still asks for a resource the one
/// that exists cannot become.
fn names_the_same_instance(resource_type: &str, left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    if !super::template::identity_is_a_reported_address(resource_type) {
        return false;
    }
    // The path of an address: what follows the authority, which is what names the resource.
    // A value carrying no path at all names nothing to compare.
    let path = |value: &str| {
        let after_scheme = ::service_common::data_plane_addressing::url_authority(value);
        after_scheme
            .find('/')
            .map(|at| after_scheme[at..].trim_end_matches('/').to_string())
            .filter(|path| path.len() > 1)
    };
    match (path(left), path(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Whether the record `existing` is the resource `declared` ASKS FOR: the same identity
/// ([`is_same_resource`]) and, for a type whose identifier its OWNING SERVICE assigns, a
/// declaration that still asks for the resource that identifier names.
///
/// Such an identifier is one no deployment recomputes, so the record carries the one in force
/// forward ([`super::StoredStack::deploy_template`]) — and an identifier that carries no trace
/// of the properties the resource was created with stands still while the declaration asks for
/// something the resource cannot become. An Amazon ECS task-definition revision is exactly
/// that: it is immutable, every property of the type carries "Update requires: `Replacement`"
/// ([`super::template::replacement_properties`]), and its ARN is its family and a number. A
/// declaration that restates one of those properties therefore asks for a DIFFERENT resource,
/// which this deployment CREATES — the same reading the rename pass makes of a replacement
/// ([`super::stack::declares_the_same_replacing_properties`], read here rather than asked
/// again, so the two cannot disagree about what a replacement is).
///
/// For every other type the question is already settled before this: a replacement moves the
/// name such a resource is created under, and with it the identifier
/// ([`super::replacing_update::rename_replacements`]), so the identity test above answers it.
fn is_the_declared_resource(existing: &StoredResource, declared: &StoredResource) -> bool {
    is_same_resource(existing, declared)
        && (!super::template::identity_is_service_assigned(&declared.resource_type)
            || super::stack::declares_the_same_replacing_properties(existing, declared))
}

/// Whether a record of the set a deployment reconciles against is a resource the stack has
/// ALREADY DEPLOYED, so a deployment that re-declares it with the same identity
/// ([`is_same_resource`]) acts on nothing.
///
/// A deployment creates a resource the stack does not have yet, and that is what a RECORD
/// says: one the owning service created carries `materialised`, and one of a type no service
/// here provisions carries no flag because there was never anything to create — the stack
/// still recorded it as a resource it created, under the identifier the naming rules
/// computed, and the next deployment of the same declaration must not report it as created
/// all over again. What the flag separates is therefore the DECLARED-ONLY record (nothing to
/// create, nothing to carry out) from one whose create an owning service refused or never
/// reached: that resource does not exist there, so a deployment that declares it again
/// creates it.
///
/// Which types are declared-only is asked of [`owner_for`] — the one seam that answers who
/// provisions a type, whichever module that type's arms live in. Asking a sub-catalogue of it
/// instead would read a type it does not list as declared-only, and a refused create of such
/// a type would then be taken for a resource that exists: the next deployment would ask its
/// owning service to CHANGE a resource that service does not hold.
pub(super) fn already_deployed(existing: &StoredResource) -> bool {
    existing.materialised || owner_for(&existing.resource_type).is_none()
}

/// What deleting a stack's resources left.
#[derive(Debug, Clone, Default)]
pub struct Deleted {
    /// The stack's resources as the deletion left them: one whose deletion the owning
    /// service carried out no longer carries `materialised`, so a retried delete does not
    /// ask for it again.
    pub resources: Vec<StoredResource>,
    /// The reason the stack's deletion cannot be reported as complete: an owning service
    /// refused to delete one of the resources — the stack-level summary of which ones
    /// ([`super::resource_failure_summary`]), with that service's own refusal carried on
    /// the resource's transition.
    pub failure: Option<String>,
    /// Every resource transition this deletion made, oldest first — the stack's
    /// per-resource event records, exactly as [`Materialised::transitions`] carries a
    /// deployment's.
    pub transitions: Vec<ResourceTransition>,
    /// The resources whose deletion the owning service ACCEPTED while still holding them:
    /// the call went through and the resource is not gone yet, so the operation that deleted
    /// it is held open until that service no longer holds it
    /// ([`super::stabilising`]) — a stack operation is never reported complete while a
    /// resource it deleted is still there. Recorded by the operation that drove the teardown
    /// ([`super::StoredStack::record_stabilisations`]), exactly as
    /// [`Materialised::stabilising`] is.
    pub removing: Vec<Stabilising>,
    /// The resources the owning service REFUSED to delete, in the order this walk reached
    /// them: the logical ids [`Deleted::failure`] summarises.
    ///
    /// What a FURTHER pass of the same teardown must leave alone ([`Teardown::left_alone`]):
    /// the refusal has already been reported on each of them, so a pass that asks about the
    /// resources an unfinished removal held back asks about none of these a second time.
    pub refused: Vec<String>,
}

/// What a teardown does with a resource the service that owns it REFUSES to delete.
///
/// `DeleteStackInput$DeletionMode` is what chooses: "STANDARD — use the standard behavior",
/// under which the refusal is the stack's failure, and "FORCE_DELETE_STACK — delete the stack
/// if it's stuck in a DELETE_FAILED state due to resource deletion failure", under which the
/// refused resource is left where it is and the stack's deletion goes on without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnRefusal {
    /// The refusal STOPS the teardown and is the stack's own failure: nothing after the
    /// refused resource is asked about ([`Deleted::failure`] names it).
    Stop,
    /// The refused resource is ABANDONED — nothing more is asked of its owning service, so
    /// it stays exactly as it is — and the walk carries on with the rest, so the teardown
    /// reports no failure of its own.
    Abandon,
    /// The refusal is REPORTED on the resource (`DELETE_FAILED`, carrying why) and is not the
    /// teardown's failure: the resource is left where it is, the stack stops recording it, and
    /// the walk carries on with the rest.
    ///
    /// This is the CLEANUP of a resource an update REPLACED, which runs after the replacement
    /// is already in force: "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS — the stack update was
    /// successful, but CloudFormation is cleaning up the old resources" (the stack status of
    /// that phase), so the change the update made cannot be undone by a delete that failed
    /// afterwards — the old physical resource is simply left behind, no longer held by the
    /// stack.
    Orphan,
}

/// Delete the resources a stack created, newest first — the reverse of the order they were
/// created in, so a resource is deleted before whatever it depends on.
///
/// [`Deleted::failure`] names the first resource whose owning service refused to delete
/// it; the stack's deletion cannot be reported as complete then, and the resources it had
/// already deleted are reported as deleted so a retry does not ask for them again. A
/// resource the service no longer has is a success: the request asks for a state the
/// service is already in. Under [`OnRefusal::Abandon`] a refusal is not a failure at all:
/// that resource is left alone and the walk goes on, so the teardown answers no failure and
/// the stack's deletion completes around what could not be deleted.
///
/// `held` is the logical ids the stack HOLDS a record of
/// ([`super::StoredStack::held_resource_ids`]) — the resources this teardown is responsible
/// for. Every one of them goes through its deletion and is recorded as the pair of
/// transitions it went through ([`Deleted::transitions`]), which is what names the resource
/// a deletion failed on and what makes the teardown's order observable: a resource of a type
/// this emulator provisions NOWHERE has no owning service to ask, so nothing is asked of
/// anybody, but the stack owned the record and its removal is reported exactly as any other
/// resource's is. A record the stack does NOT hold — one whose create an owning service
/// refused, one a failed deployment never reached — has nothing to remove and is recorded
/// nowhere.
///
/// What becomes of each PHYSICAL resource is its own `DeletionPolicy`, where this walk is one
/// that reads the attribute ([`Policies`]): `Retain` is asked of nobody and recorded
/// `DELETE_SKIPPED`, `Snapshot` is the owning service's delete taking a final snapshot
/// ([`final_snapshot_identifier`]), and `Delete` — the default, and what a walk that does not
/// read the attribute does — is the delete this walk has always made.
///
/// A resource whose owning service ACCEPTED the delete without having removed the resource
/// yet is reported in [`Deleted::removing`]: the operation that drove this teardown holds its
/// completion on those, so no stack operation is reported complete while a resource it
/// deleted is still there.
pub async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    walk: &Teardown<'_>,
) -> Deleted {
    let mut outcome = delete_pass(ctx, region, resources, walk, &mut BTreeSet::new()).await;
    settle_accepted_removals(ctx, region, &mut outcome).await;
    outcome
}

/// How many times a teardown whose own FAILURE ends the stack operation asks the owning
/// services whether the removals it had already accepted have been carried out.
///
/// A refusal CONCLUDES the operation — the stack takes its failed status on the spot — and
/// the waits a teardown leaves behind ([`Deleted::removing`]) are released by an operation
/// that is still in flight, which a concluded one is not. So a walk that is about to report a
/// failure asks those services itself. The bound is generous, for the reason
/// [`UNWIND_ATTEMPTS`] is: together with [`ACCEPTED_REMOVAL_RETRY_DELAY`] it has to outlast
/// the spans the services of this emulator take to carry a removal out, or a resource one of
/// them was about to let go of would be reported as one it still holds.
const ACCEPTED_REMOVAL_ATTEMPTS: usize = 40;

/// How long that wait leaves between asks. Chosen: short enough that the removals a teardown
/// accepted are noticed as soon as they land, rather than one span after.
const ACCEPTED_REMOVAL_RETRY_DELAY: ::std::time::Duration =
    ::std::time::Duration::from_millis(100);

/// Carry out the removals a teardown that FAILED had already accepted, before the failure it
/// is about to report concludes the operation.
///
/// CloudFormation reports a resource `DELETE_COMPLETE` only once the service that owns it no
/// longer holds it, and it does not report an operation finished — completed or failed —
/// while a resource it deleted is still there. A teardown that goes through holds to that
/// through the waits it leaves behind ([`Deleted::removing`], released by
/// [`outstanding_teardown`] as the operation goes on); one whose own refusal ends the
/// operation has no such later pass, so the answers are read here, from the owning services'
/// own reads ([`super::provisioner::Provisioner::removal`]) and never a span this crate
/// assumed. Every resource reported gone loses its wait: the stack's failure then reports a
/// deletion that really happened.
///
/// Nothing is waited for where the walk has no failure to report, where it accepted no
/// removal, or where the only waits left are a CUSTOM RESOURCE's — what ends one of those is
/// the answer its provider PUTs to the request, which no read of this crate's can bring
/// forward. A removal an owning service has not carried out inside the bound keeps its wait,
/// exactly as it would without this pass.
async fn settle_accepted_removals(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    outcome: &mut Deleted,
) {
    if outcome.failure.is_none() {
        return;
    }
    for attempt in 0..ACCEPTED_REMOVAL_ATTEMPTS {
        if !outcome.removing.iter().any(answered_by_a_read) {
            return;
        }
        if attempt > 0 {
            ::tokio::time::sleep(ACCEPTED_REMOVAL_RETRY_DELAY).await;
        }
        let mut still_removing: Vec<Stabilising> = Vec::new();
        for pending in ::std::mem::take(&mut outcome.removing) {
            if !answered_by_a_read(&pending) {
                still_removing.push(pending);
                continue;
            }
            let gone = match owner_for(&pending.resource_type) {
                // BOXED, for the reason the walk's own calls are: the read is the owning
                // service's whole handler.
                Some(provisioner) => matches!(
                    Box::pin(provisioner.removal(ctx, region, &pending.physical_resource_id))
                        .await,
                    Ok(Removal::Gone)
                ),
                // No service of this emulator owns the type, so no read will ever answer
                // for it: there is nothing left holding the resource and nothing to wait
                // for.
                None => true,
            };
            if !gone {
                still_removing.push(pending);
            }
        }
        outcome.removing = still_removing;
    }
}

/// Whether what ends this wait is a READ of the service that owns the resource — which is
/// every resource but a CUSTOM RESOURCE, whose wait is the answer its own provider owes the
/// request it was sent ([`custom_resource`]).
fn answered_by_a_read(pending: &Stabilising) -> bool {
    pending.awaited == super::stabilising::Awaited::Gone && pending.provider_request.is_none()
}

/// What one teardown walk is judged by, beside the resources it is walking: the records the
/// stack HOLDS, what each of them needs, what a refusal does, and whether the walk reads each
/// resource's own `DeletionPolicy`.
///
/// One value rather than four arguments, because every caller of [`delete`], [`unwind`] and
/// the passes behind them answers all four for the same walk.
#[derive(Debug, Clone, Copy)]
pub struct Teardown<'a> {
    /// `Stack$StackId`: the stack whose resources this teardown is taking down, which is
    /// what a CUSTOM RESOURCE's `Delete` request names ([`custom_resource::ProviderCall`]).
    pub stack_id: &'a str,
    /// The logical ids the stack holds a record of
    /// ([`super::StoredStack::held_resource_ids`]) — the resources this teardown is
    /// responsible for.
    pub held: &'a BTreeSet<String>,
    /// What each of those resources needs ([`Dependencies`]).
    pub needs: &'a Dependencies,
    /// What the walk does with a resource the owning service refuses to delete.
    pub on_refusal: OnRefusal,
    /// Whether the walk reads each resource's own `DeletionPolicy` ([`Policies`]).
    pub policies: Policies,
    /// The resources this walk LEAVES ALONE whatever it finds: the ones an earlier pass of
    /// the same teardown was refused ([`Deleted::refused`]) and everything those need
    /// ([`needed_by`]).
    ///
    /// A refusal has been reported on each of them already, and a resource a refused one
    /// needs cannot be deleted while that one is still there — so a pass that is only there
    /// to ask about the resources an unfinished removal held back asks about none of them,
    /// and reports no second refusal of its own. [`NOTHING_LEFT_ALONE`] for the first pass of
    /// a teardown, which has been refused nothing yet.
    pub left_alone: &'a BTreeSet<String>,
}

/// The [`Teardown::left_alone`] of a walk no earlier pass of the same teardown was refused
/// anything by — every walk but the owed pass that follows a held refusal.
pub const NOTHING_LEFT_ALONE: &BTreeSet<String> = &BTreeSet::new();

/// The DIRECT dependency relation of the resources a teardown is walking, keyed by logical
/// id: what each resource needs ([`super::StoredStack::resource_dependencies`]).
///
/// A deletion asks the owning services in reverse creation order — a resource before
/// whatever it depends on — and the relation is what says WHICH resources a refusal holds
/// back: a resource the refused one needs cannot be deleted while that one is still there, so
/// the walk leaves it alone and the stack keeps its record. An EMPTY relation is one the
/// caller does not know (a deployment taking down what it replaced knows the template it is
/// deploying, not the stack's), and then a refusal holds nothing else back.
pub type Dependencies = BTreeMap<String, BTreeSet<String>>;

/// The dependency relation the teardown of the operation IN FLIGHT on `stack` is judged
/// against ([`Dependencies`]): the one of the template that operation is deploying where it
/// has one — a failed CREATE's rollback takes down what that create was deploying — and the
/// one in force on the stack otherwise, which is what its own deletion walks.
///
/// Read under the same lock the resources are, by the operation that is about to drive the
/// teardown, so the relation and the records it is asked about are one reading of the stack.
pub fn dependencies_in_flight(stack: &super::StoredStack) -> Dependencies {
    match stack.pending_deployment.as_ref() {
        Some(deployment) => deployment.dependencies.clone(),
        None => stack.resource_dependencies.clone(),
    }
}

/// Every resource `refused` needs, directly or through another resource — the set a teardown
/// that could not delete `refused` must leave alone.
///
/// Walked breadth-first over the relation, visiting each logical id at most once, so a
/// relation with a cycle in it (which no deployable template has) terminates.
fn needed_by(needs: &Dependencies, refused: &str) -> BTreeSet<String> {
    let mut closure: BTreeSet<String> = BTreeSet::new();
    let mut frontier: Vec<&str> = vec![refused];
    while let Some(logical_id) = frontier.pop() {
        let Some(needed) = needs.get(logical_id) else {
            continue;
        };
        for dependency in needed {
            if closure.insert(dependency.clone()) {
                frontier.push(dependency.as_str());
            }
        }
    }
    closure
}

/// Whether `logical_id` depends — directly or through another resource — on one of
/// `unpublished`: the resources whose own attributes are not published yet.
///
/// What a resource DEPENDS ON is what its declaration reads ([`Dependencies`]), so this is
/// the one question that decides whether its declaration is resolved: a resource none of
/// whose dependencies is waiting reads nothing that is still unknown. An EMPTY set is
/// nothing waiting, which is the common case and answers without walking the relation.
fn awaits_a_dependency(
    logical_id: &str,
    dependencies: &Dependencies,
    unpublished: &BTreeSet<String>,
) -> bool {
    if unpublished.is_empty() {
        return false;
    }
    needed_by(dependencies, logical_id)
        .iter()
        .any(|needed| unpublished.contains(needed))
}

/// Whether `logical_id` depends — directly or through another resource — on one of the
/// resources this walk could not carry out ([`reconcile`]'s own `failed`).
///
/// Judged by the same relation [`awaits_a_dependency`] reads, for the same reason: what a
/// resource depends on is what its declaration reads, and a declaration reading a resource
/// that failed is not one any owning service could be asked for. Such a resource is left
/// alone — nothing is attempted for it and it reports no transition of its own — while every
/// resource whose dependencies DID come through is still attempted, which is what makes a
/// failed deployment report every broken resource rather than the first one.
fn depends_on_a_failure(
    logical_id: &str,
    dependencies: &Dependencies,
    failed: &[(String, &'static str)],
) -> bool {
    if failed.is_empty() {
        return false;
    }
    needed_by(dependencies, logical_id)
        .iter()
        .any(|needed| failed.iter().any(|(id, _)| id == needed))
}

/// Take down the physical resource a REPLACEMENT supersedes, BEFORE the replacement is
/// created, and report that removal on the resource's own transitions.
///
/// For a type whose owning service cannot hold two of the resource at once — a subnet has
/// exactly one route-table association — a replacement is delete-before-create
/// ([`super::template::replacement_deletes_first`]): the old resource is deleted first, so the
/// create that follows is one that service admits and there is never a moment in which both
/// exist. The deletion is reported as the pair every other removal is reported with — the
/// `DELETE_IN_PROGRESS` naming the identifier being removed, then `DELETE_COMPLETE` — which is
/// the order a client reading the stack's events sees it in.
///
/// `Err` is the owning service's own refusal of the removal, worded as a handler's failure:
/// nothing was taken down, so the replacement cannot be created and the caller fails the
/// resource with it.
async fn take_down_replaced_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    replaced: &mut StoredResource,
    outcome: &mut Materialised,
) -> Result<(), String> {
    outcome.transitions.push(ResourceTransition::starting(
        replaced,
        super::resource_status::DELETE_IN_PROGRESS,
    ));
    let owner = owner_for(&replaced.resource_type);
    let taken_down = match owner {
        // Boxed for the reason every other provider call of this walk is boxed.
        Some(provisioner) => Box::pin(super::service_role::on_resource(
            super::service_role::target_of(replaced),
            provisioner.delete(ctx, region, replaced, None),
        ))
        .await,
        // Nothing exists in any owning service for this type, so there is nothing to take
        // down: the stack's own record IS the resource, and the record it is being replaced by
        // is the one the stack now keeps.
        None => Ok(()),
    };
    match taken_down {
        Ok(()) => {
            // The old physical resource is gone, so the record the stack held for it no longer
            // stands for anything an owning service holds.
            replaced.materialised = false;
            outcome.transitions.push(ResourceTransition::finished(
                replaced,
                super::resource_status::DELETE_COMPLETE,
                None,
            ));
            Ok(())
        }
        Err(refusal) => {
            let reason =
                handler_failure_reason(owner, &refusal, &replaced.physical_resource_id);
            outcome.transitions.push(ResourceTransition::finished(
                replaced,
                super::resource_status::DELETE_FAILED,
                Some(reason.clone()),
            ));
            Err(reason)
        }
    }
}

/// Whether a teardown reads each resource's own `DeletionPolicy`
/// ([`super::deletion_policy`]) for what becomes of the physical resource.
///
/// The attribute says what to do with a resource "when its stack is deleted", and
/// CloudFormation reads it wherever the stack STOPS MANAGING the resource: the stack's own
/// deletion, and a deployment that no longer declares it. The OLD physical resource of a
/// REPLACEMENT is a different event governed by a different attribute —
/// `UpdateReplacePolicy`, whose action this service reports
/// ([`super::change_plan::policy_action`]) — so a walk taking one of those down reads THAT
/// attribute instead ([`Policies::Replaced`]), whose three documented values are the first
/// three of these and whose documented default is to delete the resource.
///
/// WHICH walk this is also answers the one value whose meaning depends on the walk,
/// `RetainExceptOnCreate` ([`policy_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policies {
    /// Each resource's own `DeletionPolicy` decides what becomes of it.
    Honoured,
    /// Each resource's own `DeletionPolicy` decides what becomes of it, and this walk is the
    /// ROLLBACK of the operation that CREATED the resources it is taking down — the one
    /// event `RetainExceptOnCreate` does not keep a resource at
    /// ([`super::deletion_policy::RETAIN_EXCEPT_ON_CREATE`]).
    RolledBack,
    /// Each resource's own `UpdateReplacePolicy` decides what becomes of it: this walk is
    /// taking down the old physical resource of a replacement.
    Replaced,
    /// The physical resource is deleted whatever either attribute states.
    Ignored,
}

/// What this walk does with one resource: the attribute the walk reads for it, and the
/// documented default otherwise ([`super::deletion_policy::default_for`]) — which is also what
/// a declaration naming a value that is none of the documented ones gets.
///
/// `RetainExceptOnCreate` is `DeletionPolicy`'s fourth value, and the one whose answer depends
/// on the walk: "retain the resource only if the resource creation fails", so it keeps the
/// resource exactly as `Retain` keeps one wherever the stack STOPS MANAGING a resource it has
/// — the stack's own deletion, and a deployment that no longer declares it — and the resource
/// is deleted only where the walk is the rollback of the operation that created it
/// ([`Policies::RolledBack`]). `UpdateReplacePolicy` documents three values and not that one,
/// so a declaration stating it THERE states no documented policy and gets the default: the
/// old resource of a replacement is not one whose creation failed.
fn policy_of(resource: &StoredResource, policies: Policies) -> &str {
    let stated = match policies {
        Policies::Ignored => return super::deletion_policy::DELETE,
        Policies::Replaced => {
            return resource
                .update_replace_policy
                .as_deref()
                .unwrap_or(super::deletion_policy::DELETE)
        }
        Policies::Honoured | Policies::RolledBack => resource.deletion_policy.as_deref(),
    };
    match stated.unwrap_or_else(|| {
        super::deletion_policy::default_for(&resource.resource_type, &resource.properties)
    }) {
        super::deletion_policy::RETAIN_EXCEPT_ON_CREATE => match policies {
            Policies::RolledBack => super::deletion_policy::DELETE,
            _ => super::deletion_policy::RETAIN,
        },
        documented => documented,
    }
}

/// How many digest characters the identifier of a `Snapshot` policy's snapshot ends in.
///
/// Enough that the name cannot be one a caller happened to create a snapshot of its own
/// under, and short enough to leave the resource's own identifier legible inside the length
/// every owning service's final-snapshot member bounds (Amazon RDS's
/// `FinalDBSnapshotIdentifier` admits 255 characters).
const FINAL_SNAPSHOT_DIGEST_CHARS: usize = 8;

/// What the digest of a final snapshot's identifier is derived under, which keeps it apart
/// from every other identifier this crate derives from the same resource.
const FINAL_SNAPSHOT_SALT: &str = "final-snapshot";

/// The identifier CloudFormation asks the owning service to take the snapshot of a
/// `Snapshot` policy's resource under.
///
/// Derived from the resource's own physical id, so the snapshot a deletion leaves behind
/// names the resource whose contents it holds, and a deletion ASKED AGAIN (a stack whose
/// earlier deletion failed on another resource) asks for that same snapshot rather than a
/// second one. The form is the one those services' final-snapshot members admit — the
/// resource's identifier, which is one of their own identifiers already, and hexadecimal
/// digits after a single hyphen.
fn final_snapshot_identifier(resource: &StoredResource) -> String {
    format!(
        "{}-{}",
        resource.physical_resource_id.trim_end_matches('-'),
        super::template::content_digest(
            FINAL_SNAPSHOT_SALT,
            &resource.physical_resource_id,
            FINAL_SNAPSHOT_DIGEST_CHARS,
        )
    )
}

/// The resource types whose `DeletionPolicy: Snapshot` snapshot is a CALL OF THE OWNING
/// SERVICE'S OWN, with the operation that takes it.
///
/// "For resources that support snapshots, CloudFormation creates a snapshot for the resource
/// before deleting it", and the published list of types that support the policy is wider than
/// the ones whose delete takes a final-snapshot member (Amazon RDS's
/// `FinalDBSnapshotIdentifier`, Amazon Redshift's `FinalClusterSnapshotIdentifier`): an
/// `AWS::EC2::Volume`'s snapshot is an `ec2:CreateSnapshot` of the volume, which `DeleteVolume`
/// models no member for. For such a type the snapshot is taken by the call named here, before
/// the delete; every other type's is the delete's own member
/// ([`super::provisioner::Provisioner::delete`]), and a type that is neither has no snapshot
/// to take.
///
/// Each entry is the template type, the signing name of the service that owns the snapshot,
/// its awsQuery API version and the action: the snapshot is the OWNING service's own record,
/// made through its own wire, so what a stack leaves behind is a snapshot that service's own
/// reads report.
const SNAPSHOT_BY_A_CALL_OF_ITS_OWN: &[(&str, &str, &str, &str)] =
    &[("AWS::EC2::Volume", "ec2", "2016-11-15", "CreateSnapshot")];

/// Take the snapshot a `Snapshot` deletion policy owes for one resource, where that snapshot
/// is a call of the owning service's own ([`SNAPSHOT_BY_A_CALL_OF_ITS_OWN`]).
///
/// `identifier` is the name CloudFormation asks for the snapshot under
/// ([`final_snapshot_identifier`]), which travels as the snapshot's `Description`: EC2 mints
/// the `snap-…` id itself, so the identifier CloudFormation derived is what names the
/// snapshot's provenance rather than its id.
///
/// `Ok(())` for a type whose snapshot this is not the call for — the delete's own member
/// carries it, or there is no snapshot of that type to take — so nothing is asked of any
/// service for it. `Err(refusal)` is the owning service's own answer, which leaves the
/// resource undeleted: the policy exists to preserve the data, so a snapshot that could not
/// be taken is not a resource that may be removed.
async fn take_final_snapshot(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    identifier: &str,
) -> Result<(), Refusal> {
    let Some((_, signing_name, version, action)) = SNAPSHOT_BY_A_CALL_OF_ITS_OWN
        .iter()
        .find(|(resource_type, _, _, _)| *resource_type == resource.resource_type)
    else {
        return Ok(());
    };
    let parameters = vec![
        ("VolumeId".to_string(), resource.physical_resource_id.clone()),
        ("Description".to_string(), identifier.to_string()),
    ];
    call_query(ctx, signing_name, region, version, action, &parameters).await?;
    Ok(())
}

/// A refusal another service answered with: its error CODE and its message, kept apart so
/// a caller branches on the code (as a client does) and never on text a request can
/// influence.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl ::std::fmt::Display for Refusal {
    /// One line, as the stack's status reason carries it.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        if self.message.is_empty() {
            write!(formatter, "{}", self.code)
        } else {
            write!(formatter, "{}: {}", self.code, self.message)
        }
    }
}

/// What stands in for an error code an answer did not carry: not a code any service
/// answers, so it can never collide with one. An answer that identifies nothing is still
/// a failed create — the status it came back with is what the stack's reason reports.
const UNNAMED_CODE_PREFIX: &str = "HTTP ";

#[cfg(test)]
mod repointed_token_tests;

#[cfg(test)]
mod handler_error_class_tests;

/// The TEARDOWN WALK itself: the pass that asks the owning services to delete a stack's
/// resources, and the re-asked pass a rollback drives ([`teardown_walk`]). Its own file so this
/// one stays inside the length the build holds every file to; what the walk is judged by, what
/// it reports back and the entry points that drive it are here.
mod teardown_walk;
use teardown_walk::delete_pass;
pub use teardown_walk::unwind;

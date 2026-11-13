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
//! dependency by. A LATER deployment resolves its declarations against the identifiers the
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

mod apigateway_rest_api;
/// The four `AWS::AppConfig::*` types a stack declares — the application, its environment,
/// the configuration profile that reads a configuration and the deployment strategy that
/// rolls one out: created in AWS AppConfig itself ([`appconfig_resources`]).
mod appconfig_resources;
/// The two `AWS::ApplicationAutoScaling::*` types a stack declares — the scalable target and
/// the policy that steers it: visible to the rest of this service's state because the types'
/// own NAMES live here, beside the arms that provision them
/// ([`super::template::return_values`] names the two whose identity their create answers).
pub(super) mod application_auto_scaling;
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
mod ec2_vpc_network;
/// `AWS::ECS::TaskDefinition`: the revision a stack declares, registered in Amazon ECS
/// itself ([`ecs_task_definition`]).
mod ecs_task_definition;
/// The four `AWS::ElasticLoadBalancingV2::*` types a stack declares — the load balancer, the
/// target group traffic is routed to, the listener on the load balancer and the rule that
/// routes a request: created in Elastic Load Balancing itself
/// ([`elastic_load_balancing`]).
pub(super) mod elastic_load_balancing;
mod iam_policy;
/// `AWS::IoT::Authorizer`: the custom authorizer a stack declares, created in AWS IoT
/// itself ([`iot_authorizer`]).
pub(super) mod iot_authorizer;
mod lambda_alias;
mod lambda_event_source_mapping;
mod lambda_function;
mod lambda_layer_version;
mod lambda_permission;
mod lambda_version;
/// The Amazon CloudWatch Logs types a stack declares — a log group, a stream inside one and
/// a metric filter on one: visible to the rest of this service's state because the types'
/// own NAMES live here, beside the arms that provision them, and more than one module answers
/// about them ([`super::template`] reads a metric filter's own name property).
pub(super) mod logs_resources;
/// `AWS::CloudFormation::Stack`: the NESTED STACK a stack declares, created in
/// CloudFormation itself ([`nested_stack`]).
mod nested_stack;
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
mod step_functions_state_machine;

/// Custom resources: the provider a template names in a resource's `ServiceToken`, the
/// request CloudFormation sends it on every stack operation touching the resource, and the
/// answer the provider PUTs to the `ResponseURL` the request carries.
pub mod custom_resource;
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

/// The success signals the INSTANCES of a signalled resource send: what releases the
/// resource-signal wait of a resource whose signals come from the instances it consists of
/// ([`instance_signals`]).
mod instance_signals;

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
}

/// How a deployment puts back a resource whose change it could not carry through
/// ([`roll_back_failed_resource`]).
///
/// CloudFormation does not leave such a resource half-changed: the configuration in force
/// before the deployment is put back through the owning service's own update API. Which
/// statuses REPORT that revert is what the variants separate, because that follows from what
/// becomes of the stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FailedResourceRevert {
    /// Nothing is put back: this deployment IS the putting back (a rollback, a resumption, a
    /// revert), or it is a CREATE, where a resource that failed has no earlier configuration
    /// of its own to go back to.
    #[default]
    None,
    /// The revert belongs to the STACK's own rollback, which follows this deployment: a
    /// rollback reports every resource it puts back as that resource's own update reaching
    /// its terminal status, so the revert is reported as `UPDATE_IN_PROGRESS` and then
    /// `UPDATE_COMPLETE`.
    WithTheStackRollback,
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
    /// resource has at that point — the one it already carries for a change to a resource
    /// that exists, and none for a create, where the owning service has not answered yet.
    fn starting(resource: &StoredResource, status: &str) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: None,
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
    /// Every resource transition this deployment made, oldest first — the stack's
    /// per-resource event records.
    pub transitions: Vec<ResourceTransition>,
    /// The resources this deployment CREATED whose owning service does not report them
    /// ready yet, and the ones it DELETED whose owning service still holds them: the call
    /// was accepted and has not been carried out, so CloudFormation holds the resource — and
    /// the operation — open until that service says otherwise
    /// ([`super::stabilising`]).
    pub stabilising: Vec<Stabilising>,
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
    Failed(String),
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
    /// One of the inline-policy types [`iam_policy`] embeds in IAM (`AWS::IAM::Policy` and
    /// the per-entity `AWS::IAM::RolePolicy`, `UserPolicy` and `GroupPolicy`). Which of them
    /// it is travels with the owner, so the algorithm resolves the type once.
    IamPolicy(iam_policy::Kind),
    /// `AWS::Lambda::Alias`, whose arms are [`lambda_alias`]'s.
    LambdaAlias,
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
    /// One of the two Amazon Cognito Identity types [`cognito_identity_pool`] creates: the
    /// identity pool, and the role attachment that binds the pool's IAM roles to it. Which
    /// of them it is travels with the owner, as for the Amazon EC2 types above.
    CognitoIdentityPool(cognito_identity_pool::Kind),
    /// One of the three Amazon Cognito user-pool types [`cognito_user_pool`] creates: the
    /// pool, the app client that signs in against it, and the third-party identity provider
    /// it federates with. Which of them it is travels with the owner, as for the types above.
    CognitoUserPool(cognito_user_pool::Kind),
    /// `AWS::CloudFormation::Stack`, whose arms are [`nested_stack`]'s: the owning service is
    /// CloudFormation itself, so a nested stack is created by the same `CreateStack` a
    /// client calls.
    NestedStack,
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
    /// One of the `AWS::ApiGateway::*` types of that service's REST control plane, which
    /// [`apigateway_rest_api`] creates in Amazon API Gateway: the api, a resource of it, a
    /// method on that resource, a deployment of them, the stage that serves it, and the four
    /// that govern who reaches that stage — an authorizer, an api key, a usage plan and the
    /// association that puts a key in a plan. Which of them it is travels with the owner, as
    /// it does for the Amazon EC2 types above.
    ApiGatewayRestApi(apigateway_rest_api::Kind),
    /// One of the four `AWS::ElasticLoadBalancingV2::*` types [`elastic_load_balancing`]
    /// creates in Elastic Load Balancing: the load balancer, a target group, a listener on
    /// the load balancer and a rule of that listener. Which of them it is travels with the
    /// owner, as it does for the types above.
    ElasticLoadBalancing(elastic_load_balancing::Kind),
    /// One of the four `AWS::AppConfig::*` types [`appconfig_resources`] creates in AWS
    /// AppConfig: the application, an environment of it, a configuration profile of it and the
    /// deployment strategy a rollout follows. Which of them it is travels with the owner, as
    /// it does for the types above.
    AppConfigResource(appconfig_resources::Kind),
    /// One of the two Amazon CloudFront types [`cloudfront_resources`] creates: the
    /// distribution, and the origin access control its origins reach a private bucket with.
    /// Which of them it is travels with the owner, as it does for the types above.
    CloudFrontResource(cloudfront_resources::Kind),
    /// One of the two `AWS::ApplicationAutoScaling::*` types
    /// [`application_auto_scaling`] registers in Application Auto Scaling: the scalable
    /// target, and the scaling policy that steers one. Which of them it is travels with the
    /// owner, as it does for the types above.
    ApplicationAutoScaling(application_auto_scaling::Kind),
}

/// The owner of a template resource type, or `None` for a type this emulator leaves
/// declared-only.
pub(super) fn owner_for(resource_type: &str) -> Option<Owner> {
    if resource_type == dynamodb_table::DYNAMODB_TABLE {
        return Some(Owner::DynamodbTable);
    }
    if let Some(kind) = iam_policy::kind_of(resource_type) {
        return Some(Owner::IamPolicy(kind));
    }
    if resource_type == lambda_alias::LAMBDA_ALIAS {
        return Some(Owner::LambdaAlias);
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
    if let Some(kind) = ec2_vpc_network::kind_of(resource_type) {
        return Some(Owner::Ec2VpcNetwork(kind));
    }
    if resource_type == ec2_instance::EC2_INSTANCE {
        return Some(Owner::Ec2Instance);
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
    if let Some(kind) = apigateway_rest_api::kind_of(resource_type) {
        return Some(Owner::ApiGatewayRestApi(kind));
    }
    if let Some(kind) = application_auto_scaling::kind_of(resource_type) {
        return Some(Owner::ApplicationAutoScaling(kind));
    }
    if let Some(kind) = appconfig_resources::kind_of(resource_type) {
        return Some(Owner::AppConfigResource(kind));
    }
    if let Some(kind) = elastic_load_balancing::kind_of(resource_type) {
        return Some(Owner::ElasticLoadBalancing(kind));
    }
    if let Some(kind) = cloudfront_resources::kind_of(resource_type) {
        return Some(Owner::CloudFrontResource(kind));
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
            .filter(|stack| stack.region == region)
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
                    // out would report a resource as creating that nothing is creating.
                    Ok(Readiness::Failed(reason)) => {
                        stack.fail_stabilisation(now, &barrier, &reason)
                    }
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
                stack.fail_transition(SystemTime::now(), super::status::ROLLBACK_FAILED, failure);
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
    Box::pin(deferred::resume(ctx, state, region)).await;
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
            stack.fail_transition(SystemTime::now(), super::status::ROLLBACK_FAILED, failure);
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
                outcome.failure = Some(reason);
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
        repoint_references(&mut resource.properties, &minted);
        repoint_references(&mut deployed_properties, &minted);
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
        if let Some(existing) = previous
            .iter()
            .find(|existing| already_deployed(existing) && is_same_resource(existing, &resource))
        {
            // The stack already has exactly this resource and the deployment did not change
            // its identity, so nothing is created again; what the owning service reported
            // about it, and the configuration it really has, are carried forward.
            let declared_properties = resource.properties.clone();
            let changed = declared_properties != existing.properties;
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
                outcome.transitions.push(ResourceTransition::starting(
                    &resource,
                    super::resource_status::UPDATE_IN_PROGRESS,
                ));
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
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(reason.to_string()),
                        ));
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
                            outcome.transitions.push(ResourceTransition::finished(
                                &resource,
                                super::resource_status::UPDATE_FAILED,
                                Some(handler_failure_reason(
                                    None,
                                    &refusal,
                                    &resource.physical_resource_id,
                                )),
                            ));
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
                    outcome.transitions.push(ResourceTransition::finished(
                        &resource,
                        super::resource_status::UPDATE_FAILED,
                        Some(reason),
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
                let applied = match owner_for(&resource.resource_type) {
                    Some(provisioner) => {
                        // The call is the stack's, made under its service role and about
                        // THIS resource ([`super::service_role`]).
                        // BOXED, for the reason the create's own call below records: the
                        // call's future is the owning service's whole handler, and holding one
                        // of those inline in this walk's state machine is what takes a
                        // deployment of a wide template past the thread's stack.
                        Box::pin(super::service_role::on_resource(
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
                        .await
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
                            Ok(Readiness::Ready) => {}
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
                            }
                            // That service says the change is not going to take effect, so
                            // waiting it out would report a resource as changed that is not:
                            // the update of this resource failed, carrying what it said.
                            Ok(Readiness::Failed(reason)) => {
                                failed.push((
                                    resource.logical_id.clone(),
                                    super::failed_action::UPDATE,
                                ));
                                outcome.transitions.push(ResourceTransition::finished(
                                    &resource,
                                    super::resource_status::UPDATE_FAILED,
                                    Some(reason),
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
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_FAILED,
                            Some(unsupported),
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
                    Err(refusal) => {
                        // The owning service refused the change: nothing changed there, so
                        // the deployment reports that refusal rather than a configuration
                        // nothing applied.
                        failed.push((
                            resource.logical_id.clone(),
                            super::failed_action::UPDATE,
                        ));
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
                    if changed {
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            super::resource_status::UPDATE_COMPLETE,
                            None,
                        ));
                    }
                }
                Err(refusal) => {
                    if !changed {
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
        // A resource whose declaration reads something a resource it DEPENDS ON has not
        // published yet is not created in this pass. A CUSTOM RESOURCE's attributes are the
        // `Data` its provider answers with, so a declaration naming one through `Fn::GetAtt`
        // is not resolved until that answer is in hand, and creating the resource now would
        // ask its owning service for something the template does not describe. It is held at
        // a wait of its own ([`super::stabilising::Awaited::Deferred`]) and created by the
        // pass that runs when those waits release ([`deferred::resume`]) — which is the
        // order CloudFormation creates a stack's resources in: "a resource that depends on
        // another is scheduled only once that one has finished".
        if awaits_a_dependency(&resource.logical_id, &stack.dependencies, &unpublished) {
            unpublished.insert(resource.logical_id.clone());
            outcome.stabilising.push(Stabilising {
                awaited: super::stabilising::Awaited::Deferred,
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                physical_resource_id: resource.physical_resource_id.clone(),
                reported_state: super::resource_status::CREATE_IN_PROGRESS.to_string(),
                provider_request: None,
            });
            outcome.resources.push(resource);
            continue;
        }
        // Whether this create REPLACES the resource the stack already has under this logical
        // id: the record did not match one it has ([`is_same_resource`]), and yet the stack
        // holds a deployed resource of the same logical id and type — so the declaration asks
        // for a value the resource that exists cannot take, and what the deployment does is
        // create a new physical resource for it. That is reported as the UPDATE's own
        // transitions rather than as a first create ([`Reporting`]).
        let replacing = previous.iter().any(|existing| {
            already_deployed(existing)
                && existing.logical_id == resource.logical_id
                && existing.resource_type == resource.resource_type
        });
        let reporting = Reporting::of(replacing, stack.express);
        // The deployment is CREATING this resource: the stack records it as one it created
        // once this is through, whether the create reaches an owning service or not, so the
        // transition that opens it is recorded here.
        outcome.transitions.push(ResourceTransition::creating(
            &resource,
            reporting.in_progress,
            reporting.opening_reason,
        ));
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
            failed.push((
                resource.logical_id.clone(),
                reporting.action,
            ));
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
                failed.push((
                    resource.logical_id.clone(),
                    reporting.action,
                ));
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
                    failed.push((
                        resource.logical_id.clone(),
                        reporting.action,
                    ));
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
            failed.push((
                resource.logical_id.clone(),
                reporting.action,
            ));
            outcome.transitions.push(ResourceTransition::failed(
                &resource,
                reporting.failed,
                reason,
            ));
            outcome.resources.push(resource);
            continue;
        }
        // Which service owns the type, resolved ONCE here: the create below and the
        // readiness read that follows it are two questions for the same provisioner, and
        // one lookup is what keeps them from ever being asked of different ones.
        let owner = owner_for(&resource.resource_type);
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
                // BOXED, for the reason [`roll_back_failed_resource`]'s own call records: the
                // call's future is the owning service's whole handler, and holding one of
                // those inline in this walk's state machine is what takes a deployment of a
                // wide template past the thread's stack — the more so for a walk a READ
                // resumes, which is already several frames down ([`deferred::resume`]).
                Box::pin(super::service_role::on_resource(
                    super::service_role::target_of(&asked),
                    provisioner.create(ctx, region, &asked, &tags, &stack.capabilities),
                ))
                .await
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
                            stand_in: stand_in.clone(),
                            assigned: assigned.clone(),
                            // An attribute is an ARN, an endpoint or a url — a value a
                            // property carries WHOLE.
                            composed: false,
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
                let readiness = match owner {
                    Some(provisioner) => {
                        // Boxed for the reason the create's own call just above is.
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
                    Ok(Readiness::Ready) => {
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            reporting.completed,
                            None,
                        ));
                    }
                    Ok(Readiness::Pending(state)) => {
                        // The terminal transition is planned here as it is for any other
                        // resource; the gate this records is what holds it back until the
                        // owning service reports the resource ready.
                        outcome.stabilising.push(Stabilising {
                            awaited: super::stabilising::Awaited::Ready,
                            logical_id: resource.logical_id.clone(),
                            resource_type: resource.resource_type.clone(),
                            physical_resource_id: resource.physical_resource_id.clone(),
                            reported_state: state,
                            // The wait is the OWNING SERVICE's answer about the resource, so
                            // there is no provider request it is for.
                            provider_request: None,
                        });
                        // A resource that is still coming up has not published the values its
                        // owning service only reports once it IS up — the address a cluster
                        // serves on, the endpoint a stream is written to — so nothing that
                        // READS one of them may be created ahead of that answer
                        // ([`awaits_a_dependency`]): the document such a create would carry
                        // does not have the value yet. The dependents are held at waits of
                        // their own and created by the pass that runs once this one releases
                        // ([`deferred::resume`]), which is the order CloudFormation creates a
                        // stack's resources in — the same hold a custom resource whose
                        // provider has not answered yet puts on the resources that read its
                        // `Data`.
                        unpublished.insert(resource.logical_id.clone());
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            reporting.completed,
                            None,
                        ));
                    }
                    // The resource exists in that service and is not going to come up, or
                    // that service could not be asked about it at all: either way the
                    // create did not go through, so the deployment stops here exactly as it
                    // does on a refused create. The record keeps `materialised`, so the
                    // rollback still deletes what the create made.
                    Ok(Readiness::Failed(reason)) => {
                        failed.push((
                            resource.logical_id.clone(),
                            reporting.action,
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            reporting.failed,
                            Some(reason),
                        ));
                        outcome.resources.push(resource);
                        continue;
                    }
                    Err(refusal) => {
                        failed.push((
                            resource.logical_id.clone(),
                            reporting.action,
                        ));
                        outcome.transitions.push(ResourceTransition::finished(
                            &resource,
                            reporting.failed,
                            Some(handler_failure_reason(
                                owner,
                                &refusal,
                                &resource.physical_resource_id,
                            )),
                        ));
                        outcome.resources.push(resource);
                        continue;
                    }
                }
            }
            Err(refusal) => {
                failed.push((
                    resource.logical_id.clone(),
                    reporting.action,
                ));
                outcome.transitions.push(ResourceTransition::failed(
                    &resource,
                    reporting.failed,
                    // A name already in use that THIS REGION's stacks are what hold is
                    // reported in CloudFormation's own words, naming the stack that manages
                    // it; everything else the owning service refused is reported as that
                    // service worded it.
                    already_managed_reason(&resource, stack, &refusal).unwrap_or_else(|| {
                        handler_failure_reason(owner, &refusal, &resource.physical_resource_id)
                    }),
                ));
                outcome.resources.push(resource);
                continue;
            }
        }
        outcome.resources.push(resource);
    }
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
        repoint_references(&mut resource.properties, &minted);
        if let Some(existing) = previous
            .iter()
            .find(|existing| already_deployed(existing) && is_same_resource(existing, &resource))
        {
            carry_forward(existing, &mut resource);
        }
        outcome.resources.push(resource);
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
        // A create failed, so this deployment is not the one that replaces anything: the
        // resources it would have deleted are still there, and the stack keeps their
        // records so its own deletion still deletes them.
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
    };
    let mut deleted = delete_pass(ctx, region, &replacements, &replacement_walk, &mut opened).await;
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

/// How deep [`repoint_references`] walks a property document.
///
/// The document's nesting is the REQUESTER's, and a recursive walk bounded by its input
/// exhausts the native stack — which aborts the process rather than failing one stack. Past
/// this depth the values are left exactly as they are: a reference to another resource is a
/// property of it, never a hundred levels inside one.
const MAX_REPOINTED_NESTING: usize = 64;

/// Re-point every reference inside one property document from the stand-in identifier a
/// resource was given before its create to the identifier its owning service MINTED.
///
/// The rule: a reference to a resource resolves to the identifier that resource really has,
/// and for a type whose identity its owning service assigns
/// ([`super::template::identity_is_service_assigned`]) that identifier only exists once the
/// create has answered. A declaration is resolved before any of them have, so a property
/// naming such a resource (`ResolverRuleId: !Ref Rule`) holds the stand-in the naming rules
/// computed; `minted` pairs each stand-in with what its service answered, and a property
/// whose value IS one of those stand-ins becomes the minted identifier.
///
/// The same holds for an ATTRIBUTE the owning service reports differently from the computed
/// one (`KmsMasterKeyId: !GetAtt [Key, Arn]`, where the ARN carries the key id KMS minted):
/// [`reconcile`] pairs those too, so a property reaches its own service as the value the
/// resource it names really publishes.
///
/// Matched WHOLE value — or, where the stand-in is an identifier CloudFormation computed
/// rather than a name the declaration asked for ([`Minted::composed`]), as one whole TOKEN
/// of a longer value ([`retokenised`]), which is how a template composes an identifier into
/// one of its own strings (`integrations/<IntegrationId>`, or the ARN an `Fn::Sub` builds
/// around a `Ref`). Never as any shorter run of characters: a stand-in is often the NAME the
/// declaration asked for, so a longer value that merely contains one (an association named
/// `<rule name>-assoc`) is a value of its own and is left exactly as written.
///
/// Called by [`reconcile`] for the identifiers minted in the walk it is driving. A resource
/// is never re-pointed with its OWN pair — it cannot reference itself, and for a type whose
/// stand-in is the name it declares that pair would rewrite that very property. The
/// identifiers a stack ALREADY has are not substituted at all:
/// [`super::StoredStack::deploy_template`] resolves the declarations against them instead.
fn repoint_references(properties: &mut Value, minted: &[Minted]) {
    repoint(properties, minted, 0);
}

/// One identifier an owning service MINTED in a deployment walk, against the stand-in the
/// naming rules had computed for the same resource ([`repoint_references`]).
#[derive(Debug, Clone)]
struct Minted {
    /// What a reference to the resource was resolved to before its create.
    stand_in: String,
    /// What that reference has to carry now.
    assigned: String,
    /// Whether the stand-in may also be matched INSIDE a longer value, as one whole token
    /// of it ([`retokenised`]).
    ///
    /// True only where the stand-in is an identifier CloudFormation COMPUTED rather than
    /// the name the declaration asked for: such a value is not one a template composed out
    /// of its own strings, so a longer value carrying it carries it because a reference to
    /// this resource put it there. That is how an API Gateway v2 route names the
    /// integration it is wired to — `Target: !Join ['/', ['integrations', !Ref
    /// Integration]]`, which is the `integrations/<IntegrationId>` form that service
    /// accepts — and how a template builds the ARN of a resource it declares, with the
    /// reference inside a longer string (`!Sub 'arn:aws:execute-api:${AWS::Region}:\
    /// ${AWS::AccountId}:${Api}/*/*'`). Without this, such a route would be created
    /// pointing at an integration id nothing minted, and such a grant made for an api id
    /// nothing minted.
    composed: bool,
}

/// [`repoint_references`] over one value, `depth` levels into the document.
fn repoint(properties: &mut Value, minted: &[Minted], depth: usize) {
    if minted.is_empty() || depth >= MAX_REPOINTED_NESTING {
        return;
    }
    match properties {
        Value::String(text) => {
            if let Some(minted) = minted
                .iter()
                .find(|minted| minted.stand_in.as_str() == text.as_str())
            {
                *text = minted.assigned.clone();
                return;
            }
            for minted in minted.iter().filter(|minted| minted.composed) {
                if let Some(repointed) = retokenised(text, &minted.stand_in, &minted.assigned) {
                    *text = repointed;
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                repoint(item, minted, depth + 1);
            }
        }
        Value::Object(members) => {
            for value in members.values_mut() {
                repoint(value, minted, depth + 1);
            }
        }
        _ => {}
    }
}

/// Whether `character` can be part of one IDENTIFIER: an ASCII letter or digit, a hyphen or
/// an underscore, which is what the identifiers AWS services assign are made of.
///
/// It is the boundary [`retokenised`] matches a stand-in at: a stand-in whose neighbour in a
/// longer value is one of these is part of a longer token of that value, so it is not a
/// reference to the resource and is left exactly as written.
fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

/// `text` with every WHOLE-TOKEN occurrence of `stand_in` replaced by `assigned`, or `None`
/// for a text that carries none.
///
/// This is how a reference COMPOSED INTO a longer string reaches the identifier its resource
/// really has: a template builds the ARN of a resource it declares out of its own text and
/// that reference (`!Sub 'arn:aws:execute-api:${AWS::Region}:${AWS::AccountId}:${Api}/*/*'`),
/// so the identifier sits inside a longer value delimited by whatever the template wrote
/// around it — a colon, a slash, a quote, a space — rather than being the whole value or one
/// whole slash-separated segment of it.
///
/// A token is bounded by any character an identifier cannot carry
/// ([`is_identifier_character`]), so a stand-in that merely begins or ends a longer
/// identifier is left alone. Only a stand-in CloudFormation itself computed is matched this
/// way ([`Minted::composed`]): a value a declaration asked for is a value of its own
/// wherever it appears.
fn retokenised(text: &str, stand_in: &str, assigned: &str) -> Option<String> {
    if stand_in.is_empty() {
        return None;
    }
    let mut repointed = String::new();
    let mut cursor = 0;
    let mut replaced = false;
    while let Some(offset) = text[cursor..].find(stand_in) {
        let at = cursor + offset;
        let end = at + stand_in.len();
        let bounded = text[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !is_identifier_character(character))
            && text[end..]
                .chars()
                .next()
                .is_none_or(|character| !is_identifier_character(character));
        repointed.push_str(&text[cursor..at]);
        if bounded {
            repointed.push_str(assigned);
            replaced = true;
        } else {
            repointed.push_str(stand_in);
        }
        cursor = end;
    }
    if !replaced {
        return None;
    }
    repointed.push_str(&text[cursor..]);
    Some(repointed)
}

/// Record the identity an owning service ASSIGNED to a resource it created: the id its
/// `Ref` resolves to from here on, and the `Id`/`<Type>Id` attributes read off that id,
/// which the naming rules had computed from the name instead.
///
/// Taking an identifier over is [`super::template::adopt_identity`], the one place that
/// step is written — the same one a binding's composite primary identifier goes through —
/// so the attributes and the id a resource reports cannot drift apart whichever of them
/// produced it.
///
/// `None` is every type whose physical id CloudFormation computes, which the create already
/// carried out to that service: nothing to adopt.
fn adopt_minted_identity(resource: &mut StoredResource, assigned: Option<String>) {
    let Some(assigned) = assigned else {
        return;
    };
    super::template::adopt_identity(resource, assigned);
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
/// recompute it.
pub fn is_same_resource(left: &StoredResource, right: &StoredResource) -> bool {
    left.logical_id == right.logical_id
        && left.resource_type == right.resource_type
        && left.physical_resource_id == right.physical_resource_id
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
fn already_deployed(existing: &StoredResource) -> bool {
    existing.materialised || provisioner_for(&existing.resource_type).is_none()
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
    delete_pass(ctx, region, resources, walk, &mut BTreeSet::new()).await
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
}

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

/// The stack-level reason a deployment whose RESOURCES failed reports, or `None` for a walk
/// that failed none of them.
///
/// One summary sentence per verb the failures fall under
/// ([`super::resource_failure_summary`]), each naming every resource that failed under that
/// verb in the order the walk reached them. A deployment attempts every resource whose
/// dependencies allow it, so more than one can have failed — and the field a client parses
/// to recover the set of failed logical ids has to name all of them. One deployment can be
/// doing two things at once (creating one resource while changing another), and then each
/// verb reports its own set, which is what keeps the sentence true of the resources it names.
fn failed_resource_summary(failed: &[(String, &'static str)]) -> Option<String> {
    let mut verbs: Vec<&'static str> = Vec::new();
    for (_, action) in failed {
        if !verbs.contains(action) {
            verbs.push(action);
        }
    }
    let mut summary = String::new();
    for verb in verbs {
        let named = failed
            .iter()
            .filter(|(_, action)| *action == verb)
            .map(|(logical_id, _)| logical_id.as_str());
        summary.push_str(&super::resource_failure_summary(verb, named));
    }
    (!summary.is_empty()).then_some(summary)
}

/// Put ONE resource whose change this deployment could not carry through back the way it
/// was, through the owning service's own update API, and report that revert on the
/// resource's own transitions.
///
/// CloudFormation does not leave a resource whose provider refused its change half-changed:
/// the resource is rolled back to the configuration in force before the deployment. WHICH
/// statuses report that revert follows from what becomes of the stack
/// ([`FailedResourceRevert`]), and a deployment that is itself a revert puts nothing back.
///
/// `in_force` is the record of the same resource the stack had BEFORE this deployment, whose
/// properties are the configuration being put back; a refusal of the change never applied it,
/// so for most failures the call confirms the configuration rather than changing it — which
/// is what the owning service's answer is asked for. The record is left stating that
/// configuration, and a service that would not take it leaves the resource in the failed
/// status with its own refusal, never a revert nothing carried out.
async fn roll_back_failed_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    stack: &StackProvenance,
    resource: &mut StoredResource,
    in_force: &StoredResource,
    outcome: &mut Materialised,
) {
    // The statuses this revert is reported under, which is what the deployment's own mode
    // says: a stack that goes on to roll back reports every resource it puts back as that
    // resource's own update reaching its terminal status, while one that rests in the failed
    // status has only the resource's own rollback transitions to report it.
    let (opened, ended, refused) = match stack.failed_resource_revert {
        FailedResourceRevert::None => return,
        FailedResourceRevert::WithTheStackRollback => (
            super::resource_status::UPDATE_IN_PROGRESS,
            super::resource_status::UPDATE_COMPLETE,
            super::resource_status::UPDATE_FAILED,
        ),
        FailedResourceRevert::OfTheResourceAlone => (
            super::resource_status::UPDATE_ROLLBACK_IN_PROGRESS,
            super::resource_status::UPDATE_ROLLBACK_COMPLETE,
            super::resource_status::UPDATE_ROLLBACK_FAILED,
        ),
    };
    outcome
        .transitions
        .push(ResourceTransition::starting(resource, opened));
    let restored = in_force.properties.clone();
    let owner = owner_for(&resource.resource_type);
    let put_back = match owner {
        Some(provisioner) => {
            // BOXED, like every other provider call this walk makes from inside a future
            // this big: the call's own future is the owning service's whole handler, and
            // holding a second one of those inline in [`reconcile`]'s state machine is what
            // takes a deployment of a wide template past the thread's stack.
            Box::pin(super::service_role::on_resource(
                super::service_role::target_of(resource),
                provisioner.update(
                    ctx,
                    region,
                    resource,
                    &restored,
                    &stack.tags_for(&resource.logical_id),
                    &stack.capabilities,
                ),
            ))
            .await
        }
        // Nothing exists in any owning service for this type, so there is nothing anywhere
        // to put back: the stack's own record IS the resource, and restoring the record
        // below is the whole revert.
        None => Ok(Some(Created::default())),
    };
    match put_back {
        // Either the owning service took the previous configuration, or the type has no
        // update path at all and so was never changed: the resource carries the
        // configuration it had before this deployment, which is what the record states.
        Ok(_) => {
            resource.properties = restored;
            outcome
                .transitions
                .push(ResourceTransition::finished(resource, ended, None));
        }
        Err(refusal) => {
            outcome.transitions.push(ResourceTransition::finished(
                resource,
                refused,
                Some(handler_failure_reason(
                    owner,
                    &refusal,
                    &resource.physical_resource_id,
                )),
            ));
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
/// attribute instead ([`Policies::Replaced`]), which takes the same three values and whose
/// documented default is to delete the resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policies {
    /// Each resource's own `DeletionPolicy` decides what becomes of it.
    Honoured,
    /// Each resource's own `UpdateReplacePolicy` decides what becomes of it: this walk is
    /// taking down the old physical resource of a replacement.
    Replaced,
    /// The physical resource is deleted whatever either attribute states.
    Ignored,
}

/// What this walk does with one resource: the attribute the walk reads for it, and the
/// documented default ([`super::deletion_policy::DELETE`]) otherwise — which is also what a
/// declaration naming no policy, and one naming a value that is none of the three, gets.
fn policy_of(resource: &StoredResource, policies: Policies) -> &str {
    match policies {
        Policies::Ignored => super::deletion_policy::DELETE,
        Policies::Honoured => resource
            .deletion_policy
            .as_deref()
            .unwrap_or(super::deletion_policy::DELETE),
        Policies::Replaced => resource
            .update_replace_policy
            .as_deref()
            .unwrap_or(super::deletion_policy::DELETE),
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

/// One pass of [`delete`] over the set, recording what it transitioned.
///
/// `opened` is the logical ids whose deletion has already been OPENED, which this pass adds
/// to: each resource's `DELETE_IN_PROGRESS` is recorded the first time a pass reaches it,
/// and a RE-ASK of one an owning service has not carried out yet ([`unwind`]) is the same
/// deletion still in progress rather than a second one. A resource an earlier pass never
/// reached — one after the refusal that stopped it — is opened by the pass that does reach
/// it.
///
/// `policies` is whether this walk reads each resource's own `DeletionPolicy` ([`Policies`]).
async fn delete_pass(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    walk: &Teardown<'_>,
    opened: &mut BTreeSet<String>,
) -> Deleted {
    let Teardown {
        stack_id,
        held,
        needs,
        on_refusal,
        policies,
    } = *walk;
    let mut outcome = Deleted {
        resources: resources.to_vec(),
        failure: None,
        transitions: Vec::new(),
        removing: Vec::new(),
    };
    // The resources an owning service REFUSED to delete, and everything those resources need
    // ([`needed_by`]): the refusals are what the teardown reports as its failure, and what
    // they need is what it leaves alone, because a resource that is still there is one
    // whatever it depends on is still in use by.
    let mut refused: Vec<String> = Vec::new();
    let mut still_needed: BTreeSet<String> = BTreeSet::new();
    // Newest first, over the indices of the set being reported back, so each resource's
    // record is updated in place as its deletion succeeds.
    for index in (0..outcome.resources.len()).rev() {
        let resource = &outcome.resources[index];
        if still_needed.contains(&resource.logical_id) {
            // A resource that one the owning service would not delete still needs: deleting
            // it is not something that service could carry out while that resource is there,
            // so nothing is asked about it and the stack keeps its record — a later deletion
            // is what deletes it.
            continue;
        }
        // Which service is asked to delete the resource. `None` is a type this emulator
        // provisions nowhere: there is no API to call for it.
        let owner = owner_for(&resource.resource_type);
        if !resource.materialised {
            // Nothing exists in any owning service for this resource, so nothing is asked
            // of anybody. The stack still HELD the record, though, and its teardown is
            // reported exactly as every other resource's is — whether an owning service
            // had one to delete is not something a stack's event stream says, and the
            // record this disposal leaves says `DELETE_COMPLETE` either way
            // ([`super::stack::disposed_resources`]), so a resource whose create FAILED
            // goes through the delete pair as well. Two records have no teardown at all:
            // one the stack does not hold (a declaration the operation never reached) and
            // one an earlier pass of this same walk already took down.
            if !held.contains(&resource.logical_id) || opened.contains(&resource.logical_id) {
                continue;
            }
        }
        let policy = policy_of(resource, policies);
        if policy == super::deletion_policy::RETAIN {
            // "CloudFormation keeps the resource without deleting the resource or its
            // contents when its stack is deleted": nothing at all is asked of the owning
            // service, so the resource stays exactly as it is and goes on serving its own
            // clients, and the one thing the stack records about it is that its deletion was
            // SKIPPED — once, however many passes of one walk reach the record. The record
            // stops carrying `materialised`: the stack has RELEASED the resource, so no
            // later deletion of this stack's own owes a call about it.
            let first = opened.insert(resource.logical_id.clone());
            outcome.resources[index].materialised = false;
            if first {
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_SKIPPED,
                    None,
                ));
            }
            continue;
        }
        if opened.insert(resource.logical_id.clone()) {
            outcome.transitions.push(ResourceTransition::starting(
                resource,
                super::resource_status::DELETE_IN_PROGRESS,
            ));
        }
        // "For resources that support snapshots, CloudFormation creates a snapshot for the
        // resource before deleting it": for a type whose owning API takes that snapshot as a
        // member of the delete it rides on that call
        // ([`super::provisioner::Provisioner::delete`]), and for one whose snapshot is a CALL
        // OF ITS OWN it is made here, BEFORE the delete — so the snapshot outlives the
        // resource either way. A type whose owning service takes no snapshot on the way out
        // is deleted as it is for the default policy: there is no snapshot of it to take.
        let final_snapshot = (policy == super::deletion_policy::SNAPSHOT && resource.materialised)
            .then(|| final_snapshot_identifier(resource));
        if let Some(identifier) = final_snapshot.as_deref() {
            let resource = &outcome.resources[index];
            let snapshotted = super::service_role::on_resource(
                super::service_role::target_of(resource),
                take_final_snapshot(ctx, region, resource, identifier),
            )
            .await;
            if let Err(refusal) = snapshotted {
                // The snapshot the policy asked for could not be taken, so the resource is
                // NOT deleted: deleting it would destroy the data the policy exists to
                // preserve. The refusal is reported exactly as a refused delete is.
                let reason = handler_failure_reason(
                    owner,
                    &refusal,
                    &outcome.resources[index].physical_resource_id,
                );
                if on_refusal == OnRefusal::Abandon {
                    // ABANDONED: the stack has RELEASED the resource, exactly as a `Retain`
                    // policy releases one, so the record stops carrying `materialised` —
                    // this teardown owes no further call about it and the stack's deletion
                    // completes with it left exactly as it is.
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        super::resource_status::DELETE_SKIPPED,
                        Some(reason),
                    ));
                    outcome.resources[index].materialised = false;
                    continue;
                }
                refused.push(resource.logical_id.clone());
                still_needed.extend(needed_by(needs, &resource.logical_id));
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_FAILED,
                    Some(reason),
                ));
                continue;
            }
        }
        let resource = &outcome.resources[index];
        // A CUSTOM RESOURCE is removed by its own PROVIDER: the `Delete` request carries the
        // physical resource the provider minted and the properties it was last declared
        // with, and the removal is not finished until the provider answers
        // ([`custom_resource`]). `None` for every other resource, whose removal is the
        // owning service's own delete.
        let provider_delete = (resource.materialised
            && custom_resource::is_custom_resource(&resource.resource_type))
            .then(custom_resource::new_request_id);
        // ...and the owning service is asked ONLY about a resource that really exists: a
        // record nothing created has no resource anywhere to delete, and the identifier it
        // carries may be another stack's (the name its create was refused for), which is
        // that stack's to keep.
        let asked = owner.filter(|_| resource.materialised);
        let deleted = match (&provider_delete, asked) {
            (Some(request_id), _) => {
                let call = custom_resource::ProviderCall {
                    request_type: custom_resource::request_type::DELETE,
                    request_id,
                    stack_id,
                    logical_resource_id: &resource.logical_id,
                    resource_type: &resource.resource_type,
                    properties: &resource.properties,
                    physical_resource_id: Some(&resource.physical_resource_id),
                    // A `Delete` names no previous declaration: the properties it carries
                    // ARE the ones the resource was last declared with.
                    old_properties: None,
                };
                custom_resource::send(ctx, region, &call)
                    .await
                    .map(|response_url| Some(call.remembered(&response_url)))
            }
            (None, Some(provisioner)) => {
                // BOXED, for the reason the deployment walk's own calls are: the call's future
                // is the owning service's whole handler, and holding one of those inline in
                // this walk's state machine is what takes a teardown of a wide stack past the
                // thread's stack.
                Box::pin(super::service_role::on_resource(
                    super::service_role::target_of(resource),
                    provisioner.delete(ctx, region, resource, final_snapshot.as_deref()),
                ))
                .await
                .map(|()| None)
            }
            (None, None) => Ok(None),
        };
        match deleted {
            Ok(provider_request) => {
                // The delete was ACCEPTED; whether the resource is gone is the owning
                // service's to say. A type whose delete that service documents as
                // asynchronous leaves the resource in a state it is still being removed from
                // and answers which one ([`super::provisioner::Provisioner::removal`]), and
                // the operation is then held open until that service no longer holds it
                // ([`super::stabilising`]) — CloudFormation does not report an operation
                // complete while a resource it deleted is still there. An unanswered read is
                // no evidence that the resource is still there, so it opens no wait: the
                // delete itself went through.
                let removal = match (&provider_request, asked) {
                    // The provider has been SENT the `Delete`; the resource is gone when it
                    // answers, which is a wait like any other.
                    (Some(_), _) => Ok(Removal::Pending(
                        super::resource_status::DELETE_IN_PROGRESS.to_string(),
                    )),
                    (None, Some(provisioner)) => {
                        // Boxed for the reason the delete's own call above is.
                        Box::pin(provisioner.removal(
                            ctx,
                            region,
                            &outcome.resources[index].physical_resource_id,
                        ))
                        .await
                    }
                    (None, None) => Ok(Removal::Gone),
                };
                if let Ok(Removal::Pending(state)) = removal {
                    let resource = &outcome.resources[index];
                    outcome.removing.push(Stabilising {
                        awaited: super::stabilising::Awaited::Gone,
                        logical_id: resource.logical_id.clone(),
                        resource_type: resource.resource_type.clone(),
                        physical_resource_id: resource.physical_resource_id.clone(),
                        reported_state: state,
                        provider_request,
                    });
                    // ...and the resource is STILL THERE until that answer comes, so
                    // everything it depends on is left alone exactly as a refusal leaves it:
                    // "CloudFormation deletes the resources in reverse order", and a resource
                    // whose removal has only been ACCEPTED has not been removed — deleting
                    // what it depends on now would take away something still in use. The
                    // teardown asks about them once this wait releases
                    // ([`settle_outstanding_teardowns`]).
                    still_needed.extend(needed_by(needs, &resource.logical_id));
                }
                outcome.resources[index].materialised = false;
                let resource = &outcome.resources[index];
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_COMPLETE,
                    None,
                ));
            }
            Err(refusal) => {
                let reason = handler_failure_reason(
                    owner_for(&outcome.resources[index].resource_type),
                    &refusal,
                    &outcome.resources[index].physical_resource_id,
                );
                if on_refusal == OnRefusal::Abandon {
                    // ABANDONED: nothing more is asked of the owning service, so the
                    // resource stays exactly as it is and goes on serving its own clients.
                    // The record reports that its deletion was SKIPPED, carrying why, and
                    // the walk carries on with the resources declared before it — the
                    // teardown has no failure of its own, so the stack's deletion completes.
                    // The stack has RELEASED the resource, exactly as a `Retain` policy
                    // releases one, so the record stops carrying `materialised`: this
                    // teardown owes no further call about it.
                    let resource = &outcome.resources[index];
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        super::resource_status::DELETE_SKIPPED,
                        Some(reason),
                    ));
                    outcome.resources[index].materialised = false;
                    continue;
                }
                if on_refusal == OnRefusal::Orphan {
                    // ORPHANED: the refusal is reported on the resource and is not this
                    // teardown's failure — the change this cleanup follows is already in
                    // force, so there is nothing for the refusal to undo. The stack stops
                    // recording the resource, which is what leaves the old physical resource
                    // behind rather than held under a logical id the stack now records a
                    // different resource under.
                    outcome.resources[index].materialised = false;
                    let resource = &outcome.resources[index];
                    outcome.transitions.push(ResourceTransition::finished(
                        resource,
                        super::resource_status::DELETE_FAILED,
                        Some(reason),
                    ));
                    continue;
                }
                // The refusal is this teardown's FAILURE, and the resource is reported as the
                // one it failed on. The walk goes on with the resources that do not depend on
                // it: CloudFormation deletes what it can and reports what it could not, so a
                // sibling the refused resource has nothing to do with is deleted rather than
                // left behind by the refusal of another.
                let resource = &outcome.resources[index];
                refused.push(resource.logical_id.clone());
                still_needed.extend(needed_by(needs, &resource.logical_id));
                outcome.transitions.push(ResourceTransition::finished(
                    resource,
                    super::resource_status::DELETE_FAILED,
                    Some(reason),
                ));
            }
        }
    }
    if !refused.is_empty() {
        outcome.failure = Some(super::resource_failure_summary(
            super::failed_action::DELETE,
            refused.iter().map(String::as_str),
        ));
    }
    outcome
}

/// How many times the UNWIND of a failed operation asks the owning services to delete
/// what that operation had already made.
///
/// A service that answers a create while the resource is still coming up refuses to
/// delete it until it is up (Amazon Data Firehose answers `DeleteDeliveryStream` for a
/// stream it is still creating with "cannot be deleted in CREATING state"), and
/// CloudFormation's rollback is what waits for that: `ROLLBACK_IN_PROGRESS` is a status a
/// stack sits in until the resources are gone. The wait is BOUNDED — these attempts, one
/// [`UNWIND_RETRY_DELAY`] apart — so a service that refuses the delete for any other
/// reason is reported rather than waited on forever.
///
/// The count is what makes the bound cover a resource that is merely coming up: together
/// with [`UNWIND_RETRY_DELAY`] it has to outlast the longest span a called service gives
/// one of those (a few seconds — Amazon Data Firehose gives a stream three), or a rollback
/// would report a resource it could have removed as one it could not.
const UNWIND_ATTEMPTS: usize = 20;

/// How long the unwind waits before asking again. Chosen: shorter than the status spans
/// the services this emulator calls give a resource that is coming up, so the first
/// attempt after one of those elapses carries the delete through.
const UNWIND_RETRY_DELAY: ::std::time::Duration = ::std::time::Duration::from_millis(200);

/// Delete what a FAILED create had already made, for the stack operation that is rolling
/// back — [`delete`], re-asked while an owning service has not let a resource go yet.
///
/// The resources still carrying `materialised` are the ones left to delete, which is what
/// each re-ask narrows to, so nothing is asked for twice. [`Deleted::failure`] is what the
/// LAST attempt answered: a refusal that outlives the bound above is the rollback's own
/// failure, reported on the stack rather than passed off as a rollback that removed what
/// it did not.
///
/// `held` is the set [`delete`] documents: the records the stack holds, which is what this
/// takes down and reports. A rollback is the stack giving up the resources the failed
/// operation had made, so what becomes of each PHYSICAL resource is its own
/// `DeletionPolicy` — unless the operation was asked for `RetainExceptOnCreate`, the one
/// member documented to override that policy for a resource the operation itself created
/// ([`super::StoredStack::rolled_back_resource_policies`], which is what `policies` carries
/// here).
pub async fn unwind(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resources: &[StoredResource],
    walk: &Teardown<'_>,
) -> Deleted {
    // The deletions already opened, carried across the attempts: a re-ask of one still in
    // flight does not open it again, and one a stopped pass never reached is opened by the
    // pass that does reach it.
    let mut opened = BTreeSet::new();
    // An unwind is not a `DeleteStack`, so no `DeletionMode` chose for it: a refusal that
    // outlives the retries is the rollback's own failure.
    let mut outcome = delete_pass(ctx, region, resources, walk, &mut opened).await;
    for _ in 1..UNWIND_ATTEMPTS {
        if outcome.failure.is_none() {
            break;
        }
        ::tokio::time::sleep(UNWIND_RETRY_DELAY).await;
        let remaining = ::std::mem::take(&mut outcome.resources);
        // What the earlier attempts transitioned happened BEFORE what this one does. The
        // refusal this attempt is about to retry was not that resource's OUTCOME — its
        // deletion is still in progress, which is the whole point of the re-ask — so the
        // failed terminal the last pass recorded is dropped, and only the terminal of the
        // attempt nothing follows stands.
        let mut transitioned: Vec<ResourceTransition> =
            ::std::mem::take(&mut outcome.transitions)
                .into_iter()
                .filter(|transition| {
                    transition.status != super::resource_status::DELETE_FAILED
                })
                .collect();
        // The waits the earlier attempts opened are the deletions they carried through, and
        // they stand: this attempt is about the resources those passes could NOT delete.
        let mut removing = ::std::mem::take(&mut outcome.removing);
        outcome = delete_pass(ctx, region, &remaining, walk, &mut opened).await;
        transitioned.append(&mut outcome.transitions);
        outcome.transitions = transitioned;
        removing.append(&mut outcome.removing);
        outcome.removing = removing;
    }
    outcome
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

/// The SDK service NAME the diagnostic below names a called service by, per SIGNING name.
///
/// A resource provider calls the service that owns the type through an AWS SDK, and what it
/// reports when that call is refused is the SDK's exception message: the service's own
/// message followed by the SDK's own diagnostic of the call ([`sdk_diagnostic`]). The
/// service in that diagnostic is the SDK's name for it, which is NOT the signing name the
/// request was signed with — Amazon OpenSearch Service signs `es` and its SDK names it
/// `OpenSearch`, that model's own `sdkId`.
///
/// One entry per service whose reported wording this emulator pins; a service with no entry
/// has its message reported exactly as it worded it, with no diagnostic invented for it.
/// Each entry also carries the SHAPE that service's SDK words the diagnostic in
/// ([`SdkDiagnostic`]), which is the SDK's and cannot be derived from the service.
const SDK_SERVICE_NAMES: &[(&str, &str, SdkDiagnostic)] = &[
    // AWS AppConfig's SDK names it by that model's own `sdkId`, which is also the
    // `cloudFormationName` the type names of its resources carry
    // (`smithy_query(query="service_info")` on that service's model: `sdkId: "AppConfig"`,
    // `cloudFormationName: AppConfig`). The shape is the bare ENVELOPE, with no attempt count
    // after it, as the recorded `ResourceStatusReason` of an `AWS::AppConfig::Application`
    // whose declared name is longer than that service's maximum carries it (`"<the service's
    // sentence> (Service: AppConfig, Status Code: 400, Request ID: …)"`, where Amazon Route 53's
    // below carries `(SDK Attempt Count: …)` past the closing parenthesis). The four resource
    // types this emulator provisions over this signing name are all that service's
    // ([`provisioning::appconfig_resources`]).
    ("appconfig", "AppConfig", SdkDiagnostic::Envelope),
    // Application Auto Scaling's SDK names it by its `sdkId` without the spaces, which is
    // also the `cloudFormationName` the type names of its resources carry
    // (`smithy_query(query="service_info")`).
    (
        "application-autoscaling",
        "ApplicationAutoScaling",
        SdkDiagnostic::Attempts,
    ),
    // Amazon Braket's SDK names it by that model's own `sdkId` (`Braket`), and words its
    // diagnostic with the refused OPERATION and that service's error code in it, as the
    // recorded `ResourceStatusReason` of a failed `AWS::Braket::SpendingLimit` carries it (in
    // the behavioral corpus `simparity-behavioral-aws-v1`, check
    // `braket__cloudformation.deploy-braket-spendinglimit-simulator-device-rejected`, whose
    // reason reads `Resource handler returned message: "<the service's sentence> (Service:
    // Braket; Operation: CreateSpendingLimit; Status Code: 400; Error Code:
    // ValidationException; Request ID: …)" (RequestToken: …, HandlerErrorCode:
    // InvalidRequest)`).
    ("braket", "Braket", SdkDiagnostic::Operation),
    // Amazon EC2's SDK names it by that model's own `sdkId` (`EC2`,
    // `smithy_query(query="service_info")` on that service's model) written as one word with a
    // capital initial, exactly as Amazon RDS's below is, and words the diagnostic with the
    // refused call's error CODE and the PROXY it was made through — as the recorded
    // `ResourceStatusReason` of an `AWS::EC2::LaunchTemplate` whose declared name Amazon EC2
    // already holds a template under carries it (in the behavioral corpus, check
    // `cloudformation__ec2.deploy-ec2-launch-template-duplicate-name-rejected`, whose reason
    // reads `Resource handler returned message: "<the service's sentence> (Service: Ec2;
    // Status Code: 400; Error Code: InvalidLaunchTemplateName.AlreadyExistsException; Request
    // ID: …; Proxy: null)" (RequestToken: …, HandlerErrorCode: AlreadyExists)`). Every type
    // this emulator provisions over this signing name is that service's.
    ("ec2", "Ec2", SdkDiagnostic::Proxied),
    // Elastic Load Balancing v2's SDK names it `ElasticLoadBalancingV2` — that model's own
    // `awsService.cloudFormationName`, which is also the name its SDK client carries
    // (`smithy_query(query="service_info")`: `sdkId: "Elastic Load Balancing v2"`,
    // `cloudFormationName: ElasticLoadBalancingV2`). The four resource types this emulator
    // provisions over this signing name are all that service's
    // ([`provisioning::elastic_load_balancing`]), and the shape is the bare ENVELOPE, with no
    // attempt count after it, as the recorded `ResourceStatusReason` of an
    // `AWS::ElasticLoadBalancingV2::ListenerRule` whose priority the listener already
    // evaluates a rule at carries it (`"<the service's sentence> (Service:
    // ElasticLoadBalancingV2, Status Code: 400, Request ID: …)"`, where Amazon Route 53's
    // below carries `(SDK Attempt Count: …)` past the closing parenthesis).
    (
        "elasticloadbalancing",
        "ElasticLoadBalancingV2",
        SdkDiagnostic::Envelope,
    ),
    ("es", "OpenSearch", SdkDiagnostic::Attempts),
    ("iam", "Iam", SdkDiagnostic::Attempts),
    ("lambda", "Lambda", SdkDiagnostic::Attempts),
    // Amazon RDS's SDK names it by that model's own `sdkId` (`RDS`) written as one word with
    // a capital initial, and words the bare ENVELOPE — the service, the status and the call's
    // id, with nothing after it — as the recorded `ResourceStatusReason` of an
    // `AWS::RDS::DBSubnetGroup` whose subnets cover too few Availability Zones carries it (in
    // the behavioral corpus, check
    // `cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative`, whose
    // reason reads `Resource handler returned message: "<the service's sentence> (Service:
    // Rds, Status Code: 400, Request ID: …)" (RequestToken: …, HandlerErrorCode:
    // InvalidRequest)`).
    ("rds", "Rds", SdkDiagnostic::Envelope),
    // Amazon Route 53's SDK names it by its `sdkId` without the space, exactly as
    // Application Auto Scaling's does (`smithy_query(query="service_info")`: `sdkId:
    // "Route 53"`). The shape is the ATTEMPT-counting one, as the recorded
    // `ResourceStatusReason` of an `AWS::Route53::RecordSet` whose declared zone Route 53
    // does not hold carries it (`(Service: Route53, Status Code: 404, Request ID: …) (SDK
    // Attempt Count: …)`).
    ("route53", "Route53", SdkDiagnostic::Attempts),
    ("s3", "S3", SdkDiagnostic::Attempts),
];

/// The SHAPE of the parenthesis an SDK appends to the message of the exception it raises for
/// a refused call.
///
/// Each names the service and the HTTP status; what differs is what else they carry and how
/// the parts are separated. Which of them a called service's SDK words is that SDK's, and it
/// is not derivable from the service, so it is RECORDED per service in
/// [`SDK_SERVICE_NAMES`] — each entry citing where that shape was read from — and a service
/// with no entry has no diagnostic invented for it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SdkDiagnostic {
    /// `(Service: <service>, Status Code: <status>, Request ID: <id>) (SDK Attempt Count:
    /// <n>)` — comma-separated, with the call's id (or ids) and how many attempts the SDK
    /// made at it.
    Attempts,
    /// `(Service: <service>; Operation: <operation>; Status Code: <status>; Error Code:
    /// <code>; Request ID: <id>)` — semicolon-separated, naming the operation that was
    /// refused and the error code it was refused with in place of the attempt count.
    Operation,
    /// `(Service: <service>, Status Code: <status>, Request ID: <id>)` — the same
    /// comma-separated envelope [`SdkDiagnostic::Attempts`] opens with, and NOTHING after it:
    /// the SDK of such a service words no attempt count at all.
    Envelope,
    /// `(Service: <service>; Status Code: <status>; Error Code: <code>; Request ID: <id>;
    /// Proxy: null)` — semicolon-separated like [`SdkDiagnostic::Operation`], naming the error
    /// code the call was refused with but not the call, and closing with the PROXY the SDK made
    /// the request through: `null` for the direct call this emulator's in-process dispatch is.
    Proxied,
}

/// The services whose answers carry an EXTENDED request id beside the request id, which
/// their SDK states in the diagnostic too.
///
/// Amazon S3 is the service that answers one: every S3 response carries `x-amz-id-2` beside
/// `x-amz-request-id`, and the exception an S3 SDK raises states both ("Request ID: ...,
/// Extended Request ID: ..."). A service with no entry answers one id, and its diagnostic
/// states that one.
const SDK_EXTENDED_REQUEST_ID_SERVICES: &[&str] = &["s3"];

/// How many characters the request id of a service that answers an extended one carries, and
/// the case it is written in: Amazon S3's `x-amz-request-id` is a short opaque token in
/// upper case, where the request id of the services above is a uuid
/// ([`super::new_request_id`]).
const SDK_SHORT_REQUEST_ID_CHARACTERS: usize = 16;

/// How many characters the EXTENDED request id carries: a long opaque token, which is what
/// `x-amz-id-2` is.
const SDK_EXTENDED_REQUEST_ID_CHARACTERS: usize = 64;

/// The request id — and the extended request id where the service answers one — that the
/// diagnostic states for a call to `signing_name`.
///
/// Both are this call's own, minted per call as every identifier this service hands out is:
/// the in-process answer carries none of its own, and they identify the CALL rather than
/// anything about the resource.
fn sdk_request_ids(signing_name: &str) -> String {
    if !SDK_EXTENDED_REQUEST_ID_SERVICES.contains(&signing_name) {
        return format!("Request ID: {}", super::new_request_id());
    }
    format!(
        "Request ID: {}, Extended Request ID: {}",
        super::new_identifier_digits("sdk-request", SDK_SHORT_REQUEST_ID_CHARACTERS)
            .to_uppercase(),
        super::new_identifier_digits(
            "sdk-extended-request",
            SDK_EXTENDED_REQUEST_ID_CHARACTERS
        ),
    )
}

/// The diagnostic an AWS SDK appends to the message of the exception it raises for a refused
/// call: which service refused it, the HTTP status it answered with, the id (or ids) of the
/// call and how many attempts the SDK made at it.
///
/// `None` for a service [`SDK_SERVICE_NAMES`] does not name, which leaves the message as
/// that service worded it — and for a diagnostic that NAMES the operation when the caller
/// named none, since an operation is not something to invent.
///
/// `operation` is the called operation as its own model names it, which only a caller that
/// knows which operation it is making can state ([`call_rest_json_named`]).
fn sdk_diagnostic(
    signing_name: &str,
    operation: Option<&str>,
    status: u16,
    code: &str,
) -> Option<String> {
    let (service, shape) = SDK_SERVICE_NAMES
        .iter()
        .find(|(name, _, _)| *name == signing_name)
        .map(|(_, service, shape)| (*service, *shape))?;
    match shape {
        SdkDiagnostic::Attempts => Some(format!(
            "(Service: {service}, Status Code: {status}, {}) (SDK Attempt Count: {})",
            sdk_request_ids(signing_name),
            sdk_attempt_count(code),
        )),
        SdkDiagnostic::Operation => operation.map(|operation| {
            format!(
                "(Service: {service}; Operation: {operation}; Status Code: {status}; \
                 Error Code: {code}; {})",
                sdk_request_ids(signing_name),
            )
        }),
        SdkDiagnostic::Envelope => Some(format!(
            "(Service: {service}, Status Code: {status}, {})",
            sdk_request_ids(signing_name),
        )),
        SdkDiagnostic::Proxied => Some(format!(
            "(Service: {service}; Status Code: {status}; Error Code: {code}; {}; \
             Proxy: {SDK_DIRECT_CALL_PROXY})",
            sdk_request_ids(signing_name),
        )),
    }
}

/// The PROXY an SDK that words one states for a call it made DIRECTLY, with no proxy
/// configured — which is every call this emulator's in-process dispatch makes
/// ([`in_process_calls::dispatch`]).
const SDK_DIRECT_CALL_PROXY: &str = "null";

/// How many attempts an AWS SDK makes at a call whose refusal it classifies as worth
/// retrying: its default maximum, reached because every attempt was refused the same way.
const SDK_RETRIED_ATTEMPTS: u32 = 3;

/// How many it makes at a refusal it does not retry: the one call it was asked for.
const SDK_SINGLE_ATTEMPT: u32 = 1;

/// The classes of refusal an SDK retries, as [`HANDLER_ERROR_CLASSES`] names them: a
/// throttled call, an exhausted service limit and a service that failed are conditions that
/// can clear on their own, where a refusal of the REQUEST cannot and is raised on the first
/// attempt.
const SDK_RETRIED_CLASSES: [&str; 3] =
    ["Throttling", "ServiceLimitExceeded", "GeneralServiceException"];

/// The attempt count the diagnostic reports for a refusal carrying `code`, from the class
/// that code falls in ([`handler_error_code`]) — never from the message, which a request can
/// influence.
fn sdk_attempt_count(code: &str) -> u32 {
    if SDK_RETRIED_CLASSES.contains(&handler_error_code(code)) {
        SDK_RETRIED_ATTEMPTS
    } else {
        SDK_SINGLE_ATTEMPT
    }
}

/// One refused answer's message as the SDK of the called service raises it: the message that
/// service worded, then that SDK's diagnostic of the call where this crate words one
/// ([`sdk_diagnostic`]).
pub(super) fn sdk_message(signing_name: &str, status: u16, code: &str, message: &str) -> String {
    sdk_message_of(signing_name, None, status, code, message)
}

/// [`sdk_message`] for a caller that knows WHICH operation of that service it called, which
/// is part of the diagnostic some SDKs word ([`SdkDiagnostic::Operation`]).
pub(super) fn sdk_message_of(
    signing_name: &str,
    operation: Option<&str>,
    status: u16,
    code: &str,
    message: &str,
) -> String {
    match sdk_diagnostic(signing_name, operation, status, code) {
        Some(diagnostic) if message.is_empty() => diagnostic,
        Some(diagnostic) => format!("{message} {diagnostic}"),
        None => message.to_string(),
    }
}

/// Each `HandlerErrorCode` a provider's refusal is reported under, with the fragments an
/// owning service's own error CODE carries when the refusal falls in that class.
///
/// The classes are the model's own `HandlerErrorCode` values
/// ([`crate::ops::types::HANDLER_ERROR_CODE_VALUES`], which the tests below hold every
/// class here to), and that is what a client reads to decide whether a failed resource is
/// worth retrying — a throttled create is, a malformed declaration never is. The class is
/// decided by the error CODE the owning service answered, the value a client of that
/// service branches on, and never by message text a request can influence.
///
/// Walked in order, so a code carrying more than one fragment takes the first: the narrower
/// class comes before the wider one it reads like (`RequestLimitExceeded` is throttling, not
/// an exhausted quota).
const HANDLER_ERROR_CLASSES: &[(&str, &[&str])] = &[
    (
        "AccessDenied",
        &["AccessDenied", "Forbidden", "Unauthorized", "AuthorizationError"],
    ),
    (
        "InvalidCredentials",
        &["InvalidClientTokenId", "SignatureDoesNotMatch", "ExpiredToken"],
    ),
    (
        "Throttling",
        &["Throttl", "TooManyRequests", "RequestLimitExceeded", "SlowDown"],
    ),
    (
        "ServiceLimitExceeded",
        &["LimitExceeded", "QuotaExceeded", "TooManyEntries"],
    ),
    (
        HANDLER_ERROR_ALREADY_EXISTS,
        &["AlreadyExists", "AlreadyOwnedBy", "Duplicate"],
    ),
    ("NotFound", &["NotFound", "NoSuch", "DoesNotExist"]),
    (
        "ResourceConflict",
        &["Conflict", "InUse", "InvalidState", "Concurrent"],
    ),
    (
        "InternalFailure",
        &["InternalFailure", "SerializationException"],
    ),
    (
        "GeneralServiceException",
        &[
            "InternalError",
            "InternalServerError",
            "ServiceUnavailable",
            "ServiceFailure",
            // A call the owning service would not carry out because the resource it manages
            // still HOLDS content (Amazon S3's `BucketNotEmpty` for a bucket that still has
            // objects in it): the declaration is well formed and the provider asked for
            // exactly what the stack said, so what it passes on is the called service's own
            // exception rather than a fault it found in the request — and, like every other
            // class here, a condition that clears on its own once that content is gone,
            // which is why the SDK retried the call ([`SDK_RETRIED_CLASSES`]).
            "NotEmpty",
        ],
    ),
];

/// The class a refusal falls in when nothing above matches: the owning service refused the
/// request the declaration asked for, which is what every validation refusal is.
///
/// Read by [`super::provisioner::Provisioner::handler_message`] too: it is the class whose
/// own wording some providers carry inside the message.
pub(super) const HANDLER_ERROR_INVALID_REQUEST: &str = "InvalidRequest";

/// The class a refusal that the resource's NAME is already in use falls in — the one class
/// CloudFormation's own record of the account and Region can say more about than the service
/// that answered it ([`already_managed_reason`]).
const HANDLER_ERROR_ALREADY_EXISTS: &str = "AlreadyExists";

/// The lowest HTTP status that is the owning SERVICE failing rather than the request being
/// wrong, for an answer that carried no error code at all ([`UNNAMED_CODE_PREFIX`]).
const SERVER_ERROR_STATUS: u16 = 500;

/// The class an answer that named no error at all falls in when its status says the owning
/// service failed rather than that the request was wrong.
const HANDLER_ERROR_SERVICE_FAILED: &str = "GeneralServiceException";

/// The `HandlerErrorCode` an owning service's refusal is reported under — see
/// [`HANDLER_ERROR_CLASSES`].
fn handler_error_code(code: &str) -> &'static str {
    if let Some(status) = code.strip_prefix(UNNAMED_CODE_PREFIX) {
        // An answer that named no error at all: only the status it came back with says
        // whether the service failed or the request was refused.
        return match status.parse::<u16>() {
            Ok(status) if status >= SERVER_ERROR_STATUS => HANDLER_ERROR_SERVICE_FAILED,
            _ => HANDLER_ERROR_INVALID_REQUEST,
        };
    }
    HANDLER_ERROR_CLASSES
        .iter()
        .find(|(_, fragments)| fragments.iter().any(|fragment| code.contains(fragment)))
        .map_or(HANDLER_ERROR_INVALID_REQUEST, |(class, _)| *class)
}

/// The `ResourceStatusReason` the event for a resource its provider refused to act on
/// carries: "Success/failure message associated with the resource".
///
/// CloudFormation does not report its own words for such a failure — it reports the
/// provider's, quoted, beside the token of the resource request it answered and the
/// `HandlerErrorCode` class that refusal falls in ([`handler_error_code`]), so a client
/// reads WHY the resource failed and whether the failure is of a kind worth retrying.
///
/// The token is this resource request's own, minted per request as every identifier this
/// service hands out is.
///
/// `owner` is the provisioner of the type whose resource failed, because the message inside
/// the quotes is the type's HANDLER's
/// ([`super::provisioner::Provisioner::handler_message`]): for a type whose provider words a
/// refusal through the class it put it in, that wording is part of what CloudFormation
/// quotes. A type this emulator has no provisioner for reports the refusal as it came.
///
/// `identifier` is the resource the refusal is about, as its owning service knows it: a
/// provider that names the resource in its own report of a refusal — a bucket whose name is
/// already taken ([`s3_bucket::handler_message`]) — states it from here, so the event says
/// WHICH resource the refusal was about.
fn handler_failure_reason(owner: Option<Owner>, refusal: &Refusal, identifier: &str) -> String {
    // A service that worded no message at all still named the error, and the code is then
    // all there is to report.
    let message = if refusal.message.is_empty() {
        refusal.code.as_str()
    } else {
        refusal.message.as_str()
    };
    let class = match owner {
        Some(owner) => owner.handler_error_class(&refusal.code, handler_error_code(&refusal.code)),
        None => handler_error_code(&refusal.code),
    };
    let message = match owner {
        // The bucket provider's own wording, which lives beside this algorithm for the
        // reason [`s3_bucket`] records.
        Some(Owner::Builtin(Provisioner::S3Bucket)) => {
            s3_bucket::handler_message(class, identifier, message)
        }
        Some(provisioner) => provisioner.handler_message(class, message),
        None => message.to_string(),
    };
    format!(
        "Resource handler returned message: \"{message}\" (RequestToken: {}, \
         HandlerErrorCode: {class})",
        super::new_request_id(),
    )
}


/// CLOUDFORMATION's own reason for a create the owning service refused because the name is
/// already in use AND a live stack of this Region is what manages the resource holding it, or
/// `None` where the refusal is not that.
///
/// The service that owns a type arbitrates its own namespace, and a name already in use is
/// what it answers the `AlreadyExists` class for ([`handler_error_code`]) — but CloudFormation
/// knows something that service cannot: WHICH STACK manages the resource that holds the name.
/// Where the holder is one of this Region's stacks, the resource's event therefore carries
/// CloudFormation's own sentence naming that stack
/// ([`super::physical_name::already_exists_in_stack`]) instead of the owning service's report
/// about its own namespace, which is the actionable half of the failure. A name no stack of
/// the Region manages is nothing CloudFormation can add to — a name another account holds, or
/// one a client took outside any stack — so that service's own message stands.
///
/// Asked at the refusal rather than ahead of the create for the reason
/// [`super::physical_name`] records: a create the owning service ACCEPTS under a name one of
/// this Region's stacks holds is a deployment that got the resource it declared, and nothing
/// of CloudFormation's turns it into a failure.
fn already_managed_reason(
    resource: &StoredResource,
    stack: &StackProvenance,
    refusal: &Refusal,
) -> Option<String> {
    if handler_error_code(&refusal.code) != HANDLER_ERROR_ALREADY_EXISTS {
        return None;
    }
    super::physical_name::taken_by_another_stack(resource, &stack.managed_names, &stack.stack_id)
        .map(|held| {
            super::physical_name::already_exists_in_stack(
                &held.physical_resource_id,
                &held.stack_id,
            )
        })
}

/// The refusal the stack's SERVICE ROLE answers a call of `operation` on `signing_name`
/// with, or `None` where nothing refuses it — so a call the role's policies deny is never
/// made ([`super::service_role`]).
///
/// The refusal is CLOUDFORMATION's, not the owning service's: the call was not made, so
/// there is no answer of that service's to report. Its code is the one an unauthorized call
/// is classified under ([`handler_error_code`]'s `AccessDenied` class, which is what tells a
/// client whether the failed resource is worth retrying) rather than the particular spelling
/// the called service would have used, and its message is the sentence AWS words such a
/// refusal with, naming the principal, the action and the resource. `operation` is `None` for
/// a wire this cannot read an operation name off, which is judged by nothing.
fn denied_by_the_service_role(signing_name: &str, operation: Option<&str>) -> Option<Refusal> {
    let message = super::service_role::refusal(signing_name, operation?)?;
    Some(Refusal {
        code: super::service_role::ACCESS_DENIED.to_string(),
        message,
    })
}

#[cfg(test)]
mod repointed_token_tests {
    use super::retokenised;

    /// A stand-in CloudFormation computed is re-pointed wherever a template composed it into
    /// one of its own strings — and left exactly as written where it is only part of a longer
    /// identifier, which is a value of its own rather than a reference to the resource.
    ///
    /// The rows span the delimiters a template puts around such a reference and the ways a
    /// longer identifier can carry the same characters.
    #[test]
    fn a_composed_stand_in_is_re_pointed_at_token_boundaries_only() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            // The whole value, and the forms an ARN composes the reference into.
            ("abc123", "abc123", Some("zzz999")),
            (
                "between a colon and a slash",
                "arn:aws:execute-api:us-east-1:000000000000:abc123/*/*",
                Some("arn:aws:execute-api:us-east-1:000000000000:zzz999/*/*"),
            ),
            (
                "at the end of the value",
                "arn:aws:execute-api:us-east-1:000000000000:abc123",
                Some("arn:aws:execute-api:us-east-1:000000000000:zzz999"),
            ),
            (
                "one whole slash-separated segment",
                "integrations/abc123",
                Some("integrations/zzz999"),
            ),
            (
                "more than once in one value",
                "abc123/stage/abc123",
                Some("zzz999/stage/zzz999"),
            ),
            // ...and the negative branch: part of a longer identifier, so not a reference.
            ("a suffixed identifier", "abc123-assoc", None),
            ("a prefixed identifier", "the-abc123", None),
            ("an underscore away from a boundary", "abc123_two", None),
            ("a longer identifier around it", "xabc123x", None),
            ("a value that does not carry it", "arn:aws:sns:us-east-1:000000000000:q", None),
        ];
        for (case, text, expected) in cases {
            let answered = retokenised(text, "abc123", "zzz999");
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case {case:?}: re-pointing {text:?} answered {answered:?}"
            );
        }
    }

    /// A stand-in that CARRIES a slash is matched by the same rule: a slash is no character
    /// of an identifier, so such a stand-in is re-pointed where the whole of it is a token of
    /// a longer value and left alone where a longer identifier runs into it.
    #[test]
    fn a_slash_bearing_stand_in_follows_the_same_boundary_rule() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("the whole value", "apis/abc123", Some("zzz999")),
            (
                "composed into a longer value",
                "arn:aws:execute-api:us-east-1:0:apis/abc123/*",
                Some("arn:aws:execute-api:us-east-1:0:zzz999/*"),
            ),
            ("a longer identifier ahead of it", "the-apis/abc123", None),
            ("a longer identifier after it", "apis/abc123-assoc", None),
        ];
        for (case, text, expected) in cases {
            let answered = retokenised(text, "apis/abc123", "zzz999");
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case {case:?}: re-pointing {text:?} answered {answered:?}"
            );
        }
    }

    /// A stand-in that is not there, and the empty one, re-point nothing: an empty value is
    /// no identifier a reference could have resolved to.
    #[test]
    fn nothing_is_re_pointed_for_an_empty_stand_in() {
        let answered = retokenised("arn:aws:execute-api:us-east-1:0:abc123/*", "", "zzz999");
        assert_eq!(
            answered, None,
            "expected an empty stand-in to re-point nothing, got {answered:?}"
        );
    }
}

#[cfg(test)]
mod handler_error_class_tests {
    use super::{
        handler_error_code, sdk_attempt_count, HANDLER_ERROR_CLASSES, HANDLER_ERROR_INVALID_REQUEST,
        HANDLER_ERROR_SERVICE_FAILED, SDK_RETRIED_ATTEMPTS, SDK_RETRIED_CLASSES,
        SDK_SINGLE_ATTEMPT,
    };
    use crate::ops::types::HANDLER_ERROR_CODE_VALUES;

    /// Every class the SDK diagnostic counts attempts for is one the classification above
    /// can actually answer, so the retried set cannot drift from the class table.
    #[test]
    fn every_retried_class_is_one_the_classification_answers() {
        for class in SDK_RETRIED_CLASSES {
            assert!(
                HANDLER_ERROR_CLASSES.iter().any(|(named, _)| *named == class),
                "case {class:?}: not a class the refusal classification answers, which are \
                 {HANDLER_ERROR_CLASSES:?}"
            );
        }
    }

    /// The attempt count follows the CLASS of the refusal: the SDK's default maximum for a
    /// condition that can clear on its own, one attempt for a refusal of the request itself.
    #[test]
    fn the_attempt_count_follows_the_class() {
        for (code, expected) in [
            ("LimitExceededException", SDK_RETRIED_ATTEMPTS),
            ("ThrottlingException", SDK_RETRIED_ATTEMPTS),
            ("ServiceUnavailable", SDK_RETRIED_ATTEMPTS),
            ("HTTP 503", SDK_RETRIED_ATTEMPTS),
            ("ValidationException", SDK_SINGLE_ATTEMPT),
            ("InvalidTypeException", SDK_SINGLE_ATTEMPT),
            ("ResourceNotFoundException", SDK_SINGLE_ATTEMPT),
            ("HTTP 400", SDK_SINGLE_ATTEMPT),
        ] {
            let actual = sdk_attempt_count(code);
            assert_eq!(
                actual, expected,
                "case {code:?} ({}): expected {expected} attempt(s), got {actual}",
                handler_error_code(code)
            );
        }
    }

    /// Every class this module reports is one the MODEL declares: the classification is a
    /// mapping onto `HandlerErrorCode`, so a class spelled any other way would put a value
    /// on the wire that enum does not have.
    #[test]
    fn every_reported_class_is_one_the_model_declares() {
        for class in HANDLER_ERROR_CLASSES
            .iter()
            .map(|(class, _)| *class)
            .chain([HANDLER_ERROR_INVALID_REQUEST, HANDLER_ERROR_SERVICE_FAILED])
        {
            assert!(
                HANDLER_ERROR_CODE_VALUES.contains(&class),
                "case {class:?}: not a HandlerErrorCode the model declares, which are \
                 {HANDLER_ERROR_CODE_VALUES:?}"
            );
        }
    }

    /// And so is whatever the classification answers for an arbitrary code, including one
    /// no class matches and an answer that named no error at all.
    #[test]
    fn every_answer_is_one_the_model_declares() {
        for code in [
            "DBSubnetGroupNotFoundFault",
            "InvalidParameterValue",
            "ThrottlingException",
            "BucketAlreadyOwnedByYou",
            "ResourceInUseException",
            "LimitExceededException",
            "AccessDeniedException",
            "a-code-no-service-answers",
            "",
            "HTTP 501",
            "HTTP 400",
            "HTTP not-a-status",
        ] {
            let class = handler_error_code(code);
            assert!(
                HANDLER_ERROR_CODE_VALUES.contains(&class),
                "case {code:?}: answered {class:?}, which is not a HandlerErrorCode the \
                 model declares"
            );
        }
    }
}

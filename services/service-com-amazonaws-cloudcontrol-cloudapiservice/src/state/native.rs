// SPDX-License-Identifier: CC0-1.0

//! The resources Cloud Control API acts on that live in ANOTHER service's store.
//!
//! Cloud Control API is a front end over the resource type's own provider, not an
//! inventory of its own: `GetResource` "can be used to return information about an
//! existing resource in your account and Amazon Web Services Region, whether those
//! resources were provisioned using Cloud Control API"
//! (`smithy_query(query="doc",shape="GetResource")`). So for a resource type whose
//! resources live in a store of THIS emulator, every resource operation of this service
//! has to resolve and act on the resource THERE: a client using Cloud Control API as the
//! uniform read path has to see the resources the owning service's own API created, and
//! stop seeing them once that API deleted them.
//!
//! One [`NativeResourceProvider`] per such `TypeName` is what this module holds. A
//! provider owns the handle to the OWNING service's state in the same shared engine
//! ([`NativeResources::for_engine`]) and answers the five things a resource operation
//! needs of a type: read one resource's model, list the type's resources, provision one,
//! apply a model to one, and remove one. A `TypeName` no provider stands behind is served
//! out of this service's own store exactly as before — that store is the only place such a
//! resource can live.
//!
//! Every one of those answers is about ONE Region: this service "can be used to return
//! information about an existing resource in your account and Amazon Web Services Region",
//! and an owning service that holds a resource per Region resolves a reference in the
//! Region the call was made in. So the Region a request was signed for travels into each
//! method, and the work a request OWES carries the Region it was accepted in
//! ([`crate::state::ResourceEffect`]) — a type whose owning store is one namespace per
//! Region can then never be read in one Region and written in another.
//!
//! LOCK ORDER: a caller holds this service's own store guard while taking a provider's
//! ([`crate::state::ComAmazonawsCloudcontrolCloudapiserviceState::materialise_due`]), so
//! the request record and the resource it owes are settled together. Never the other way
//! round: nothing in an owning service reaches back into this one.

mod alarms;
mod buckets;
mod destinations;
mod log_groups;
mod log_streams;
mod managed_policies;
mod parameters;
mod queues;
/// Whether the IAM role a resource operation request named lets this service make the calls
/// the type's handler has to make on the owning service.
mod role_permissions;
mod secrets;
mod state_machines;
mod tables;
mod topics;

pub use role_permissions::RolePermissions;

use ::serde_json::Value;
use ::std::collections::BTreeMap;
use ::std::ops::Bound;
use ::std::time::{SystemTime, UNIX_EPOCH};

/// Why an owning service would not do what a resource operation asked of it.
///
/// Each variant is the OWNING handler's verdict, which is what a Cloud Control API
/// refusal reports: the `HandlerErrorCode` it failed with
/// ([`Self::handler_error_code`]) and, where the owning service states one in its own
/// terms, its diagnostic ([`Self::detail`]).
///
/// WHICH verdict a refusal is reported under follows WHAT the refusal names as the fault,
/// which is what the two catch-all codes of the model's own `HandlerErrorCode` enum say: a
/// handler returns `InvalidRequest` when "invalid input from the user has generated a generic
/// exception" and `GeneralServiceException` when "the downstream service generated an error
/// that doesn't map to any other handler error code". So a value the desired state supplies
/// that lies outside the DOMAIN the type's schema publishes for that property — the range of
/// a count, the two values of a flag, a set stated with the property itself — is invalid
/// input from the user whichever side spots it ([`Self::Invalid`]): no resource of the type
/// could carry it, and that is settled by the one value with nothing of the store in it. A
/// refusal that one value does NOT settle — a rule over two properties, a name the owning
/// service holds to a charset or keeps for itself, a value set only that service publishes,
/// the resource's own state — is that service's error ([`Self::Rejected`]).
///
/// A call the OWNING service would have refused for want of a grant is one of those
/// downstream errors too ([`Self::Rejected`]): the handler made that call with the role's
/// credentials, that service answered its own authorization refusal, and the handler maps
/// that answer to no verdict of its own — so it is reported under `GeneralServiceException`
/// as that service's other errors on the call are. A handler reads an authorization refusal
/// on a branch of its OWN, though, so what it states about one is the call it was making and
/// never the refusal's own wording, whichever of the two a type's handler states about the
/// others ([`NativeResourceProvider::relayed_client`]).
#[derive(Debug, PartialEq, Eq)]
pub enum NativeRefusal {
    /// The owning service holds no such resource.
    NotFound,
    /// The owning service already holds a resource under this identifier.
    AlreadyExists,
    /// The owning service holds as many resources of this type as it may, and states
    /// the quota in its own terms.
    AtCapacity(String),
    /// The resource MODEL is at fault against the type's registry schema, which the
    /// handler itself judges: a property the schema does not declare, one it declares and
    /// this path does not apply, a required property the model names no value for, a value
    /// outside the form, the range or the value set that schema publishes for the property
    /// it carries, or an identifier the resource does not answer to. Nothing the store holds
    /// decides it.
    Invalid(String),
    /// The OWNING service raised an error of its own that this type's handler maps to no
    /// other verdict: a model whose every value is within what the type's schema publishes
    /// for its property, whose call that service's own API would still refuse — a name it
    /// holds to a charset of its own or keeps for itself, a value set only it publishes, a
    /// combination of properties it will not take, the resource's own state — and a call it
    /// would refuse for want of a grant ([`role_permissions::RolePermissions::refusal`]).
    /// Carries that service's own diagnostic.
    Rejected(String),
}

impl NativeRefusal {
    /// The `HandlerErrorCode` this refusal is reported under — a value of the model's own
    /// `HandlerErrorCode` enum, which `crate::state`'s wire-vocabulary check pins.
    pub fn handler_error_code(&self) -> &'static str {
        match self {
            Self::NotFound => crate::state::HANDLER_ERROR_NOT_FOUND,
            Self::AlreadyExists => crate::state::HANDLER_ERROR_ALREADY_EXISTS,
            Self::AtCapacity(_) => HANDLER_ERROR_SERVICE_LIMIT_EXCEEDED,
            Self::Invalid(_) => HANDLER_ERROR_INVALID_REQUEST,
            Self::Rejected(_) => HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION,
        }
    }
}

/// The `HandlerErrorCode` for work an owning service has no room for.
pub const HANDLER_ERROR_SERVICE_LIMIT_EXCEEDED: &str = "ServiceLimitExceeded";

/// The `HandlerErrorCode` for a fault in the resource model against what that type's
/// registry schema publishes, which is what this code is for: invalid input from the user,
/// rather than a refusal the model's own values do not settle
/// ([`HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION`]).
pub const HANDLER_ERROR_INVALID_REQUEST: &str = "InvalidRequest";

/// The `HandlerErrorCode` for an error the OWNING SERVICE raised that the resource type's
/// handler maps to none of the others — which is what this code is for: the downstream
/// service generated an error, and the handler has no verdict of its own for it. A call that
/// service refused for want of a grant is such an error: the handler made the call and read
/// that service's refusal of it ([`NativeRefusal::Rejected`]).
pub const HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION: &str = "GeneralServiceException";

/// Why the OWNING service would refuse the call a resource type's READ handler carries a
/// REFERENCE in — the verdict that handler fails with when the reference it was handed never
/// reaches an existence question at all.
///
/// A read is the type's handler forwarding that reference to an API of the owning service, so
/// a reference that API will not take is refused THERE, before anything about any store is
/// asked: the answer is the same whether or not a resource of that reference was ever
/// created, which is what makes it a different class of refusal from absence. A caller tells
/// the two apart by the verdict it is answered — a reference it cannot name a resource with
/// is the caller's to fix, where an absent resource is one to create or stop polling for.
///
/// Narrower than [`NativeRefusal`] on purpose: the verdicts that refusal carries about a
/// store's CONTENTS — absent, already there, at capacity — are answers to a question this one
/// was never asked, so none of them can be stated here. What is left is the split
/// [`NativeRefusal`]'s own two catch-all codes make: invalid input from the user, which no
/// resource of the type could ever answer to ([`Self::Invalid`]), and an error the owning
/// service raised that the handler maps to no verdict of its own ([`Self::Rejected`]).
#[derive(Debug, PartialEq, Eq)]
pub enum UnreadableReference {
    /// The reference lies outside the DOMAIN the type's schema publishes for its identifier,
    /// so the read handler fails with [`HANDLER_ERROR_INVALID_REQUEST`]. Carries the owning
    /// service's own refusal of the call that was made.
    Invalid(String),
    /// The owning service raised an error about that call which the handler maps to no
    /// verdict of its own, so it fails with [`HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION`].
    /// Carries that service's own diagnostic.
    ///
    /// No provider states one yet: every reference a provider here judges is judged against
    /// the DOMAIN its type's schema publishes, which is invalid input from the user whichever
    /// side spots it ([`Self::Invalid`]). The verdict is stated because the two codes are the
    /// whole split a handler's catch-all verdicts make, so a provider whose owning service
    /// refuses a read call for a reason the store does not settle has the honest code to
    /// report it under rather than the one that calls the caller wrong.
    Rejected(String),
}

impl UnreadableReference {
    /// The `HandlerErrorCode` the read handler failed with — a value of the model's own
    /// `HandlerErrorCode` enum, which `crate::state`'s wire-vocabulary check pins.
    pub fn handler_error_code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => HANDLER_ERROR_INVALID_REQUEST,
            Self::Rejected(_) => HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION,
        }
    }

    /// The owning service's own answer to the refused call, which the handler's verdict
    /// carries as its diagnostic.
    pub fn detail(&self) -> &str {
        match self {
            Self::Invalid(detail) | Self::Rejected(detail) => detail,
        }
    }
}

/// EVERY property each resource type's registry schema declares, by `TypeName`.
///
/// One row per type whose provider reads a resource model: the set is the PROVIDER's, stated
/// beside the reading that has an arm per property, so there is one catalogue of what a type
/// declares rather than one per stage. The stage that judges a model's shape against the
/// schema before any handler is asked reads it from here
/// ([`crate::state::registry_schema`]), and a type with no row is one that stage judges the
/// shape of nowhere — the position every catalogue of this crate takes about what it does not
/// state.
pub const DECLARED_PROPERTIES: &[(&str, &[&str])] = &[
    (alarms::TYPE_NAME, alarms::DECLARED_PROPERTIES),
    (log_groups::TYPE_NAME, log_groups::DECLARED_PROPERTIES),
    (log_streams::TYPE_NAME, log_streams::DECLARED_PROPERTIES),
    (parameters::TYPE_NAME, parameters::DECLARED_PROPERTIES),
    (
        state_machines::TYPE_NAME,
        state_machines::DECLARED_PROPERTIES,
    ),
];

/// The properties `type_name`'s registry schema declares, or none where no row states them
/// ([`DECLARED_PROPERTIES`]).
#[must_use]
pub fn declared_properties(type_name: &str) -> &'static [&'static str] {
    DECLARED_PROPERTIES
        .iter()
        .find(|(name, _)| *name == type_name)
        .map_or(&[], |(_, properties)| *properties)
}

/// The sentence a resource type's handler states when a call it made on the OWNING service
/// raised an error it maps to no verdict of its own ([`NativeRefusal::Rejected`]).
///
/// What a handler that states a sentence of its OWN reports, rather than the exception its
/// client raised ([`NativeResourceProvider::relayed_client`] answers which of the two a
/// type's handler does): the fixed text of that class around its one variable part, the
/// owning service's API operation it was making — which is the action named without the
/// namespace an IAM action carries it under. So a caller reading a failed request learns
/// which downstream call did not happen, and nothing of the downstream service's own wording
/// is passed off as Cloud Control API's.
///
/// ONE sentence for every such type, because it is the sentence of that FAILURE CLASS rather
/// than of a type: what differs between two of those handlers is which call they make, which
/// is why the call is read from each provider's own published calls ([`downstream_call`])
/// instead of being written per type.
pub fn downstream_failure(call: &str) -> String {
    let operation = call.rsplit_once(':').map_or(call, |(_, operation)| operation);
    format!("Error occurred during operation '{operation}'.")
}

/// The owning service's call a refusal `provider` raised about `operation` is reported
/// against: the FIRST action that provider publishes for the operation
/// ([`NativeResourceProvider::handler_calls`]), which is the call that carries the operation
/// out — the others are the ones it makes around that call.
///
/// `None` for an operation the provider publishes no call for, which states nothing about
/// what the handler was doing: such a refusal keeps the owning service's own diagnostic, the
/// position every catalogue of this crate takes about what it does not state.
pub fn downstream_call(
    provider: &dyn NativeResourceProvider,
    operation: &str,
) -> Option<&'static str> {
    provider.handler_calls(operation).first().copied()
}

/// WHERE the identifiers a resource operation reports lead, for a type whose primary
/// identifier is an ADDRESS the owning service announces rather than a name.
///
/// Such an identifier is a handle a client keeps and hands back — to this API, and to the
/// owning service's own operations — so it has to be the address that service would report
/// to the SAME caller: a client reaching this emulator at its own endpoint is announced that
/// endpoint by every service of it, and an identifier naming anything else cannot be joined
/// against the handle that client already holds.
///
/// A reader that has no request to answer for ([`Self::published`]) takes each owning
/// service's own published form instead, which is the address it announces when nothing was
/// asked.
#[derive(Debug, Clone, Default)]
pub struct Announced {
    /// The `host[:port]` Amazon SQS reports a queue url at for this caller, or `None` for a
    /// reader with no request behind it.
    sqs_endpoint: Option<String>,
}

impl Announced {
    /// What the owning services announce to the caller of THIS request: for Amazon SQS, the
    /// endpoint that service's own operations report a queue url at
    /// (`sqs::reported_endpoint_authority`), so the url this API reports for a queue is the
    /// string `CreateQueue` and `GetQueueUrl` answer that caller with.
    #[must_use]
    pub fn to_this_caller(
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Self {
        Self {
            sqs_endpoint: Some(
                ::service_com_amazonaws_sqs_amazonsqs::state::reported_endpoint_authority(
                    ctx, region,
                ),
            ),
        }
    }

    /// What those services publish with no request to read: each type's provider then
    /// composes the owning service's own published form.
    #[must_use]
    pub fn published() -> Self {
        Self::default()
    }

    /// The `host[:port]` a queue url this reader reports must name, when one was read off
    /// the request.
    #[must_use]
    pub fn sqs_endpoint(&self) -> Option<&str> {
        self.sqs_endpoint.as_deref()
    }
}

/// What one resource type's owning service answers about its resources.
///
/// A provider holds the handle to that service's state, so every answer here is read from
/// — and every change written to — the store the owning service's OWN operations read.
pub trait NativeResourceProvider: Send + Sync {
    /// The `TypeName` this provider stands behind.
    fn type_name(&self) -> &'static str;

    /// The property of the resource model that carries the type's primary identifier.
    fn identifier_property(&self) -> &'static str;

    /// The primary identifier the resource `properties` describes will answer to in
    /// `region`, for a type whose primary identifier the OWNING service ASSIGNS — one the
    /// type's schema declares read-only, so no desired state carries it — and `None` for a
    /// type whose desired state names its own identifier, which is the rule every other
    /// type follows ([`crate::state::derive_identifier`]).
    fn assigned_identifier(&self, region: &str, properties: &Value) -> Option<String> {
        let _ = (region, properties);
        None
    }

    /// The resource model of `identifier` in `region`, or `None` when the owning service
    /// holds no such resource there.
    fn read(&self, region: &str, identifier: &str) -> Option<Value>;

    /// Why the OWNING service would refuse the call this type's READ handler carries
    /// `identifier` in, or `None` where that call is one it would take.
    ///
    /// Asked BEFORE a resource is resolved, because the handler's own call is: a reference
    /// the owning service's read will not accept at the member it arrives on is refused by
    /// that service before it answers any existence question, so the handler's verdict is
    /// about the reference ([`UnreadableReference`]) rather than about a resource that cannot
    /// be found — and reporting it as absence would tell a caller to create what it cannot
    /// name.
    ///
    /// A provider that states no such rule answers `None`, the position every catalogue of
    /// this crate takes about what it does not state, and its references are resolved by
    /// [`Self::read`] alone: what the owning service's API takes at that member is that
    /// service's to publish, so a provider states it only where that service does.
    fn unreadable_reference(&self, identifier: &str) -> Option<UnreadableReference> {
        let _ = identifier;
        None
    }

    /// The resources of this type the owning service holds in `region`, as
    /// `(primary identifier, resource model)` in identifier order: at most `limit` of
    /// them, and only those whose identifier sorts AFTER `after`.
    ///
    /// Bounded at the owning store, so a listing builds the window it is about to report
    /// rather than the whole inventory behind it.
    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)>;

    /// Provision `identifier` from `properties`, in `region`, as of `at` — answering the
    /// model the owning service holds for it once it is there.
    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal>;

    /// Apply `properties` to `identifier` in `region`, as of `at` — answering the model the
    /// owning service holds for it afterwards.
    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal>;

    /// Remove `identifier` from the owning service's store, in `region`, as of `at`.
    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal>;

    /// The owning service's API actions this provider's handler CALLS to carry out
    /// `operation` — one of [`crate::state::OPERATION_CREATE`],
    /// [`crate::state::OPERATION_UPDATE`] and [`crate::state::OPERATION_DELETE`] — on a
    /// resource of this type.
    ///
    /// They are what the role a request names has to be allowed to do for the work to
    /// happen ([`RolePermissions`]), so a provider publishes the calls it really makes and
    /// nothing else: a call it does not make is not a permission the work needs. An
    /// operation a provider publishes nothing for decides nothing, which is the position
    /// every catalogue of this crate takes about what it does not state.
    fn handler_calls(&self, operation: &str) -> &'static [&'static str] {
        let _ = operation;
        &[]
    }

    /// The ARN of `identifier` in `region`, as the OWNING service names it: the resource an
    /// IAM statement about a resource of this type names, and so the resource a call on it is
    /// decided against ([`RolePermissions`]).
    ///
    /// `None` where this provider cannot name one — a type whose identifier is not a
    /// reference that service builds an ARN from — which leaves the decision about such a
    /// resource unmade rather than made against an ARN nothing answers to.
    fn resource_arn(&self, region: &str, identifier: &str) -> Option<String> {
        let _ = (region, identifier);
        None
    }

    /// The owning service's SDK CLIENT this type's handler reports an error that service
    /// raised through, named as that client's rendering of an exception names the service —
    /// or `None` for a handler that reports such an error as a sentence of its own.
    ///
    /// A handler reaches the service that owns its type through that service's SDK, and the
    /// two ways a handler reports an error it maps to no verdict of its own differ in WHAT it
    /// states about the failure: one states the exception its client RAISED, so the owning
    /// service's own diagnostic travels out with the client's rendering of the refused call
    /// around it, and one states only the sentence about the CALL it was making
    /// ([`downstream_failure`]), which carries no diagnosis. Which of the two a type's
    /// handler does is a property of that handler, exactly as the sentence it words a
    /// resource it cannot find in is ([`::service_common::cfn_resource_handlers`]), so each
    /// provider answers for its own.
    ///
    /// `None` is the answer for every type whose handler is not recorded relaying one, the
    /// position every catalogue of this crate takes about what it does not state.
    fn relayed_client(&self) -> Option<&'static str> {
        None
    }
}

/// The providers reachable from one engine: the owning stores this service resolves
/// resources in.
///
/// Built per request from the handler's engine ([`Self::for_engine`]), because a provider
/// is a handle to a store rather than state of its own.
pub struct NativeResources {
    /// One provider per `TypeName` this emulator holds the resources of elsewhere.
    providers: Vec<Box<dyn NativeResourceProvider>>,
    /// The account's roles, which is what a request naming a `RoleArn` is carried out with
    /// the permissions of. Held here beside the providers for the same reason they are: it
    /// is a handle to another service's store, built per request from the same engine.
    roles: RolePermissions,
}

impl ::std::fmt::Debug for NativeResources {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("NativeResources")
            .field(
                "types",
                &self
                    .providers
                    .iter()
                    .map(|provider| provider.type_name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl NativeResources {
    /// The providers whose owning store THIS engine carries, each answering with the
    /// identifiers the owning service ANNOUNCES to the caller of THIS request
    /// ([`Announced`]): call it with a handler's `&*self.engine` and that handler's context.
    ///
    /// A resource's identifier is the owning service's to report, and for a type whose
    /// identifier is an ADDRESS that service announces one that leads back to the endpoint
    /// the caller reached. Reporting the address that service would have reported is what
    /// lets a client join the identifier this API tracks against the handle it holds.
    pub fn announced_to<Eng: Send + Sync + 'static>(
        engine: &Eng,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Self {
        Self::built(engine, Announced::to_this_caller(ctx, region))
    }

    /// The providers whose owning store THIS engine carries: call it with a handler's
    /// `&*self.engine`.
    ///
    /// A store the engine does not carry contributes no provider, so its type falls back
    /// to this service's own store rather than resolving to nothing.
    ///
    /// The identifiers are the ones each owning service publishes with no request to read
    /// ([`Announced::published`]) — for a caller that holds a context, [`Self::announced_to`]
    /// is the reading.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self::built(engine, Announced::published())
    }

    fn built<Eng: Send + Sync + 'static>(engine: &Eng, announced: Announced) -> Self {
        let mut providers: Vec<Box<dyn NativeResourceProvider>> = Vec::new();
        if let Some(provider) = log_groups::LogGroupProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = log_streams::LogStreamProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = alarms::AlarmProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = destinations::DestinationProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = queues::QueueProvider::for_engine(engine, &announced) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = topics::TopicProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = buckets::BucketProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = managed_policies::ManagedPolicyProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = state_machines::StateMachineProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = parameters::ParameterProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = tables::TableProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = secrets::SecretProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        Self {
            providers,
            roles: RolePermissions::for_engine(engine),
        }
    }

    /// What the account's roles let this service do, for a request that named one.
    pub fn roles(&self) -> &RolePermissions {
        &self.roles
    }

    /// The provider that stands behind `type_name`, or `None` when the resources of that
    /// type live in this service's own store.
    pub fn provider(&self, type_name: &str) -> Option<&dyn NativeResourceProvider> {
        self.providers
            .iter()
            .find(|provider| provider.type_name() == type_name)
            .map(::std::convert::AsRef::as_ref)
    }
}

/// `at` in milliseconds since the Unix epoch, which is what the timestamp members of the
/// services whose resources these providers hold are expressed in. An instant before the
/// epoch, which no clock reports, reads as the epoch itself.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The CloudWatch Logs store the resources of the `AWS::Logs::*` types live in — that
/// service's OWN state struct, reached through the shared engine, so what is written there
/// is what its own readers report.
pub type LogsState =
    ::service_com_amazonaws_cloudwatchlogs_logs_20140328::ComAmazonawsCloudwatchlogsLogs20140328State;

/// The Amazon SQS store the resources of the `AWS::SQS::*` types live in — that service's
/// OWN state struct, reached through the shared engine, so a queue this emulator holds is
/// the queue Cloud Control API acts on whichever API created it.
pub type SqsState = ::service_com_amazonaws_sqs_amazonsqs::state::ComAmazonawsSqsAmazonsqsState;

/// The Amazon SNS store the resources of the `AWS::SNS::*` types live in — that service's
/// OWN state struct, reached through the shared engine, so a topic this emulator holds is
/// the topic Cloud Control API acts on whichever API created it.
pub type SnsState =
    ::service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;

/// The Amazon S3 store the resources of the `AWS::S3::Bucket` type live in — that service's
/// OWN state struct, reached through the shared engine, so a bucket Cloud Control API
/// provisions is the bucket that service then serves objects in.
pub type S3State = ::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State;

/// The IAM store the resources of the `AWS::IAM::*` types live in — that service's OWN
/// state struct, reached through the shared engine, so a managed policy Cloud Control API
/// provisions is the policy `iam:GetPolicy` and `iam:AttachRolePolicy` then accept, and the
/// ARN its primary identifier is comes from that service's own ARN builder.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The Step Functions store the resources of the `AWS::StepFunctions::*` types live in —
/// that service's OWN state struct, reached through the shared engine, so a state machine
/// Cloud Control API provisions is the one `states:DescribeStateMachine` and
/// `states:StartExecution` then accept under the ARN reported as its primary identifier.
pub type SfnState =
    ::service_com_amazonaws_sfn_awsstepfunctions::state::ComAmazonawsSfnAwsstepfunctionsState;

/// The Amazon DynamoDB store the resources of the `AWS::DynamoDB::Table` type live in — that
/// service's OWN state struct, reached through the shared engine, so a table
/// `dynamodb:CreateTable` created is the table Cloud Control API resolves, and a table
/// `dynamodb:DeleteTable` removed is one it reports as not found.
///
/// The DynamoDB Streams store a table's stream lives in is reached through that service's own
/// seam (`ddb::streams_state`), so the stream half of a table's lifecycle stays its
/// knowledge rather than being reassembled here.
pub type DynamodbState =
    ::service_com_amazonaws_dynamodb_dynamodb_20120810::state::ComAmazonawsDynamodbDynamodb20120810State;

/// The Systems Manager store the resources of the `AWS::SSM::Parameter` type live in — that
/// service's OWN state struct, reached through the shared engine, so a parameter
/// `ssm:PutParameter` wrote is the parameter Cloud Control API resolves, and one
/// `ssm:DeleteParameter` removed is one it reports as not found.
pub type SsmState = ::service_com_amazonaws_ssm_amazonssm::ComAmazonawsSsmAmazonssmState;

/// The AWS Secrets Manager store the resources of the `AWS::SecretsManager::Secret` type live
/// in — that service's OWN state struct, reached through the shared engine, so a secret
/// `secretsmanager:CreateSecret` created (or a CloudFormation stack declared) is the secret
/// Cloud Control API resolves, and one `secretsmanager:DeleteSecret` removed is one it reports
/// as not found.
pub type SecretsManagerState =
    ::service_com_amazonaws_secretsmanager_secretsmanager::state::ComAmazonawsSecretsmanagerSecretsmanagerState;

/// The window `(after, limit)` names of an owning store keyed by its resources' own
/// identifiers: the entries whose key sorts AFTER `after`, in key order, at most `limit`
/// of them.
///
/// A `BTreeMap`'s own ordered range, so a provider's listing reads the window it is about
/// to report and never walks the inventory behind it.
fn window<'a, T>(
    held: &'a BTreeMap<String, T>,
    after: Option<&str>,
    limit: usize,
) -> impl Iterator<Item = (&'a String, &'a T)> + 'a {
    let start = after.map_or(Bound::Unbounded, |after| Bound::Excluded(after.to_string()));
    held.range((start, Bound::Unbounded)).take(limit)
}

/// The text `value` carries, or why it is not the text `property` takes.
fn text(property: &str, value: &Value) -> Result<String, NativeRefusal> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| NativeRefusal::Invalid(format!("{property} must be a string")))
}

/// Check that `property` carries exactly `expected` — the rule a model's identity and its
/// read-only properties are held to: a read reports them, so a model a client round-trips
/// carries them, but only ever with the value the resource already answers to.
fn require_value(property: &str, value: &Value, expected: &str) -> Result<(), NativeRefusal> {
    (value.as_str() == Some(expected))
        .then_some(())
        .ok_or_else(|| {
            NativeRefusal::Invalid(format!("{property} of this resource is {expected}"))
        })
}

/// Check that `property` carries exactly what the resource already HOLDS for it — the rule
/// a property of the type's schema that this emulator's provider does not APPLY is held
/// to.
///
/// A read reports such a property, so a model a client round-trips carries it, and carrying
/// it unchanged asks for nothing; any other value — and any value at all where the resource
/// holds none — asks for work no handler here performs, which is refused rather than
/// accepted and dropped.
fn require_held(
    type_name: &str,
    property: &str,
    value: &Value,
    held: Option<&str>,
) -> Result<(), NativeRefusal> {
    match held {
        Some(held) => require_value(property, value, held),
        None => Err(unsupported(type_name, property)),
    }
}

/// Check that `property` carries exactly what a read of the resource REPORTS for it — the
/// rule a property of the type's schema whose value is not text is held to, where this
/// emulator's provider does not APPLY that property.
///
/// The text-valued rule ([`require_held`]) stated over a whole property VALUE, for a schema
/// whose properties are structures and lists: a read reports such a property, so a model a
/// client round-trips carries it, and carrying it unchanged asks for nothing; any other
/// value — and any value at all where the read reports none — asks for work no handler here
/// performs, which is refused rather than accepted and dropped.
fn require_reported(
    type_name: &str,
    property: &str,
    value: &Value,
    reported: Option<&Value>,
) -> Result<(), NativeRefusal> {
    match reported {
        Some(reported) if reported == value => Ok(()),
        Some(_) => Err(NativeRefusal::Invalid(format!(
            "{property} of this resource is what a read of it reports"
        ))),
        None => Err(unsupported(type_name, property)),
    }
}

/// The refusal for a property of a type's schema that THIS path does not apply, named so a
/// caller can tell it from a property the type does not declare at all.
///
/// Shared by every provider, because "this emulator's handler does not apply that property"
/// is one condition whichever type's model carried it.
fn unsupported(type_name: &str, property: &str) -> NativeRefusal {
    NativeRefusal::Invalid(format!(
        "{property} is a property of {type_name} this emulator's handler does not apply"
    ))
}

/// The property every type's schema carries its tag set under, as a list of
/// `{ Key, Value }` pairs.
const TAGS: &str = "Tags";

/// The key member of one entry of [`TAGS`].
const TAG_KEY: &str = "Key";

/// The value member of one entry of [`TAGS`].
const TAG_VALUE: &str = "Value";

/// The tag entries `value` carries, in the order the list carried them — or why it is not a
/// tag set at all.
///
/// Shared by every provider: the shape of a tag set is the registry's rather than any one
/// type's, so the condition and its wording are one. The entries are handed back as the
/// LIST, before two entries of one key are folded together, so a provider whose owning
/// service bounds the tag COUNT judges the list the request sent.
fn tag_entries(value: &Value) -> Result<Vec<(String, String)>, NativeRefusal> {
    let Some(entries) = value.as_array() else {
        return Err(NativeRefusal::Invalid(format!(
            "{TAGS} must be a list of {{ {TAG_KEY}, {TAG_VALUE} }} pairs"
        )));
    };
    let mut tags = Vec::with_capacity(entries.len());
    for entry in entries {
        let key = entry.get(TAG_KEY).and_then(Value::as_str);
        let tag_value = entry.get(TAG_VALUE).and_then(Value::as_str);
        let (Some(key), Some(tag_value)) = (key, tag_value) else {
            return Err(NativeRefusal::Invalid(format!(
                "every {TAGS} entry must carry {TAG_KEY} and {TAG_VALUE}"
            )));
        };
        tags.push((key.to_string(), tag_value.to_string()));
    }
    Ok(tags)
}

/// The tag set `value` carries, folded by key — what a provider writes onto the resource.
fn tag_set(value: &Value) -> Result<BTreeMap<String, String>, NativeRefusal> {
    Ok(tag_entries(value)?.into_iter().collect())
}

/// The tag set one resource renders as: one `{ Key, Value }` object per tag, in key order.
///
/// Shared by every provider, so one resource reports its tags the same way whichever type
/// it is of and whichever order the owning store holds them in.
fn tag_list<'a>(tags: impl IntoIterator<Item = (&'a String, &'a String)>) -> Value {
    let ordered: BTreeMap<&String, &String> = tags.into_iter().collect();
    Value::Array(
        ordered
            .into_iter()
            .map(|(key, value)| {
                Value::Object(::serde_json::Map::from_iter([
                    (TAG_KEY.to_string(), Value::from(key.clone())),
                    (TAG_VALUE.to_string(), Value::from(value.clone())),
                ]))
            })
            .collect(),
    )
}

/// The name a create picks for a resource whose model names none.
///
/// A name is the resource's identity and the property that carries it is optional in the
/// type's schema, exactly as it is optional in a template — where "if you don't specify a
/// name, CloudFormation generates a unique physical ID". A generated name for a FIFO
/// resource carries the suffix such a name must end with, so a model that asks for one and
/// names none still gets a name the owning service accepts.
fn generated_name(fifo: bool, fifo_suffix: &str) -> String {
    let minted = crate::state::mint_token();
    if fifo {
        return format!("{minted}{fifo_suffix}");
    }
    minted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every handler error code a provider's refusal is reported under is a value the
    /// model's own `HandlerErrorCode` enum declares, so no refusal can carry a code a
    /// client cannot read.
    #[test]
    fn a_refusals_handler_error_code_is_the_models_own() {
        for refusal in [
            NativeRefusal::NotFound,
            NativeRefusal::AlreadyExists,
            NativeRefusal::AtCapacity("at the quota".to_string()),
            NativeRefusal::Invalid("not a property of the type".to_string()),
            NativeRefusal::Rejected("the resource is not in a state for this".to_string()),
        ] {
            let code = refusal.handler_error_code();
            assert!(
                crate::ops::types::HANDLER_ERROR_CODE_VALUES.contains(&code),
                "refusal {refusal:?}: {code:?} is not a value the model's HandlerErrorCode \
                 enum declares"
            );
        }
    }

    /// A type no provider stands behind resolves to none, which is what keeps this
    /// service's own store the place such a resource lives.
    #[test]
    fn only_a_backed_type_resolves_to_a_provider() {
        let engine = ::engine::InMemoryEngine::new();
        let native = NativeResources::for_engine(&engine);
        assert!(
            native.provider("AWS::Logs::LogGroup").is_some(),
            "expected the log group provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::Logs::LogStream").is_some(),
            "expected the log stream provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::CloudWatch::Alarm").is_some(),
            "expected the metric alarm provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::Logs::Destination").is_some(),
            "expected the destination provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::SQS::Queue").is_some(),
            "expected the queue provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::SNS::Topic").is_some(),
            "expected the topic provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::S3::Bucket").is_some(),
            "expected the bucket provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::IAM::ManagedPolicy").is_some(),
            "expected the managed policy provider, got {native:?}"
        );
        assert!(
            native
                .provider("AWS::StepFunctions::StateMachine")
                .is_some(),
            "expected the state machine provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::SSM::Parameter").is_some(),
            "expected the parameter provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::DynamoDB::Table").is_some(),
            "expected the table provider, got {native:?}"
        );
        assert!(
            native.provider("AWS::SecretsManager::Secret").is_some(),
            "expected the secret provider, got {native:?}"
        );
        for unbacked in ["AWS::Kinesis::Stream", "AWS::DynamoDB::GlobalTable", "AWS::Logs"] {
            assert!(
                native.provider(unbacked).is_none(),
                "type {unbacked:?}: expected no provider, got one"
            );
        }
        assert!(
            NativeResources::for_engine(&())
                .provider("AWS::Logs::LogGroup")
                .is_none(),
            "a deployment whose engine carries no owning store must stand behind no type"
        );
    }
}

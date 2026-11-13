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

mod buckets;
mod destinations;
mod log_groups;
mod managed_policies;
mod queues;
mod state_machines;
mod topics;

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
#[derive(Debug, PartialEq, Eq)]
pub enum NativeRefusal {
    /// The owning service holds no such resource.
    NotFound,
    /// The owning service already holds a resource under this identifier.
    AlreadyExists,
    /// The owning service holds as many resources of this type as it may, and states
    /// the quota in its own terms.
    AtCapacity(String),
    /// The model names something this type's resource cannot carry — a fault in the
    /// resource model, which the handler itself judges. Carries the owning service's own
    /// diagnostic.
    Invalid(String),
    /// The owning service would not carry out the operation on the resource AS IT IS: the
    /// request is a well-formed one that service raised an error of its own about, and the
    /// handler maps it to no other verdict. Carries that service's own diagnostic.
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

/// The `HandlerErrorCode` for a model the owning service will not take.
pub const HANDLER_ERROR_INVALID_REQUEST: &str = "InvalidRequest";

/// The `HandlerErrorCode` for an error the OWNING SERVICE raised that the resource type's
/// handler maps to none of the others — which is what this code is for: the downstream
/// service generated an error, and the handler has no verdict of its own for it.
pub const HANDLER_ERROR_GENERAL_SERVICE_EXCEPTION: &str = "GeneralServiceException";

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
}

/// The providers reachable from one engine: the owning stores this service resolves
/// resources in.
///
/// Built per request from the handler's engine ([`Self::for_engine`]), because a provider
/// is a handle to a store rather than state of its own.
pub struct NativeResources {
    /// One provider per `TypeName` this emulator holds the resources of elsewhere.
    providers: Vec<Box<dyn NativeResourceProvider>>,
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
    /// The providers whose owning store THIS engine carries: call it with a handler's
    /// `&*self.engine`.
    ///
    /// A store the engine does not carry contributes no provider, so its type falls back
    /// to this service's own store rather than resolving to nothing.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        let mut providers: Vec<Box<dyn NativeResourceProvider>> = Vec::new();
        if let Some(provider) = log_groups::LogGroupProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = destinations::DestinationProvider::for_engine(engine) {
            providers.push(Box::new(provider));
        }
        if let Some(provider) = queues::QueueProvider::for_engine(engine) {
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
        Self { providers }
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
        for unbacked in ["AWS::Kinesis::Stream", "AWS::DynamoDB::Table", "AWS::Logs"] {
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

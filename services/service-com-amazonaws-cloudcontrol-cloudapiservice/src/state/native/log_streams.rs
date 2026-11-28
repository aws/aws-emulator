// SPDX-License-Identifier: CC0-1.0

//! `AWS::Logs::LogStream`, as it lives in the CloudWatch Logs service's own store.
//!
//! A log stream is a resource of the CloudWatch Logs service: `logs:CreateLogStream` creates
//! it, `logs:DescribeLogStreams` reports it, `logs:PutLogEvents` writes into it and
//! `logs:DeleteLogStream` removes it, and the record of it lives UNDER its log group in that
//! service's state in the same shared engine. So this provider holds a handle to THAT store
//! and nothing of its own: a stream this emulator holds is one Cloud Control API resolves
//! whichever API created it, and a stream CloudWatch Logs no longer holds is one Cloud
//! Control API reports as not found.
//!
//! The type's primary identifier is a COMBINATION: its schema points `primaryIdentifier` at
//! `/properties/LogGroupName` and `/properties/LogStreamName`, so one resource is named by
//! both values joined by the registry's delimiter
//! ([`crate::state::COMPOUND_IDENTIFIER_DELIMITER`]) — which is what every answer of this
//! service names such a resource by and the form every request addresses it in.
//!
//! The resource model is the type's registry schema. Catalog source: the
//! `AWS::Logs::LogStream` resource provider schema's `properties` — `LogGroupName` and
//! `LogStreamName`, both of them `createOnlyProperties` and both carried by the owning
//! record. The type declares no other property, so there is nothing else for this path to
//! apply and nothing else a read can report.

use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::{
    epoch_millis, require_value, LogsState, NativeRefusal, NativeResourceProvider,
    UnreadableReference,
};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::Logs::LogStream";

/// The group the stream belongs to — the FIRST pointer of the type's compound primary
/// identifier (`/properties/LogGroupName`).
const GROUP_PROPERTY: &str = "LogGroupName";

/// The stream's own name — the second pointer of that identifier
/// (`/properties/LogStreamName`).
const STREAM_PROPERTY: &str = "LogStreamName";

/// EVERY property this type's registry schema declares, which is the set a resource model of
/// it may carry and no more.
///
/// Catalog source: the `properties` object of the `AWS::Logs::LogStream` resource provider
/// schema, as CloudFormation publishes it (`DescribeType`) — the two properties of its own
/// identity and nothing else, which is why the model reading below is an identity check.
/// Published for the stage that judges a model's SHAPE against the schema before any handler
/// is asked ([`crate::state::registry_schema`]).
pub const DECLARED_PROPERTIES: &[&str] = &[GROUP_PROPERTY, STREAM_PROPERTY];

/// The shortest and longest a stream's name may be, and the characters it may not carry:
/// the domain the OWNING service's model gives `LogStreamName` (`@length 1..512`,
/// `@pattern ^[^:*]*$`), so nothing is created here under a name that service's own API
/// would refuse.
const MAX_STREAM_NAME_CHARACTERS: usize = 512;

/// The characters `LogStreamName`'s pattern excludes.
const EXCLUDED_STREAM_NAME_CHARACTERS: [char; 2] = [':', '*'];

/// The member of the call this type's READ handler makes that carries the GROUP segment of
/// the reference: the read reports the stream through `logs:DescribeLogStreams`, which takes
/// the group to list the streams of as `logGroupName` — the same `LogGroupName` shape of the
/// owning service the group type's own read submits its reference at.
const READ_GROUP_MEMBER: &str = "logGroupName";

/// The member of that same call which carries the STREAM segment: "the prefix to match"
/// (`logStreamNamePrefix`), whose shape is the owning service's `LogStreamName` — a WIDER
/// domain than a group name's, so each segment is judged against the shape of the member it
/// travels on rather than against one rule for both.
const READ_STREAM_PREFIX_MEMBER: &str = "logStreamNamePrefix";

/// The CloudWatch Logs log streams of one engine, as Cloud Control API resources.
pub struct LogStreamProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<LogsState>,
}

impl LogStreamProvider {
    /// The provider for the CloudWatch Logs store THIS engine carries, or `None` when it
    /// carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        logs::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for LogStreamProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        GROUP_PROPERTY
    }

    /// The owning service's calls this provider's handler makes: `logs:CreateLogStream`,
    /// which creates a stream, and `logs:DeleteLogStream`, which removes one. The type's
    /// schema declares nothing an update could apply, so an update names no call either.
    fn handler_calls(&self, operation: &str) -> &'static [&'static str] {
        // The create.
        const CREATE: &[&str] = &["logs:CreateLogStream"];
        // The removal.
        const REMOVE: &[&str] = &["logs:DeleteLogStream"];
        match operation {
            crate::state::OPERATION_CREATE => CREATE,
            crate::state::OPERATION_DELETE => REMOVE,
            _ => &[],
        }
    }

    /// The stream's own ARN, as the OWNING service builds it from the Region, its group's
    /// name and its own. A value that is not this type's compound identifier names no stream,
    /// so it names no ARN either.
    fn resource_arn(&self, region: &str, identifier: &str) -> Option<String> {
        let (group, stream) = segments(identifier)?;
        Some(logs::log_stream_arn(region, group, stream))
    }

    /// The read forwards the two segments of the reference to `logs:DescribeLogStreams` at
    /// its [`READ_GROUP_MEMBER`] and [`READ_STREAM_PREFIX_MEMBER`], so a reference the owning
    /// service will not take at either member is refused by that call, in that service's own
    /// words, before any stream is looked for: no stream can live in a group of a name
    /// CloudWatch Logs cannot hold, and none can answer to a name of its own that service
    /// will not match on. The segments are judged in the order the type's schema lists its
    /// identifier's pointers, each against the shape of the member it travels on.
    fn unreadable_reference(&self, identifier: &str) -> Option<UnreadableReference> {
        let (group, stream) = segments(identifier)?;
        logs::log_group_name_member_refusal(READ_GROUP_MEMBER, group)
            .or_else(|| logs::log_stream_name_member_refusal(READ_STREAM_PREFIX_MEMBER, stream))
            .map(UnreadableReference::Invalid)
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        // The owning service holds its groups in ONE namespace, so a stream is the same
        // stream whichever Region a call that names it was signed for.
        let _ = region;
        let (group, stream) = segments(identifier)?;
        let store = self.state.lock();
        store
            .log_groups
            .get(group)
            .filter(|held| held.streams.contains_key(stream))
            .map(|_| model(group, stream))
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let _ = region;
        let store = self.state.lock();
        // The streams of the account are held per group, so the window a listing pages over
        // is built across groups rather than read out of one ordered map: the window itself
        // is what is kept — at most `limit` identifiers, the smallest that sort after
        // `after` — so the memory one listing costs is the page it answers and never the
        // inventory behind it.
        let mut window: ::std::collections::BTreeMap<String, (String, String)> =
            ::std::collections::BTreeMap::new();
        for (group, held) in &store.log_groups {
            for stream in held.streams.keys() {
                let identifier = compound_identifier(group, stream);
                if after.is_some_and(|after| identifier.as_str() <= after) {
                    continue;
                }
                if window.len() == limit
                    && window
                        .last_key_value()
                        .is_some_and(|(largest, _)| *largest <= identifier)
                {
                    continue;
                }
                window.insert(identifier, (group.clone(), stream.clone()));
                if window.len() > limit {
                    window.pop_last();
                }
            }
        }
        window
            .into_iter()
            .map(|(identifier, (group, stream))| (identifier, model(&group, &stream)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let (group_name, stream_name) = named(identifier)?;
        read_model(properties, &group_name, &stream_name)?;
        if stream_name.chars().count() > MAX_STREAM_NAME_CHARACTERS
            || stream_name.contains(EXCLUDED_STREAM_NAME_CHARACTERS)
        {
            return Err(NativeRefusal::Invalid(format!(
                "{STREAM_PROPERTY} must be 1 to {MAX_STREAM_NAME_CHARACTERS} characters \
                 carrying neither ':' nor '*'"
            )));
        }
        let mut store = self.state.lock();
        // The stream is created in the owning service's store under its own rules: a group
        // it does not hold has no stream of this name to create, a name that group already
        // holds is that service's own already-exists verdict, and its per-group ceiling
        // bounds what this create may add.
        let group = store
            .log_groups
            .get_mut(&group_name)
            .ok_or(NativeRefusal::NotFound)?;
        if group.streams.contains_key(&stream_name) {
            return Err(NativeRefusal::AlreadyExists);
        }
        if group.streams.len() >= logs::MAX_STREAMS_PER_GROUP {
            return Err(NativeRefusal::AtCapacity(format!(
                "this emulator stores at most {} log streams per log group",
                logs::MAX_STREAMS_PER_GROUP
            )));
        }
        group.streams.insert(
            stream_name.clone(),
            logs::LogStreamRecord::new(region, &group_name, &stream_name, epoch_millis(at)),
        );
        Ok(model(&group_name, &stream_name))
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // One namespace per account, and a stream's record carries no instant a resource
        // model moves: both properties the type declares are its identity, so a model that
        // names the stream it is applied to asks for nothing and any other model names a
        // property this type does not declare.
        let _ = (region, at);
        let (group_name, stream_name) = named(identifier)?;
        read_model(properties, &group_name, &stream_name)?;
        let store = self.state.lock();
        store
            .log_groups
            .get(&group_name)
            .filter(|held| held.streams.contains_key(&stream_name))
            .ok_or(NativeRefusal::NotFound)?;
        Ok(model(&group_name, &stream_name))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let _ = (region, at);
        let (group_name, stream_name) = named(identifier)?;
        let mut store = self.state.lock();
        let group = store
            .log_groups
            .get_mut(&group_name)
            .ok_or(NativeRefusal::NotFound)?;
        // The events the stream held leave with it, so the volume the group holds — what the
        // owning service's metering measures for it — no longer counts them.
        if group.streams.remove(&stream_name).is_none() {
            return Err(NativeRefusal::NotFound);
        }
        Ok(())
    }
}

/// The group and the stream a compound identifier names, or `None` for a value that is not
/// one: both segments are non-empty, and the FIRST delimiter separates them, which is the
/// order the type's schema lists the pointers in.
fn segments(identifier: &str) -> Option<(&str, &str)> {
    let (group, stream) = identifier.split_once(crate::state::COMPOUND_IDENTIFIER_DELIMITER)?;
    (!group.is_empty() && !stream.is_empty()).then_some((group, stream))
}

/// The group and the stream `identifier` names, as owned values — or the refusal for an
/// identifier that names no resource of this type at all, which is one no stream can answer
/// to.
fn named(identifier: &str) -> Result<(String, String), NativeRefusal> {
    let (group, stream) = segments(identifier).ok_or_else(|| {
        NativeRefusal::Invalid(format!(
            "the primary identifier of {TYPE_NAME} is its {GROUP_PROPERTY} and its \
             {STREAM_PROPERTY} joined by '{}'",
            crate::state::COMPOUND_IDENTIFIER_DELIMITER
        ))
    })?;
    Ok((group.to_string(), stream.to_string()))
}

/// The compound primary identifier of the stream `stream` of the group `group`.
fn compound_identifier(group: &str, stream: &str) -> String {
    format!(
        "{group}{}{stream}",
        crate::state::COMPOUND_IDENTIFIER_DELIMITER
    )
}

/// The resource model one log stream renders as: the two properties the type's schema
/// declares, which are its identity.
fn model(group: &str, stream: &str) -> Value {
    Value::Object(Map::from_iter([
        (GROUP_PROPERTY.to_string(), Value::from(group)),
        (STREAM_PROPERTY.to_string(), Value::from(stream)),
    ]))
}

/// Check `properties` is the model of the stream `stream` of the group `group`.
///
/// Both properties the type declares are its identity, so a model may carry them — a read
/// reports them, so a model a client round-trips carries them — but only with the values the
/// resource answers to; any other property is one the type does not declare.
fn read_model(properties: &Value, group: &str, stream: &str) -> Result<(), NativeRefusal> {
    let Some(model) = properties.as_object() else {
        return Err(NativeRefusal::Invalid(format!(
            "{TYPE_NAME} requires a resource model of its properties"
        )));
    };
    for (property, value) in model {
        match property.as_str() {
            GROUP_PROPERTY => require_value(property, value, group)?,
            STREAM_PROPERTY => require_value(property, value, stream)?,
            other => {
                return Err(NativeRefusal::Invalid(format!(
                    "{other} is not a property of {TYPE_NAME}"
                )));
            }
        }
    }
    Ok(())
}

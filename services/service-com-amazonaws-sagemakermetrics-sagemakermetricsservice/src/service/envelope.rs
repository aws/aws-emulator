// SPDX-License-Identifier: CC0-1.0

//! This service's ENVELOPE: the four refusals it answers a request with before any query
//! is evaluated or any data point is read, and the only ones it answers at all.
//!
//! # The rule
//!
//! SageMaker Metrics does not judge a request against the constraints its model declares.
//! A batch operation's contract is that each item's fate is reported FOR THAT ITEM inside
//! a 200 response — `MetricQueryResult.Status`/`Message` for a query,
//! `BatchPutMetricsError.Code`/`MetricIndex` for a data point — so a value outside a
//! modelled bound never refuses the whole call (`skip_validation` in
//! `crate::ops::types`, judged by the two handlers).
//!
//! What this service does refuse is a request whose envelope it cannot serve at all, and
//! each such refusal has its OWN status, body member and constant sentence:
//!
//! * a value the member it is bound to cannot be read from never became a value, which is a
//!   DESERIALIZATION failure: HTTP 400 [`SERIALIZATION_EXCEPTION`], with the sentence in the
//!   body member spelled `Message`, naming the CLASS of JSON that arrived
//!   ([`UNEXPECTED_JSON_TYPE`]). Which values those are is the codec's own reading, so for
//!   the two kinds of member whose wire form decides it — a float, which reads a quoted
//!   non-finite number, and a timestamp, which reads only text its own `@timestampFormat`
//!   parses — this layer asks what the codec asks ([`reads_into`]);
//! * a `BatchPutMetrics` that names no trial component to associate the metrics with:
//!   HTTP 400 with the constant [`INVALID_RESOURCE`] in the body member spelled `message`;
//! * a `BatchPutMetrics` that carries no metric data to ingest: HTTP 500 with the constant
//!   [`UNHANDLED_INTERNAL_FAILURE`];
//! * a `BatchGetMetrics` that carries no queries to answer: HTTP 500 with NO detail member
//!   at all.
//!
//! Each is recorded behaviour: neither operation models any error, so the status, the
//! member a sentence travels in and the sentence itself come from this service's recorded
//! wire rather than from the model. Which MEMBER earns which of them is the fact this
//! module states, so the rule holds for whichever operation declares that member.
//!
//! # The one bound that is this emulator's own
//!
//! Taking the modelled `@length` of a batch off the request boundary takes with it the only
//! thing bounding the WORK one call asks for, so this layer carries that bound itself: a
//! request carrying more than [`crate::state::MAX_ITEMS_PER_REQUEST`] items is answered
//! with the internal-failure refusal above, before any item is read. It is a resource bound
//! and nothing else — it is stated with this service's other store caps, well above the
//! maximum either model declares, so it changes no answer the recorded wire shows.
//!
//! # Why it lives here
//!
//! The framework refuses an absent `@required` member and every constraint violation
//! before a handler runs, and renders all of them as one envelope — HTTP 400
//! `ValidationException` with a lowercase `message` carrying the constraint-walk sentence
//! (`vendor/smithy-server-runtime/src/pre_dispatch.rs`, which is agent-immutable). Its
//! wording seam (`Service::override_refusal`) chooses the error code and the sentence and
//! NOTHING else: not the status, not the body member's name. So the three statuses and the
//! two member spellings above cannot be stated there.
//!
//! `Service::override_operation` is the seam that can: a request this layer refuses is
//! answered here, in front of the modelled operation, through the protocol's own error
//! serializer — so the status, the error code and the body shape are all this service's.
//! Everything this layer does not refuse is served by the generated operation exactly as
//! the routing table would serve it, telemetry and modelled answers included. The same
//! seam, for the same reason, is where
//! `services/service-com-amazonaws-codebuild-codebuild-20161006/src/service/unreadable_body.rs`
//! separates a deserialization refusal from a constraint one.
//!
//! The body's own TOP level is read, then this layer's own item bound above is applied, and
//! only then the members of each ITEM of a batch, which is as deep as this service's inputs
//! nest: a member `Schema` carries its target's KIND but not its target's schema, so the one
//! pairing of a list member to its item shape is stated here ([`item_schema`]) rather than
//! walked generically, and anything nested deeper is left to the codec.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Service,
};

/// The error code a request body this service cannot READ is refused with.
///
/// Not one of this service's modelled shapes: the request never became an input, so no
/// operation ran to answer with an error of its own. It is the code the framework itself
/// names for a JSON body it cannot process (`pre_dispatch.rs`'s catch-all arm), which is
/// the same refusal one unreadable member earns.
pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The error code the refusal of a request that names no resource carries: the same code
/// every other 4xx refusal of this restJson1 service carries, so a client keeps branching
/// on one code.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this service's 5xx envelope refusals carry — the AWS front door's name
/// for a request its application code never answered.
pub const INTERNAL_FAILURE: &str = "InternalFailure";

/// The WHOLE sentence refusing a `BatchPutMetrics` that names no trial component to
/// associate its metrics with. A constant, with no member path, submitted value or
/// modelled expression quoted back — the same shape of constant this service states for a
/// malformed resource reference
/// ([`crate::ops::batch_get_metrics::INVALID_RESOURCE_ARN_MESSAGE`]).
pub const INVALID_RESOURCE: &str = "Invalid resource.";

/// The WHOLE sentence refusing a call that carries no data for the operation to work on.
/// A constant: nothing of the request is interpolated.
pub const UNHANDLED_INTERNAL_FAILURE: &str = "Unhandled internal failure";

/// The fixed half of the sentence refusing a member whose JSON type its modelled shape
/// cannot be read from. The variable half is the CLASS of JSON value that arrived
/// ([`unexpected_json_type`]).
pub const UNEXPECTED_JSON_TYPE: &str = " is not an expected Json type";

/// The sentence refusing a value whose JSON type the member it was bound to cannot be read
/// from: the class of value that arrived, then [`UNEXPECTED_JSON_TYPE`].
///
/// The class, never the value: a refusal states which KIND of JSON arrived
/// (`service_common::json_wire_values::json_value_class`, the workspace's one naming of
/// those classes), so the sentence is the same whatever the caller sent and a long value
/// cannot be echoed back into the response.
fn unexpected_json_type(value: &Value) -> String {
    let class = ::service_common::json_wire_values::json_value_class(value);
    format!("{class}{UNEXPECTED_JSON_TYPE}")
}

/// What this service answers for a request its envelope cannot serve.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// A value whose JSON type the member it was bound to cannot be read from, with the
    /// sentence naming the class of value that arrived.
    Unreadable(String),
    /// The call names no trial component.
    InvalidResource,
    /// The call carries no data for the operation to work on.
    UnhandledInternalFailure,
    /// The call carries nothing to answer, and the refusal states no detail at all.
    NoDetail,
}

/// The ITEM shape each list member of this service's inputs carries.
///
/// A member `Schema` carries its target's KIND but not its target's schema, so the pairing
/// is stated here — which is what lets a value nested in a batch item be read against the
/// shape it is bound to, as the recorded refusal of a data point's `Value` requires.
fn item_schema(member_name: &str) -> Option<&'static Schema> {
    match member_name {
        "MetricQueries" => Some(crate::ops::types::METRIC_QUERY_SCHEMA),
        "MetricData" => Some(crate::ops::types::RAW_METRIC_DATA_SCHEMA),
        _ => None,
    }
}

/// Which refusal a member of this service's inputs earns when the request carries NO value
/// for it — absent, JSON `null`, an empty list or blank text.
///
/// Keyed by the MEMBER rather than by the operation, so the rule holds for whichever
/// operation declares it; the member names are the ones
/// `smithy_query(query="shape",shape="BatchGetMetricsRequest,BatchPutMetricsRequest")`
/// reports as `required`. A member not named here earns no envelope refusal: the
/// operation is served and reports what it could do with the request.
fn unset_member_refusal_for(member_name: &str) -> Option<Refusal> {
    match member_name {
        // The queries a get has nothing to answer without.
        "MetricQueries" => Some(Refusal::NoDetail),
        // The trial component a put associates its metrics with.
        "TrialComponentName" => Some(Refusal::InvalidResource),
        // The data points a put has nothing to ingest without.
        "MetricData" => Some(Refusal::UnhandledInternalFailure),
        _ => None,
    }
}

impl Refusal {
    /// The status this refusal is answered at.
    fn status(&self) -> u16 {
        match self {
            Self::Unreadable(_) | Self::InvalidResource => 400,
            Self::UnhandledInternalFailure | Self::NoDetail => 500,
        }
    }

    /// The error code this refusal names.
    fn code(&self) -> &'static str {
        match self {
            Self::Unreadable(_) => SERIALIZATION_EXCEPTION,
            Self::InvalidResource => VALIDATION_EXCEPTION,
            Self::UnhandledInternalFailure | Self::NoDetail => INTERNAL_FAILURE,
        }
    }

    /// This refusal rendered in the protocol's own error envelope: its status, its code and
    /// its body shape — the member a sentence travels in differs between the three.
    fn response(&self, definition: &dyn ErasedServiceDefinition) -> Option<RtResponse> {
        let protocol = definition.protocol();
        match self {
            Self::Unreadable(message) => protocol.serialize_error(
                &Unreadable {
                    message: message.clone(),
                },
                &UNREADABLE_SCHEMA,
                self.code(),
                self.status(),
            ),
            Self::InvalidResource => protocol.serialize_error(
                &Detail {
                    message: INVALID_RESOURCE.to_string(),
                },
                &DETAIL_SCHEMA,
                self.code(),
                self.status(),
            ),
            Self::UnhandledInternalFailure => protocol.serialize_error(
                &Detail {
                    message: UNHANDLED_INTERNAL_FAILURE.to_string(),
                },
                &DETAIL_SCHEMA,
                self.code(),
                self.status(),
            ),
            Self::NoDetail => {
                protocol.serialize_error(&NoDetail, &NO_DETAIL_SCHEMA, self.code(), self.status())
            }
        }
        .ok()
    }
}

/// The `Message` member a deserialization refusal states its sentence in.
///
/// Spelled with a capital `M`, which is NOT how the framework's own refusals and this
/// service's modelled answers spell it: a client reading a deserialization refusal finds
/// the sentence under no other name.
static UNREADABLE_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#SerializationException$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_json_name("Message");

/// Schema for the deserialization refusal's shape.
///
/// In the `smithy.framework` namespace for the same reason as the scaffold's synthetic
/// shapes (`crate::scaffolded`): the shape is in no service's model, so it must not collide
/// with a modelled one. The wire `__type` is the code handed to the protocol's error
/// serializer, not this id.
static UNREADABLE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#SerializationException",
        "smithy.framework",
        "SerializationException",
    ),
    ShapeType::Structure,
    &[&UNREADABLE_MESSAGE],
);

/// The `message` member this service's other envelope refusals state their sentence in —
/// the lowercase spelling, as the framework's own refusals use.
static DETAIL_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#EnvelopeRefusal$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_json_name("message");

/// Schema for an envelope refusal that states one sentence.
static DETAIL_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#EnvelopeRefusal",
        "smithy.framework",
        "EnvelopeRefusal",
    ),
    ShapeType::Structure,
    &[&DETAIL_MESSAGE],
);

/// Schema for an envelope refusal that states NO detail: a shape with no members, so the
/// body carries only the discriminator the protocol writes.
static NO_DETAIL_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#EnvelopeRefusalWithoutDetail",
        "smithy.framework",
        "EnvelopeRefusalWithoutDetail",
    ),
    ShapeType::Structure,
    &[],
);

/// The body of a deserialization refusal: the one sentence it states, under `Message`.
#[derive(Debug, Clone)]
struct Unreadable {
    message: String,
}

impl SerializableStruct for Unreadable {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&UNREADABLE_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// The body of an envelope refusal that states one sentence, under `message`.
#[derive(Debug, Clone)]
struct Detail {
    message: String,
}

impl SerializableStruct for Detail {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&DETAIL_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// The body of an envelope refusal that states no detail: it writes no member.
#[derive(Debug, Clone)]
struct NoDetail;

impl SerializableStruct for NoDetail {
    fn serialize_members(&self, _s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        Ok(())
    }
}

/// Whether this layer refuses `req`.
pub(super) fn refuses(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
    refusal(definition, req).is_some()
}

/// This layer's refusal of `req`, or `None` when its envelope is one this service serves.
///
/// `None` covers several outcomes deliberately: the envelope is serviceable; the body is
/// not a JSON object, so no member of it can be named (the framework's own rendering of a
/// body it cannot read stands); or the request names no operation this service serves,
/// which is the router's routing failure to answer.
fn refusal(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> Option<Refusal> {
    let key = definition.protocol().routing_key(req).ok()?;
    let input = super::input_schema(key)?;
    let members = submitted_members(req)?;
    // A value that never became a value comes first: a member of the wrong JSON type is
    // not a member carrying nothing. The body's own top level is all this pass reads, so
    // the bound below is still decided before any ITEM of a batch is looked at.
    if let Some(unreadable) = unreadable_top_level_member(input, &members) {
        return Some(unreadable);
    }
    // Then the work one call may ask for, before any item of it is read: the count of a
    // batch member is read off the member, not out of its items.
    if asks_for_more_items_than_served(input, &members) {
        return Some(Refusal::UnhandledInternalFailure);
    }
    // Only then the items themselves, which is work this call has been admitted to ask
    // for: a batch past the bound above is never walked.
    if let Some(unreadable) = unreadable_item_member(input, &members) {
        return Some(unreadable);
    }
    unset_member_refusal(input, &members)
}

/// Whether the request carries more items in a batch member than one call of this service
/// serves ([`crate::state::MAX_ITEMS_PER_REQUEST`]).
///
/// Read off the ALREADY PARSED body, so counting costs nothing a request did not already
/// spend, and read for every list member the input declares an item shape for — so the bound
/// holds for whichever batch an operation takes.
fn asks_for_more_items_than_served(
    input: &Schema,
    members: &serde_json::Map<String, Value>,
) -> bool {
    declared_members(input, members).any(|(member, held)| {
        member
            .member_name()
            .and_then(item_schema)
            .is_some_and(|_| {
                held.as_array()
                    .is_some_and(|items| items.len() > crate::state::MAX_ITEMS_PER_REQUEST)
            })
    })
}

/// The members `req`'s body carries, or `None` when the body is not a JSON object.
///
/// A request with no body at all carries no member rather than being unreadable: it is the
/// request that states nothing, which is what the unset-member rules below answer.
fn submitted_members(req: &RtRequest) -> Option<serde_json::Map<String, Value>> {
    let bytes = req.body().bytes().unwrap_or_default();
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Some(serde_json::Map::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// The quoted values this protocol's JSON codec reads a float from, as it names them
/// (`vendor/smithy-schema-server/src/json.rs`: "only `Infinity`, `-Infinity`, `NaN` can
/// represent a float as a string").
const NON_FINITE_TEXTS: &[&str] = &["NaN", "Infinity", "-Infinity"];

/// Whether a JSON value can be READ into the member it is bound to — the question that
/// separates a value the codec never turned into a value at all from one that was read and
/// then resolved per item.
///
/// It is the workspace's one statement of that question
/// (`service_common::json_wire_values::reads_as`), which answers it from the JSON CLASS and
/// the member's shape type alone, corrected for the two KINDS of member whose WIRE FORM
/// decides it and which that rule therefore leaves to the protocol:
///
/// * a float member reads from a quoted non-finite number ([`NON_FINITE_TEXTS`]) and from no
///   other text. Such a point IS read — and then reported at its own index, because no
///   finite measurement came of it — while a float member holding any other text never
///   became a value at all.
/// * a timestamp member reads from a string only when that string parses in the format bound
///   to that member ([`reads_as_instant`]); the general rule admits every string, because
///   `@timestampFormat` decides which, and this is where that format is read.
///
/// Both corrections ask what this protocol's codec asks, so a value this layer calls
/// readable is one the codec goes on to read.
fn reads_into(member: &Schema, held: &Value) -> bool {
    if matches!(member.shape_type(), ShapeType::Timestamp) {
        return reads_as_instant(member, held);
    }
    if ::service_common::json_wire_values::reads_as(member.shape_type(), held) {
        return true;
    }
    matches!(
        member.shape_type(),
        ShapeType::Float | ShapeType::Double | ShapeType::BigDecimal
    ) && held
        .as_str()
        .is_some_and(|text| NON_FINITE_TEXTS.contains(&text))
}

/// Whether a JSON value became an INSTANT in the timestamp member it is bound to.
///
/// JSON `null` is absence rather than a class, and a number is the epoch a timestamp reads
/// from whatever format is bound to it. A string became an instant exactly when it PARSES in
/// that format — the same call the codec makes to read it
/// (`vendor/smithy-schema-server/src/json.rs`'s `read_timestamp`), so the two cannot
/// disagree about which text is an instant and a string the codec never turned into one is
/// refused here as the deserialization failure it is rather than inside the operation.
fn reads_as_instant(member: &Schema, held: &Value) -> bool {
    match held {
        Value::Null | Value::Number(_) => true,
        Value::String(text) => DateTime::from_str(text, instant_format(member)).is_ok(),
        _ => false,
    }
}

/// The form a timestamp member's text is read in: the `@timestampFormat` the model binds to
/// the member, and for a member the model binds none to, this protocol's default for a
/// timestamp in a JSON body — the epoch second it falls in.
///
/// Resolved off the member SCHEMA, the one thing the codec itself reads the format out of,
/// so a member the model later gives a format to is judged in that format without a change
/// here.
fn instant_format(member: &Schema) -> Format {
    match member.timestamp_format().map(|carried| carried.format()) {
        Some(TimestampFormat::DateTime) => Format::DateTime,
        Some(TimestampFormat::HttpDate) => Format::HttpDate,
        Some(TimestampFormat::EpochSeconds) | None => Format::EpochSeconds,
    }
}

/// The first value of the body's own TOP level the member it is bound to cannot be read
/// from, as the refusal it earns.
///
/// One of the two unreadable passes, and the one that reads nothing inside a batch: it
/// judges each member of the body against the member's own shape, which is what the item
/// bound ([`asks_for_more_items_than_served`]) needs decided before it reads a batch's
/// length. A key naming no member of the shape it is read against is ignored, exactly as
/// the codec ignores it.
fn unreadable_top_level_member(
    input: &Schema,
    members: &serde_json::Map<String, Value>,
) -> Option<Refusal> {
    declared_members(input, members).find_map(|(member, held)| {
        (!reads_into(member, held)).then(|| Refusal::Unreadable(unexpected_json_type(held)))
    })
}

/// The first value carried by an ITEM of a batch that the member it is bound to cannot be
/// read from, as the refusal it earns.
///
/// The second unreadable pass, run only for a request this layer has already admitted:
/// the members of each item of a batch are as deep as this service's inputs nest, and
/// walking them is per-item work, so it is done after the bound on how much of that work
/// one call may ask for. A key naming no member of the item shape is ignored, exactly as
/// the codec ignores it.
fn unreadable_item_member(
    input: &Schema,
    members: &serde_json::Map<String, Value>,
) -> Option<Refusal> {
    declared_members(input, members).find_map(|(member, held)| {
        let item = item_schema(member.member_name()?)?;
        let Value::Array(elements) = held else {
            return None;
        };
        elements.iter().find_map(|element| match element {
            // An item that is not an object cannot be read as the shape it is bound to.
            Value::Object(fields) => declared_members(item, fields).find_map(|(field, held)| {
                (!reads_into(field, held)).then(|| Refusal::Unreadable(unexpected_json_type(held)))
            }),
            Value::Null => None,
            other => Some(Refusal::Unreadable(unexpected_json_type(other))),
        })
    })
}

/// The values of `submitted` that `shape` declares a member for, each with that member's
/// own schema, in the order the request carries them.
fn declared_members<'a>(
    shape: &'a Schema,
    submitted: &'a serde_json::Map<String, Value>,
) -> impl Iterator<Item = (&'a Schema, &'a Value)> {
    submitted.iter().filter_map(move |(key, held)| {
        Some((
            ::service_common::json_wire_values::member_by_wire_name(shape, key)?,
            held,
        ))
    })
}

/// The refusal earned by the first member of `input` that
/// [`unset_member_refusal_for`] names and the request carries no value for, in the order
/// the shape declares its members.
fn unset_member_refusal(
    input: &Schema,
    members: &serde_json::Map<String, Value>,
) -> Option<Refusal> {
    input.members().iter().find_map(|member| {
        let name = member.member_name()?;
        let refusal = unset_member_refusal_for(name)?;
        let wire = member
            .json_name()
            .map(|json_name| json_name.value())
            .unwrap_or(name);
        carries_no_value(members.get(wire)).then_some(refusal)
    })
}

/// Whether the request carries NO value for a member: the key is absent, its value is JSON
/// `null`, or it is an empty collection or blank text — a list with no element and a name
/// with no non-blank character state no more than an absent member does.
///
/// Blank text is read by [`super::member_text::states_no_value`], this service's one
/// reading of that question, so every layer that judges a string reads one of these the
/// same way.
///
/// A value of another JSON class is a value (one the member may not be readable from,
/// which [`unreadable_member`] answers first).
fn carries_no_value(held: Option<&Value>) -> bool {
    match held {
        None | Some(Value::Null) => true,
        Some(Value::Array(elements)) => elements.is_empty(),
        Some(Value::String(text)) => super::member_text::states_no_value(text),
        Some(_) => false,
    }
}

/// The operation this service serves for a request its envelope refuses.
///
/// An [`ErasedOperation`] rather than a response built in `Service::override_operation`,
/// because that seam hands the router a BORROWED operation. It holds no state: the refusal
/// is re-derived from the same request the router passes back into
/// [`ErasedOperation::run`], so one value serves every such request.
pub(super) struct RefusedEnvelope;

/// The one value of [`RefusedEnvelope`] this service hands back.
pub(super) static REFUSED_ENVELOPE: RefusedEnvelope = RefusedEnvelope;

impl ErasedOperation for RefusedEnvelope {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let definition = svc.definition();
        // The same decision `Service::override_operation` made, re-derived from the same
        // request: this operation is reached only when it answered `Some`, so the refusal
        // is found again. Were it somehow not, the request is served exactly as the
        // generated routing table would serve it rather than refused in words nothing
        // decided.
        let refused = refusal(definition, req).and_then(|refusal| refusal.response(definition));
        match refused {
            Some(response) => Box::pin(std::future::ready(response)),
            None => match definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key))
            {
                Some(op) => op.run(svc, req, ctx),
                None => Box::pin(std::future::ready(
                    definition.routing_error_response(req),
                )),
            },
        }
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so a request
        // this layer refuses is authenticated exactly as the operation it addressed is: an
        // unserviceable envelope is no reason to answer an unsigned caller.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Both unreadable passes in the order [`refusal`] runs them, which is the order the
    /// rows below read: the body's own top level, then the items of a batch. The item bound
    /// [`refusal`] applies between the two is pinned on the wire, by the precedence suite in
    /// this crate's integration tests.
    fn unreadable_member(
        input: &Schema,
        members: &serde_json::Map<String, Value>,
    ) -> Option<Refusal> {
        unreadable_top_level_member(input, members)
            .or_else(|| unreadable_item_member(input, members))
    }

    /// RULE: a member earns its refusal when the request carries no value for it, and a
    /// value of any other class is a value.
    #[test]
    fn a_member_carries_no_value_when_it_is_absent_empty_or_blank() {
        // (case, the value held, carries no value)
        let rows: [(&str, Option<Value>, bool); 9] = [
            ("absent", None, true),
            ("null", Some(json!(null)), true),
            ("an empty list", Some(json!([])), true),
            ("a list with an element", Some(json!([{}])), false),
            ("the empty string", Some(json!("")), true),
            ("blank text", Some(json!("   ")), true),
            ("a name", Some(json!("tc-1")), false),
            ("a number", Some(json!(0)), false),
            ("an object", Some(json!({})), false),
        ];
        for (case, held, expected) in rows {
            let actual = carries_no_value(held.as_ref());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected carries_no_value == {expected}, got {actual}"
            );
        }
    }

    /// RULE: the sentence refusing an unreadable value names the CLASS of JSON that
    /// arrived and then the fixed text, so it never carries the value or grows with it.
    #[test]
    fn the_unreadable_sentence_names_the_json_class() {
        // (case, the submitted value, the sentence)
        let rows: [(&str, Value, String); 6] = [
            (
                "a string",
                json!("STRING_VALUE"),
                format!("STRING_VALUE{UNEXPECTED_JSON_TYPE}"),
            ),
            (
                "another string",
                json!("not-a-number"),
                format!("STRING_VALUE{UNEXPECTED_JSON_TYPE}"),
            ),
            ("a number", json!(7), format!("NUMBER_VALUE{UNEXPECTED_JSON_TYPE}")),
            (
                "a flag",
                json!(true),
                format!("BOOLEAN_VALUE{UNEXPECTED_JSON_TYPE}"),
            ),
            (
                "a list",
                json!([1, 2]),
                format!("ARRAY_VALUE{UNEXPECTED_JSON_TYPE}"),
            ),
            (
                "an object",
                json!({"MetricName": "loss"}),
                format!("OBJECT_VALUE{UNEXPECTED_JSON_TYPE}"),
            ),
        ];
        for (case, value, expected) in rows {
            let actual = unexpected_json_type(&value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: which refusal a member earns is a fact about the MEMBER, and each carries its
    /// own status and error code.
    #[test]
    fn every_envelope_refusal_has_its_own_status_and_code() {
        // (case, the refusal, its status, its code)
        let rows: [(&str, Refusal, u16, &str); 4] = [
            (
                "an unreadable member",
                Refusal::Unreadable("x".to_string()),
                400,
                SERIALIZATION_EXCEPTION,
            ),
            (
                "no trial component",
                Refusal::InvalidResource,
                400,
                VALIDATION_EXCEPTION,
            ),
            (
                "no metric data",
                Refusal::UnhandledInternalFailure,
                500,
                INTERNAL_FAILURE,
            ),
            ("no queries", Refusal::NoDetail, 500, INTERNAL_FAILURE),
        ];
        for (case, refusal, status, code) in rows {
            assert_eq!(
                refusal.status(),
                status,
                "case {case:?}: expected status {status}, got {}",
                refusal.status()
            );
            assert_eq!(
                refusal.code(),
                code,
                "case {case:?}: expected code {code}, got {}",
                refusal.code()
            );
        }
    }

    /// RULE: the refusal is about a member the addressed input declares, and the body's own
    /// top level is all that is read.
    #[test]
    fn the_refusal_is_about_a_member_the_input_declares() {
        let get = crate::ops::types::BATCH_GET_METRICS_INPUT_SCHEMA;
        let put = crate::ops::types::BATCH_PUT_METRICS_INPUT_SCHEMA;
        let object = |body: Value| match body {
            Value::Object(members) => members,
            other => panic!("the test bodies are objects, got {other}"),
        };

        assert_eq!(
            unreadable_member(put, &object(json!({"MetricData": "STRING_VALUE"}))),
            Some(Refusal::Unreadable(format!(
                "STRING_VALUE{UNEXPECTED_JSON_TYPE}"
            ))),
            "a string bound to the list member is unreadable"
        );
        assert_eq!(
            unreadable_member(
                put,
                &object(json!({"MetricData": [{"MetricName": "loss", "Value": "not-a-number"}]}))
            ),
            Some(Refusal::Unreadable(format!(
                "STRING_VALUE{UNEXPECTED_JSON_TYPE}"
            ))),
            "a value nested in a batch item is read against the shape it is bound to"
        );
        assert_eq!(
            unreadable_member(put, &object(json!({"MetricData": ["STRING_VALUE"]}))),
            Some(Refusal::Unreadable(format!(
                "STRING_VALUE{UNEXPECTED_JSON_TYPE}"
            ))),
            "an item that is not an object cannot be read as the shape it is bound to"
        );
        assert_eq!(
            unreadable_member(
                put,
                &object(json!({"MetricData": [{"MetricName": "loss", "Timestamp": "1700000000", "Value": 1.0, "Unmodelled": []}]}))
            ),
            None,
            "an instant reads from text its bound format parses, a value from a number, and \
             an unmodelled key is ignored"
        );
        assert_eq!(
            unreadable_member(
                put,
                &object(json!({"MetricData": [{"MetricName": "loss", "Timestamp": "2026-01-01T00:00:00Z", "Value": 1.0}]}))
            ),
            Some(Refusal::Unreadable(format!(
                "STRING_VALUE{UNEXPECTED_JSON_TYPE}"
            ))),
            "text the format bound to the member does not parse never became an instant"
        );
        assert_eq!(
            unreadable_member(put, &object(json!({"MetricData": [], "Unmodelled": 7}))),
            None,
            "a list bound to the list member reads, and an unmodelled key is ignored"
        );
        assert_eq!(
            unset_member_refusal(get, &object(json!({}))),
            Some(Refusal::NoDetail),
            "a get that carries no queries states no detail"
        );
        assert_eq!(
            unset_member_refusal(
                get,
                &object(json!({"MetricQueries": [{"MetricName": "loss"}]}))
            ),
            None,
            "a get carrying a query is served, however little the query says"
        );
        assert_eq!(
            unset_member_refusal(put, &object(json!({"MetricData": [{"Value": 1.0}]}))),
            Some(Refusal::InvalidResource),
            "a put that names no trial component is refused as an invalid resource"
        );
        assert_eq!(
            unset_member_refusal(put, &object(json!({"TrialComponentName": "tc-1"}))),
            Some(Refusal::UnhandledInternalFailure),
            "a put that carries no data is an unhandled internal failure"
        );
        assert_eq!(
            unset_member_refusal(
                put,
                &object(json!({"TrialComponentName": "tc-1", "MetricData": [{"Value": 1.0}]}))
            ),
            None,
            "a put carrying both is served, however little the point says"
        );
    }

    /// A timestamp member carrying `format`, built as a model-driven member schema is, so
    /// the question under test is read off the SCHEMA rather than off this service's one
    /// model.
    const fn timestamp_member(format: Option<TimestampFormat>) -> Schema {
        let member = Schema::new_member(
            ShapeId::from_static("smithy.framework#Probe$Instant", "smithy.framework", "Instant"),
            ShapeType::Timestamp,
            "Instant",
            0,
        );
        match format {
            Some(format) => member.with_timestamp_format(format),
            None => member,
        }
    }

    /// RULE: a value became an instant in a timestamp member when the codec can read it
    /// there — a number or `null` whatever format is bound to the member, and text only in
    /// the format that IS bound to it (the protocol's epoch second when the model binds
    /// none).
    ///
    /// The rows span every format a member can carry against each class of value and each
    /// published spelling of an instant, so the rule is keyed to the member's format rather
    /// than to the one timestamp this service's model declares.
    #[test]
    fn a_timestamp_reads_the_text_the_format_bound_to_it_parses() {
        // The three spellings, one per `@timestampFormat` the trait admits
        // (`aws_smithy_schema::traits::TimestampFormat`), of one instant.
        let epoch_seconds = "1700000000";
        let date_time = "2023-11-14T22:13:20Z";
        let http_date = "Tue, 14 Nov 2023 22:13:20 GMT";
        // (case, the format bound to the member, the value held, reads into it)
        let rows: [(&str, Option<TimestampFormat>, Value, bool); 21] = [
            ("no format: a number is the epoch", None, json!(7), true),
            ("no format: a fraction of a second", None, json!(7.5), true),
            ("no format: null is absence", None, json!(null), true),
            ("no format: epoch text", None, json!(epoch_seconds), true),
            ("no format: date-time text", None, json!(date_time), false),
            ("no format: http-date text", None, json!(http_date), false),
            ("no format: text that is no instant", None, json!("not-a-time"), false),
            ("no format: a flag", None, json!(true), false),
            ("no format: a list", None, json!([7]), false),
            ("no format: an object", None, json!({"secs": 7}), false),
            (
                "epoch-seconds: epoch text",
                Some(TimestampFormat::EpochSeconds),
                json!(epoch_seconds),
                true,
            ),
            (
                "epoch-seconds: date-time text",
                Some(TimestampFormat::EpochSeconds),
                json!(date_time),
                false,
            ),
            (
                "date-time: date-time text",
                Some(TimestampFormat::DateTime),
                json!(date_time),
                true,
            ),
            (
                "date-time: epoch text",
                Some(TimestampFormat::DateTime),
                json!(epoch_seconds),
                false,
            ),
            (
                "date-time: http-date text",
                Some(TimestampFormat::DateTime),
                json!(http_date),
                false,
            ),
            (
                "date-time: a number is still the epoch",
                Some(TimestampFormat::DateTime),
                json!(7),
                true,
            ),
            (
                "http-date: http-date text",
                Some(TimestampFormat::HttpDate),
                json!(http_date),
                true,
            ),
            (
                "http-date: date-time text",
                Some(TimestampFormat::HttpDate),
                json!(date_time),
                false,
            ),
            (
                "http-date: epoch text",
                Some(TimestampFormat::HttpDate),
                json!(epoch_seconds),
                false,
            ),
            (
                "http-date: a number is still the epoch",
                Some(TimestampFormat::HttpDate),
                json!(7),
                true,
            ),
            (
                "http-date: null is absence",
                Some(TimestampFormat::HttpDate),
                json!(null),
                true,
            ),
        ];
        for (case, format, held, expected) in rows {
            let member = timestamp_member(format);
            let actual = reads_into(&member, &held);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected reads_into == {expected}, got {actual}"
            );
        }
    }

    /// RULE: one call serves at most [`crate::state::MAX_ITEMS_PER_REQUEST`] items of a
    /// batch, whichever member carries them; a request past that bound is refused before any
    /// item of it is read.
    #[test]
    fn a_request_past_the_item_bound_is_refused_before_anything_is_read() {
        let served = crate::state::MAX_ITEMS_PER_REQUEST;
        // (case, the addressed input, the member, how many items, refused)
        let rows: [(&str, &Schema, &str, usize, bool); 6] = [
            (
                "queries at the bound",
                crate::ops::types::BATCH_GET_METRICS_INPUT_SCHEMA,
                "MetricQueries",
                served,
                false,
            ),
            (
                "one query past it",
                crate::ops::types::BATCH_GET_METRICS_INPUT_SCHEMA,
                "MetricQueries",
                served + 1,
                true,
            ),
            (
                "points at the bound",
                crate::ops::types::BATCH_PUT_METRICS_INPUT_SCHEMA,
                "MetricData",
                served,
                false,
            ),
            (
                "one point past it",
                crate::ops::types::BATCH_PUT_METRICS_INPUT_SCHEMA,
                "MetricData",
                served + 1,
                true,
            ),
            (
                "far past it",
                crate::ops::types::BATCH_PUT_METRICS_INPUT_SCHEMA,
                "MetricData",
                served * 10,
                true,
            ),
            (
                "the batch the model's own maximum admits",
                crate::ops::types::BATCH_PUT_METRICS_INPUT_SCHEMA,
                "MetricData",
                10,
                false,
            ),
        ];
        for (case, input, member, items, expected) in rows {
            let body = serde_json::Map::from_iter([(
                member.to_string(),
                Value::Array(vec![json!({}); items]),
            )]);
            let actual = asks_for_more_items_than_served(input, &body);
            assert_eq!(
                actual, expected,
                "case {case:?} ({items} item(s)): expected refused == {expected}, got {actual}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` responses carry a TIMESTAMP.
//!
//! RULE: a timestamp member whose model declares `@timestampFormat("date-time")` goes
//! onto the wire as an RFC 3339 / ISO-8601 date-time STRING naming the instant, at this
//! service's millisecond precision — a fractional-second field of exactly three digits,
//! present whatever the instant is (`"2025-06-17T00:00:00.000Z"`) — never as the
//! epoch-seconds NUMBER the protocol defaults to and never with the field elided.
//!
//! The rule is the whole service's, not one shape's. This model declares exactly ONE
//! timestamp shape, `com.amazonaws.mpa#IsoTimestamp`, and it carries
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="IsoTimestamp",
//! model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json")`),
//! so every timestamp member of every shape of this service targets it: a team's
//! `CreationTime` and `LastUpdateTime`, an approver's `ResponseTime` and
//! `LastActivityTime`, an identity source's `CreationTime`, a session's
//! `InitiationTime` / `ExpirationTime` / `CompletionTime`, a deletion's
//! `DeletionStartTime` / `DeletionCompletionTime`, and a policy version's
//! `CreationTime` / `LastUpdatedTime`.
//!
//! WHY IT IS APPLIED HERE. The JSON codec resolves a timestamp's wire form from the
//! MEMBER schema alone (`vendor/smithy-schema-server/src/json.rs` reads
//! `Schema::timestamp_format()`) and falls back to the protocol default of epoch
//! seconds when that schema reports no format. The schemas built from the model carry no
//! member-level format at all — the vendored codegen captures `@length` / `@range` /
//! `@pattern` / `@enum` but emits no timestamp format — so every timestamp of this
//! service went out as a bare number, which a client deserializing a `date-time` member
//! either fails to parse or decodes to a wrong instant. That codegen cannot be edited
//! from a service crate, and the alternative is to hand-write the schema and codec of
//! every structure of this service that holds a timestamp, so the rule is applied once,
//! at the protocol seam `service.rs` already owns: the modelled output is serialized
//! with each timestamp written as the date-time TEXT of its instant, and nothing else
//! about the walk changes — each member keeps the wire name, the order and the schema
//! the model gave it, and no other member kind is touched.
//!
//! Only the modelled SUCCESS body goes through it. No error shape of this model carries a
//! timestamp member, so an error document has nothing to re-render, and request
//! deserialization is untouched: no input member of this model is a timestamp (the one
//! input that carries an instant, `ListSessions`' `InitiationTime` filter, takes it as a
//! `Filter$Value` STRING).

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// Digits in the fractional-second field of every date-time this service writes: this
/// service reports an instant at MILLISECOND precision, so the field is always present and
/// always this wide, whatever the instant's own sub-second value is — a whole second is
/// `...T00:00:00.000Z`, not `...T00:00:00Z`.
const FRACTIONAL_SECOND_DIGITS: usize = 3;

/// The date-time text an instant is reported as: RFC 3339 in UTC, which is what Smithy's
/// `date-time` format is and what [`Format::DateTime`] renders — with the fractional-second
/// field normalised to [`FRACTIONAL_SECOND_DIGITS`].
///
/// An instant with no date-time rendering at all (one outside the range the formatter can
/// express) keeps the protocol's own number rather than being given a fabricated string —
/// see [`IsoTimestampSerializer::write_timestamp`].
fn date_time_text(value: &DateTime) -> Option<String> {
    let rendered = value.fmt(Format::DateTime).ok()?;
    Some(with_fixed_fractional_seconds(&rendered))
}

/// `rendered`, an RFC 3339 UTC date-time, with exactly [`FRACTIONAL_SECOND_DIGITS`]
/// fractional-second digits: [`Format::DateTime`] elides the field whenever its digits are
/// all zeros and writes as many digits as the instant happens to carry otherwise, so the
/// field is padded when it is shorter and cut when it is longer (a finer instant is
/// reported to the precision this service reports, never rounded up into another second).
///
/// Text the formatter did not shape as a `Z`-terminated date-time is handed back as it
/// came: nothing is appended to a string whose seconds field cannot be located.
fn with_fixed_fractional_seconds(rendered: &str) -> String {
    let Some(instant) = rendered.strip_suffix('Z') else {
        return rendered.to_string();
    };
    let (seconds, fraction) = instant.split_once('.').unwrap_or((instant, ""));
    let digits: String = fraction
        .chars()
        .chain(std::iter::repeat('0'))
        .take(FRACTIONAL_SECOND_DIGITS)
        .collect();
    format!("{seconds}.{digits}Z")
}

/// One output shape, serialized with every timestamp rendered as date-time text.
struct IsoTimestamps<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for IsoTimestamps<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut IsoTimestampSerializer { inner: s })
    }
}

/// The serializer the output is walked with: every call is the codec's own, except a
/// timestamp leaf, which is written as its date-time text. A nested structure, list or
/// map re-enters the wrapper, because the codec walks each level with its own serializer
/// and a timestamp is rarely the output's own member.
struct IsoTimestampSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for IsoTimestampSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &IsoTimestamps(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_list(schema, &|inner| write_elements(&mut IsoTimestampSerializer { inner }))
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_map(schema, &|inner| write_entries(&mut IsoTimestampSerializer { inner }))
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    /// The one rewrite: the member's own schema, carrying the date-time text of its
    /// instant. An instant the formatter cannot express falls through to the codec's own
    /// rendering, so a member is never dropped.
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        match date_time_text(value) {
            Some(text) => self.inner.write_string(schema, &text),
            None => self.inner.write_timestamp(schema, value),
        }
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with every
/// timestamp of a modelled response in the `date-time` form this model's one timestamp
/// shape declares.
#[derive(Debug)]
pub struct IsoTimestampRestJson {
    inner: RestJsonServerProtocol,
}

impl IsoTimestampRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it
    /// and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated service definition's own protocol (whose table is
    /// the model's); an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for IsoTimestampRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for IsoTimestampRestJson {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // No input member of this model is a timestamp, so the request side is the
        // framework's own.
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_output(
            &IsoTimestamps(output),
            output_schema,
            operation_name,
            success_status,
        )
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // No error shape of this model carries a timestamp member.
        self.inner
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::date_time_text;
    use aws_smithy_types::DateTime;
    use std::time::{Duration, SystemTime};

    /// RULE: an instant is reported as an RFC 3339 UTC date-time string whose
    /// fractional-second field is always present and always three digits wide. The rows
    /// span what the rendering generalises over: the epoch itself, a whole second, a
    /// sub-second value needing no padding, ones needing one and two zeros of padding, the
    /// largest millisecond value, a value finer than a millisecond, and an instant before
    /// the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time_with_milliseconds() {
        let rows: [(i64, u32, &str); 9] = [
            (0, 0, "1970-01-01T00:00:00.000Z"),
            (1_750_118_400, 0, "2025-06-17T00:00:00.000Z"),
            (1_750_118_400, 500_000_000, "2025-06-17T00:00:00.500Z"),
            (1_750_118_400, 123_000_000, "2025-06-17T00:00:00.123Z"),
            (1_750_118_400, 50_000_000, "2025-06-17T00:00:00.050Z"),
            (1_750_118_400, 7_000_000, "2025-06-17T00:00:00.007Z"),
            (1_750_118_400, 999_000_000, "2025-06-17T00:00:00.999Z"),
            // Finer than a millisecond: reported at this service's precision.
            (1_750_118_400, 123_456_789, "2025-06-17T00:00:00.123Z"),
            (-1, 0, "1969-12-31T23:59:59.000Z"),
        ];
        for (seconds, nanos, expected) in rows {
            let instant = if seconds < 0 {
                SystemTime::UNIX_EPOCH - Duration::new(seconds.unsigned_abs(), 0)
            } else {
                SystemTime::UNIX_EPOCH + Duration::new(seconds.unsigned_abs(), nanos)
            };
            let rendered = date_time_text(&DateTime::from(instant));
            assert_eq!(
                rendered.as_deref(),
                Some(expected),
                "case ({seconds}, {nanos}): expected {expected:?}, got {rendered:?}"
            );
        }
    }
}

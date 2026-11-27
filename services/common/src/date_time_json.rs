// SPDX-License-Identifier: CC0-1.0

//! How a JSON service carries a TIMESTAMP whose model declares the `date-time` form, for
//! every service of that class — so two of them cannot answer one instant differently.
//!
//! # The rule
//!
//! A timestamp member whose model declares `@timestampFormat("date-time")` is read from,
//! and written to, the JSON wire as an RFC 3339 / ISO 8601 date-time STRING
//! (`"2024-01-01T00:00:00Z"`), never as the epoch-seconds NUMBER the `restJson1` and
//! `awsJson` protocols default to. Every generated SDK deserializes such a member with a
//! date-time parser, so a bare number is a value no client of the modelled API can read.
//!
//! # Why a service needs this
//!
//! The JSON codec resolves a timestamp's wire form from the MEMBER schema alone
//! (`vendor/smithy-schema-server/src/json.rs`'s `read_timestamp` / `write_timestamp` read
//! `Schema::timestamp_format()`) and falls back to the protocol default — epoch seconds —
//! when the member schema reports no format. The schemas built from a model carry no
//! member-level format at all (grep `vendor/smithy-macros`: it emits no
//! `with_timestamp_format`), so a declared `date-time` member went out as a number and a
//! date-time string sent for one failed an integer parse on the way in.
//!
//! The structural fix belongs in that codegen, which a service crate cannot edit
//! (`vendor/` is read-only), and the alternative on the service side is to hand-write the
//! schema and the codec of every structure that holds a timestamp. So the correction lives
//! here once, at the protocol seam `service.rs` already owns.
//!
//! # Where a service takes it
//!
//! At `Service::override_protocol`, which hands back a borrow, so the service holds one
//! [`DateTimeRestJson`] (or, for a model whose protocol is `awsJson1_0`,
//! [`DateTimeAwsJson`]) in a field and builds it in its constructor:
//!
//! ```ignore
//! fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
//!     Some(&self.protocol)
//! }
//! ```
//!
//! # Which services it is correct for
//!
//! A service EVERY timestamp member of whose model declares `date-time` — either on one
//! timestamp shape the whole model targets, or on each member individually — takes
//! [`DateTimeMembers::Every`], which the no-argument constructors select. The wrappers
//! supply the format the codegen dropped and never override one a member DOES declare, so a
//! model that later gains an `epoch-seconds` or `http-date` member keeps that member's own
//! form.
//!
//! A model that declares BOTH forms — some members `date-time`, others `epoch-seconds` —
//! takes [`DateTimeMembers::Only`] through `for_members`, naming the members whose model
//! declares `date-time` by their absolute member ids (`"ns#Shape$member"`, the id the
//! generated member schema carries). Every other timestamp member is then read and written
//! in the protocol's own form, which for an `epoch-seconds` member is the form the model
//! declares. The naming is a transcription of the model, so the service that passes one
//! re-derives it from its own model file in a test rather than trusting the list.
//!
//! Nothing else about either walk changes: a member's wire name, its order and its schema
//! stay the ones the model gave it, and no other member kind is touched. The wrappers add
//! one stack frame per nesting level and nothing else — no buffer, no copy of the document
//! and no caller-sized allocation. Nesting DEPTH stays bounded by the codec itself, whose
//! `read_struct`/`read_list`/`read_map` take a depth guard and refuse a body nesting past
//! their own maximum.
//!
//! # Services still carrying their own copy
//!
//! This correction predates this module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it —
//! `grep -rln 'fn date_time_text' services/*/src` lists them (today the Amazon Location,
//! Amazon MQ, Bedrock Data Automation and Multi-party Approval crates, each carrying a
//! module of its own under `src/service/`, spelling the protocol `DateTimeRestJson`,
//! `Iso8601TimestampRestJson` or `IsoTimestampRestJson`). Those copies are to be MIGRATED
//! onto this module and are left in place only because this change does not own those
//! services' issues, each being covered by its own suite. Nothing new should inline the
//! rule again — call this module.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::json::JsonCodec;
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::rpc::HttpRpcServerProtocol;
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};

/// The schema a format-less timestamp member is READ against: a timestamp carrying the
/// `date-time` format its model declares and its generated member schema dropped.
///
/// The codecs read nothing but [`Schema::timestamp_format`] out of the schema handed to
/// `read_timestamp`, so standing this in for a member that reports no format is what makes
/// the declared format reach the parse. Its shape id names this correction rather than any
/// one model's shape, because the member's own id is not what the codec reads.
static DATE_TIME_TIMESTAMP: Schema = Schema::new(
    ShapeId::from_static("smithy.api#Timestamp", "smithy.api", "Timestamp"),
    ShapeType::Timestamp,
)
.with_timestamp_format(TimestampFormat::DateTime);

/// The date-time text an instant is reported as: RFC 3339 in UTC, which is what Smithy's
/// `date-time` format is and what [`Format::DateTime`] renders.
///
/// An instant with no date-time rendering at all (one outside the range the formatter can
/// express) keeps the protocol's own number rather than being given a fake string — see
/// [`DateTimeWriter::write_timestamp`].
///
/// Public because a service that also renders an instant OUTSIDE a modelled member — into a
/// document it serves, say — must render it the same way as the member reporting it does,
/// and one renderer is what makes those two agree.
#[must_use]
pub fn date_time_text(value: &DateTime) -> Option<String> {
    value.fmt(Format::DateTime).ok()
}

/// Which of a model's format-less timestamp members the declared `date-time` form is
/// supplied for — the one thing that differs between a model whose every timestamp is a
/// `date-time` one and a model that declares both forms.
#[derive(Clone, Copy, Debug)]
pub enum DateTimeMembers {
    /// Every format-less timestamp member, for a model whose every timestamp member
    /// declares `date-time`.
    Every,
    /// Only the members named here, by absolute member id (`"ns#Shape$member"`), for a model
    /// that declares `date-time` on some members and another form on others.
    Only(&'static [&'static str]),
}

impl DateTimeMembers {
    /// Whether the member `schema` names is one this selection covers.
    ///
    /// A member schema carries its own absolute id — `"<namespace>#<Shape>$<member>"` — so
    /// one member of one shape is named exactly, and two shapes that declare a member of the
    /// same name are told apart.
    fn covers(self, schema: &Schema) -> bool {
        match self {
            Self::Every => true,
            Self::Only(members) => {
                let id = schema.shape_id().to_string();
                members.contains(&id.as_str())
            }
        }
    }
}

// =================================================================================
// Request side
// =================================================================================

/// The inner deserializer one wrapper forwards to, however it is held.
///
/// The protocol seam hands back an owned `Box<dyn ShapeDeserializer>` while the nested walks
/// hand each level a `&mut dyn ShapeDeserializer`, and both have to wear the same wrapper —
/// so the wrapper is generic over this, and the forwarding body below is written once.
///
/// `pub(crate)` so a second wrapper in this crate ([`crate::query_boolean_values`], which
/// replaces one read of a Query request exactly as this one replaces one read of a JSON
/// request) wears the same seam rather than restating it.
pub(crate) trait InnerDeserializer {
    fn get(&mut self) -> &mut dyn ShapeDeserializer;
    fn peek(&self) -> &dyn ShapeDeserializer;
}

impl InnerDeserializer for Box<dyn ShapeDeserializer + '_> {
    fn get(&mut self) -> &mut dyn ShapeDeserializer {
        &mut **self
    }

    fn peek(&self) -> &dyn ShapeDeserializer {
        &**self
    }
}

impl InnerDeserializer for &mut (dyn ShapeDeserializer + '_) {
    fn get(&mut self) -> &mut dyn ShapeDeserializer {
        &mut **self
    }

    fn peek(&self) -> &dyn ShapeDeserializer {
        &**self
    }
}

/// The deserializer an input is read with: every call is the codec's own, except a timestamp
/// leaf whose member schema declares no format, which is read as a date-time.
///
/// A nested structure, list or map re-enters the wrapper, because the codec walks each level
/// with its own deserializer and a timestamp is rarely the input's own member.
struct DateTimeReader<D>(D, DateTimeMembers);

impl<D: InnerDeserializer> ShapeDeserializer for DateTimeReader<D> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let members = self.1;
        self.0.get().read_struct(schema, &mut |member, d| {
            consumer(member, &mut DateTimeReader(d, members))
        })
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let members = self.1;
        self.0
            .get()
            .read_list(schema, &mut |d| consumer(&mut DateTimeReader(d, members)))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let members = self.1;
        self.0.get().read_map(schema, &mut |key, d| {
            consumer(key, &mut DateTimeReader(d, members))
        })
    }

    fn read_boolean(&mut self, schema: &Schema) -> Result<bool, SerdeError> {
        self.0.get().read_boolean(schema)
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        self.0.get().read_byte(schema)
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        self.0.get().read_short(schema)
    }

    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        self.0.get().read_integer(schema)
    }

    fn read_long(&mut self, schema: &Schema) -> Result<i64, SerdeError> {
        self.0.get().read_long(schema)
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        self.0.get().read_float(schema)
    }

    fn read_double(&mut self, schema: &Schema) -> Result<f64, SerdeError> {
        self.0.get().read_double(schema)
    }

    fn read_big_integer(&mut self, schema: &Schema) -> Result<BigInteger, SerdeError> {
        self.0.get().read_big_integer(schema)
    }

    fn read_big_decimal(&mut self, schema: &Schema) -> Result<BigDecimal, SerdeError> {
        self.0.get().read_big_decimal(schema)
    }

    fn read_string(&mut self, schema: &Schema) -> Result<String, SerdeError> {
        self.0.get().read_string(schema)
    }

    fn read_blob(&mut self, schema: &Schema) -> Result<Blob, SerdeError> {
        self.0.get().read_blob(schema)
    }

    /// The one rewrite on the way in: a member this selection covers whose schema reports no
    /// timestamp format is read with the `date-time` format its model declares instead of the
    /// protocol default. A member that declares a format, and one outside the selection, keep
    /// the reading the framework gives them.
    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        if schema.timestamp_format().is_some() || !self.1.covers(schema) {
            return self.0.get().read_timestamp(schema);
        }
        self.0.get().read_timestamp(&DATE_TIME_TIMESTAMP)
    }

    fn read_document(&mut self, schema: &Schema) -> Result<Document, SerdeError> {
        self.0.get().read_document(schema)
    }

    fn is_null(&self) -> bool {
        self.0.peek().is_null()
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        self.0.get().read_null()
    }

    fn container_size(&self) -> Option<usize> {
        self.0.peek().container_size()
    }
}

// =================================================================================
// Response side
// =================================================================================

/// One shape, serialized with each format-less timestamp this selection covers rendered as
/// date-time text.
struct DateTimes<'a>(&'a dyn SerializableStruct, DateTimeMembers);

impl SerializableStruct for DateTimes<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DateTimeWriter {
            inner: s,
            members: self.1,
        })
    }
}

/// The serializer an output is walked with: every call is the codec's own, except a
/// timestamp leaf, which is written as its date-time text. A nested structure, list or map
/// re-enters the wrapper, because the codec walks each level with its own serializer and a
/// timestamp is rarely the output's own member.
struct DateTimeWriter<'a> {
    inner: &'a mut dyn ShapeSerializer,
    /// The members the rewrite applies to; carried down every nesting level.
    members: DateTimeMembers,
}

impl ShapeSerializer for DateTimeWriter<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_struct(schema, &DateTimes(value, self.members))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let members = self.members;
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut DateTimeWriter { inner, members })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let members = self.members;
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut DateTimeWriter { inner, members })
        })
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

    /// The one rewrite on the way out: the member's own schema, carrying the date-time text
    /// of its instant. A member that declares a format, one outside this selection, or an
    /// instant the formatter cannot express, falls through to the codec's own rendering, so a
    /// member is never dropped.
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        if schema.timestamp_format().is_some() || !self.members.covers(schema) {
            return self.inner.write_timestamp(schema, value);
        }
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

// =================================================================================
// The protocol
// =================================================================================

/// The `restJson1` codec with every timestamp in the `date-time` form the model declares:
/// the model's own protocol in every other respect.
#[derive(Debug)]
pub struct DateTimeRestJson {
    inner: RestJsonServerProtocol,
    members: DateTimeMembers,
}

impl DateTimeRestJson {
    /// Build the protocol once, at service construction: `Service::override_protocol` hands
    /// back a borrow of it and must never build one per request.
    ///
    /// Every format-less timestamp member is corrected, which is what a model whose every
    /// timestamp member declares `date-time` wants; a model that declares both forms takes
    /// [`Self::for_members`] instead.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated service definition's own protocol (whose table is the model's);
    /// an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_members(DateTimeMembers::Every)
    }

    /// The same protocol, correcting ONLY the timestamp members named here — by absolute
    /// member id (`"ns#Shape$member"`) — which is what a model that also declares
    /// `epoch-seconds` members wants: every member outside the list keeps the form the
    /// protocol gives it.
    #[must_use]
    pub fn for_members(members: &'static [&'static str]) -> Self {
        Self::with_members(DateTimeMembers::Only(members))
    }

    fn with_members(members: DateTimeMembers) -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
            members,
        }
    }
}

impl Default for DateTimeRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for DateTimeRestJson {
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
        let inner = self.inner.deserialize_input(request, input_schema)?;
        Ok(Box::new(DateTimeReader(inner, self.members)))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_output(
            &DateTimes(output, self.members),
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
        // Wrapped for the same reason the success body is: the seam is uniform, so a
        // timestamp member an error shape carries cannot silently go out in the protocol's
        // default form. The wrapper is a no-op for a shape that holds no timestamp.
        self.inner.serialize_error(
            &DateTimes(error, self.members),
            error_schema,
            error_code,
            http_status,
        )
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_event_stream_message(
            event_type,
            is_exception,
            &DateTimes(event, self.members),
            event_schema,
        )
    }
}

/// The `awsJson1_0` codec with every timestamp in the `date-time` form the model declares:
/// the model's own protocol in every other respect — the same correction
/// [`DateTimeRestJson`] applies, over the RPC protocol instead of the REST one.
///
/// The wrappers above are the protocol's, not the codec's: both protocols drive the same
/// JSON codec, which resolves a timestamp's wire form from the member schema alone, so one
/// pair of wrappers serves both and neither can drift from the other.
#[derive(Debug)]
pub struct DateTimeAwsJson {
    inner: HttpRpcServerProtocol<JsonCodec>,
    members: DateTimeMembers,
}

impl DateTimeAwsJson {
    /// Build the protocol once, at service construction: `Service::override_protocol` hands
    /// back a borrow of it and must never build one per request.
    ///
    /// The inner protocol is the framework's own `awsJson1_0`
    /// ([`aws_smithy_schema_server::AwsJson10`]), so the protocol id, the routing key (the
    /// `X-Amz-Target` suffix), the media type and the error envelope are the model's.
    #[must_use]
    pub fn new() -> Self {
        Self::with_members(DateTimeMembers::Every)
    }

    /// The same protocol, correcting ONLY the timestamp members named here — the selective
    /// form [`DateTimeRestJson::for_members`] documents.
    #[must_use]
    pub fn for_members(members: &'static [&'static str]) -> Self {
        Self::with_members(DateTimeMembers::Only(members))
    }

    fn with_members(members: DateTimeMembers) -> Self {
        Self {
            inner: ::aws_smithy_schema_server::AwsJson10(),
            members,
        }
    }
}

impl Default for DateTimeAwsJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for DateTimeAwsJson {
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
        let inner = self.inner.deserialize_input(request, input_schema)?;
        Ok(Box::new(DateTimeReader(inner, self.members)))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_output(
            &DateTimes(output, self.members),
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
        self.inner.serialize_error(
            &DateTimes(error, self.members),
            error_schema,
            error_code,
            http_status,
        )
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_event_stream_message(
            event_type,
            is_exception,
            &DateTimes(event, self.members),
            event_schema,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{date_time_text, DateTimeMembers};
    use aws_smithy_schema::{Schema, ShapeId, ShapeType};
    use aws_smithy_types::DateTime;

    /// One member schema as the codegen builds it: the absolute id is the owning shape's id
    /// with `$<member>` appended.
    static OWNED: Schema = Schema::new_member(
        ShapeId::from_static("ns.example#Owner$at", "ns.example", "at"),
        ShapeType::Timestamp,
        "at",
        0,
    );

    /// A member of the same NAME on another shape, which a selection by member id must tell
    /// apart from [`OWNED`].
    static NAMESAKE: Schema = Schema::new_member(
        ShapeId::from_static("ns.example#Other$at", "ns.example", "at"),
        ShapeType::Timestamp,
        "at",
        0,
    );

    /// RULE: `Every` covers any member; `Only` covers exactly the member ids it names, so two
    /// shapes declaring a member of one name are told apart.
    #[test]
    fn a_selection_covers_the_members_it_names() {
        let rows: [(&str, DateTimeMembers, bool, bool); 3] = [
            ("every", DateTimeMembers::Every, true, true),
            (
                "only-the-owner",
                DateTimeMembers::Only(&["ns.example#Owner$at"]),
                true,
                false,
            ),
            ("only-nothing", DateTimeMembers::Only(&[]), false, false),
        ];
        for (case, members, owned, namesake) in rows {
            assert_eq!(
                members.covers(&OWNED),
                owned,
                "case {case:?}: expected the owner's member covered={owned}, got {}",
                members.covers(&OWNED)
            );
            assert_eq!(
                members.covers(&NAMESAKE),
                namesake,
                "case {case:?}: expected the namesake member covered={namesake}, got {}",
                members.covers(&NAMESAKE)
            );
        }
    }

    /// RULE: an instant is reported as an RFC 3339 UTC date-time string. The rows span what
    /// the rendering generalises over: the epoch itself, a whole second, an instant carrying
    /// sub-second digits, and one before the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time() {
        let rows: [(f64, &str); 4] = [
            (0.0, "1970-01-01T00:00:00Z"),
            (1_704_067_200.0, "2024-01-01T00:00:00Z"),
            (1_704_067_200.25, "2024-01-01T00:00:00.25Z"),
            (-1.0, "1969-12-31T23:59:59Z"),
        ];
        for (seconds, expected) in rows {
            let rendered = date_time_text(&DateTime::from_secs_f64(seconds));
            assert_eq!(
                rendered.as_deref(),
                Some(expected),
                "case {seconds}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}

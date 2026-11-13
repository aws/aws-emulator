// SPDX-License-Identifier: CC0-1.0

//! An `@httpQuery`-bound TIMESTAMP read in every form the wire names an instant in, as a
//! wrapper any service can serve its operations behind.
//!
//! # The rule
//!
//! A timestamp bound to the query string carries an INSTANT, and the renderings a client
//! puts that instant on the wire in belong to the protocol rather than to one member: the
//! RFC 3339 `date-time` — with the `Z` zone or with the numeric UTC offset RFC 3339
//! equally admits — and the number of seconds since the epoch, whole or fractional. Every
//! one of them names the same instant, so every one of them binds it, and the member's
//! modelled constraints are then applied to the instant that was sent.
//!
//! A value that names NO instant is a different answer: it never becomes a modelled input
//! at all, so it is not a violation of a constraint the model declares but a value the
//! protocol could not convert, and it is refused as one — the framework's own error
//! envelope, naming the value it could not read ([`unconvertible_value_response`]).
//!
//! # The gap this closes
//!
//! The framework's query-value deserializer picks a timestamp's wire format off the
//! member's schema: `@timestampFormat("epoch-seconds")` reads the number of seconds since
//! the epoch, and a member that declares no format is read in the protocol's default for a
//! URI-bound timestamp, RFC-3339 `date-time`
//! (`vendor/smithy-schema-server/src/http_binding.rs`'s `read_timestamp`). But the schema
//! built from the model carries no member-level `@timestampFormat` at all
//! (`grep -r timestampFormat vendor/smithy-macros/src` finds nothing), so EVERY
//! `@httpQuery` timestamp is read as `date-time` — including the ones whose model declares
//! `epoch-seconds`, and the ones whose own documentation states the epoch-seconds form
//! ("When used in a raw HTTP Query API, it is formatted as epoch time in seconds"). That
//! one reader also takes the `Z` zone only, so the numeric offset RFC 3339 admits — which
//! `datetime.isoformat()` produces, so a real client sends it — is refused as well. A
//! client sending either form is then refused, with HTTP 400, before the operation's
//! handler runs, and told its timestamp is unreadable when the request's real defect lies
//! in another member.
//!
//! [`QueryTimestampForms`] closes that for the operations of a service that bind a
//! timestamp to the query string: a query value in any form the rule above admits is
//! rewritten into the RFC-3339 rendering of the SAME instant before the framework's walk
//! reads the request, so the walk binds the instant the client sent and everything
//! downstream — constraint validation, the handler, the response — sees it. A value the
//! framework's own reader already takes is left exactly as sent.
//!
//! The structural fix is in the vendored codegen — emit the member trait into the schema
//! and widen that one reader — which cannot be made from a service crate. Until it is, a
//! service whose model binds a timestamp to the query string serves those operations
//! through this wrapper (from its `override_operation` seam) rather than inlining the
//! reading again.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, PreDispatchProtocol, Service,
};

/// One modelled operation, served with each of its `@httpQuery`-bound timestamps read in
/// every form an instant reaches the query string in, and with a value that names no
/// instant refused the way the protocol refuses one.
///
/// Nothing else is re-implemented: the request is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
pub struct QueryTimestampForms<O> {
    operation: O,
    /// The `@httpQuery` names of this operation's timestamp members that the generated
    /// schema reads in the default `date-time` format, read off its input schema ONCE at
    /// construction: the schema is a `&'static Schema` baked at codegen, so the answer is
    /// fixed per operation and nothing about it is per-request.
    query_names: Vec<&'static str>,
}

impl<O: Operation> QueryTimestampForms<O> {
    /// Wrap one operation, already constructed from the shared engine.
    ///
    /// An operation that binds NO timestamp to the query string is wrapped harmlessly: it
    /// has no name to read, so every request reaches it untouched.
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            query_names: date_time_query_members(O::INPUT_SCHEMA),
        }
    }
}

impl<O> QueryTimestampForms<O> {
    /// Wrap an operation that is already wrapped in another layer — so it is an
    /// [`ErasedOperation`] but no longer an [`Operation`] — naming the operation `S` whose
    /// input schema the query-bound timestamp members are read off.
    ///
    /// This wrapper belongs OUTSIDE any layer that reads the request's values, because
    /// what it hands on is the request with each timestamp in the form the framework's
    /// walk reads: a layer wrapped inside it sees that request, and one wrapped around it
    /// would see the form the client sent.
    pub fn over<S: Operation>(operation: O) -> Self {
        Self {
            operation,
            query_names: date_time_query_members(S::INPUT_SCHEMA),
        }
    }
}

impl<O> ErasedOperation for QueryTimestampForms<O>
where
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match read_query_timestamps(req, &self.query_names) {
                Reading::AsSent => self.operation.run(svc, req, ctx).await,
                // The rewritten request lives for the call it is read by, which is all the
                // framework's walk borrows it for; `ctx` still carries the request parts
                // the client sent, so a handler reading the URI sees the original.
                Reading::Rewritten(rewritten) => {
                    self.operation.run(svc, &rewritten, ctx).await
                }
                Reading::NoInstant(value) => unconvertible_value_response(
                    svc.definition().wire_protocol(req).envelope(),
                    &value,
                ),
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// What reading a request's query-bound timestamps answers.
enum Reading {
    /// Every bound value is one the framework's own deserializer reads, so the request is
    /// served exactly as it arrived.
    AsSent,
    /// The request with each bound value rewritten into the `date-time` rendering of the
    /// instant it names. BOXED: a whole request dwarfs the other two answers, and this
    /// value is built on the one path that needs it.
    Rewritten(Box<RtRequest>),
    /// A bound value names no instant at all, carried here as the client sent it so the
    /// refusal can name the value it could not convert.
    NoInstant(String),
}

/// Read the query values `req` binds to the timestamp members `names`.
///
/// The value is read the way the framework's query-value deserializer reads it — decoded
/// as one `application/x-www-form-urlencoded` component, which is what makes the verdict
/// here and the binding downstream agree about what the client sent — and the rewritten
/// query keeps every other pair byte for byte.
fn read_query_timestamps(req: &RtRequest, names: &[&str]) -> Reading {
    if names.is_empty() {
        return Reading::AsSent;
    }
    let Some((path, query)) = req.uri().split_once('?') else {
        return Reading::AsSent;
    };

    let mut rewritten = String::with_capacity(query.len());
    let mut changed = false;
    for (index, pair) in query.split('&').enumerate() {
        if index > 0 {
            rewritten.push('&');
        }
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let bound = names
            .iter()
            .any(|name| *name == smithy_server_runtime::percent_decode(raw_name));
        if !bound {
            rewritten.push_str(pair);
            continue;
        }
        let value = smithy_server_runtime::percent_decode(raw_value);
        match read_value(&value) {
            Value::AsSent => rewritten.push_str(pair),
            Value::AsDateTime(rendered) => {
                changed = true;
                rewritten.push_str(raw_name);
                rewritten.push('=');
                rewritten.push_str(&rendered);
            }
            // Refused on the first such value, in the order the query carries them: the
            // request binds no instant for the member, so nothing downstream can run.
            Value::NoInstant => return Reading::NoInstant(value),
        }
    }
    if !changed {
        return Reading::AsSent;
    }

    let Some(mut normalized) = req.try_clone() else {
        // A body this request cannot replay cannot be carried into a rewritten one. The
        // request is then served exactly as it arrived, which is the framework's own
        // refusal of the form it cannot read — traced, because it is the one path where
        // this module does not apply its rule.
        ::tracing::warn!(
            "a query-bound timestamp was left unread: the request cannot be rebuilt"
        );
        return Reading::AsSent;
    };
    match normalized.set_uri(format!("{path}?{rewritten}")) {
        Ok(()) => Reading::Rewritten(::std::boxed::Box::new(normalized)),
        Err(_) => {
            ::tracing::warn!(
                "a query-bound timestamp was left unread: the rewritten URI does not parse"
            );
            Reading::AsSent
        }
    }
}

/// What one query value bound to a timestamp member earns.
enum Value {
    /// The framework's own deserializer reads this value as it stands.
    AsSent,
    /// The value names an instant that reader does not take: the `date-time` rendering of
    /// that same instant, encoded for a query string.
    AsDateTime(String),
    /// The value names no instant in any form the wire carries one in.
    NoInstant,
}

/// Read one decoded query value.
fn read_value(value: &str) -> Value {
    // What the framework's query-value deserializer itself accepts is asked first, from
    // that same reader, so a value it binds is never rewritten and the two cannot
    // disagree about the instant.
    if DateTime::from_str(value, Format::DateTime).is_ok() {
        return Value::AsSent;
    }
    match instant_of(value).and_then(|instant| instant.fmt(Format::DateTime).ok()) {
        // `:` is the only character the rendering carries that a query string reserves.
        Some(rendered) => Value::AsDateTime(rendered.replace(':', "%3A")),
        None => Value::NoInstant,
    }
}

/// The instant a query value names, in any form the wire carries a timestamp in.
///
/// Two forms beyond the `Z`-zoned RFC-3339 `date-time` the framework's reader already
/// takes:
///
/// * `epoch-seconds` — an optionally negative decimal number, with at most one fractional
///   part (`1698778057`, `1698778057.5`, `-86400`). Nothing wider is read as an instant,
///   so a value in an encoding neither a timestamp format nor the protocol describes
///   (`1e9`, `+1`, `inf`) names none.
/// * RFC 3339 with a numeric UTC offset — the civil part read as UTC and shifted by the
///   offset the text declares ([`crate::rfc3339`], the one reading of that offset).
///
/// The calendar conversion itself is the maintained reader's in both cases; what is
/// decided here is only which text is handed to it.
fn instant_of(value: &str) -> Option<DateTime> {
    if is_decimal_number(value) {
        let seconds: f64 = value.parse().ok()?;
        return seconds.is_finite().then(|| DateTime::from_secs_f64(seconds));
    }
    let (civil, offset_seconds) = crate::rfc3339::split_utc_offset(value)?;
    let as_utc = DateTime::from_str(&format!("{civil}Z"), Format::DateTime).ok()?;
    // The civil reading runs `offset_seconds` AHEAD of UTC, so the instant it names is
    // that much earlier than the same fields read as UTC.
    Some(DateTime::from_secs_and_nanos(
        as_utc.secs().checked_sub(offset_seconds)?,
        as_utc.subsec_nanos(),
    ))
}

/// Whether `value` is a plain decimal number: an optional `-`, at least one digit, and at
/// most one `.` followed by at least one digit.
fn is_decimal_number(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|fraction| {
            !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// The `@httpQuery` names of `schema`'s timestamp members that are read in the protocol's
/// default `date-time` format — the members a value in another form has to be rewritten
/// for. A member whose generated schema DOES carry its own `@timestampFormat` is read in
/// THAT format and is left to the framework.
fn date_time_query_members(schema: &'static Schema) -> Vec<&'static str> {
    schema
        .members()
        .iter()
        .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
        .filter(|member| member.timestamp_format().is_none())
        .filter_map(|member| member.http_query().map(|query| query.value()))
        .collect()
}

/// The error type a protocol-level failure to convert a value to the type the model gives
/// its member carries. Not a modelled shape of any service: the value never became a
/// modelled input, so what answers is the framework in front of the service.
const CONVERSION_ERROR_TYPE: &str = "SerializationException";

/// The member the JSON families' framework envelope carries its sentence under —
/// capitalized, which is what distinguishes that envelope from a modelled error's own
/// lowercase `message`.
const FRAMEWORK_MESSAGE_MEMBER: &str = "Message";

/// The status a value that names no instant is refused with.
const UNCONVERTIBLE_STATUS: u16 = 400;

/// How a query value that names no instant is refused: HTTP 400 in the FRAMEWORK's error
/// envelope, naming the value it could not convert.
///
/// Such a value is refused before the request becomes a modelled input, so what answers is
/// not the service's modelled validation error. For the JSON families that envelope
/// carries the sentence under the capitalized `Message` member and names
/// [`CONVERSION_ERROR_TYPE`]: `x-amzn-errortype` for restJson1, which resolves an error
/// shape from that header, and the body's `__type` for the awsJson families, which resolve
/// it from the body and send no such header. A service in any other protocol family keeps
/// that family's own pre-dispatch rendering of a value its codec could not read.
///
/// The value is echoed inside single quotes exactly as the request spelled it, which is
/// the one variable part of the sentence — held, like every other echo of a request value,
/// to the one ceiling this crate quotes one to ([`crate::validation::quoted_echo`]), so an
/// oversized query parameter cannot make the error body as large as itself.
fn unconvertible_value_response(envelope: PreDispatchProtocol<'_>, value: &str) -> RtResponse {
    let message = format!(
        "'{}' can not be converted to Date",
        crate::validation::quoted_echo(value)
    );
    let PreDispatchProtocol::Json {
        content_type,
        errortype_header,
    } = envelope
    else {
        return smithy_server_runtime::protocol_error_response_with(
            envelope,
            ProtocolError::Serde(SerdeError::custom(message)),
            None,
        );
    };
    // Rendered with the maintained JSON writer, so a value carrying a quote, a backslash
    // or a control character cannot break the envelope it is echoed into.
    let body = if errortype_header {
        serde_json::json!({ FRAMEWORK_MESSAGE_MEMBER: message })
    } else {
        serde_json::json!({
            "__type": CONVERSION_ERROR_TYPE,
            FRAMEWORK_MESSAGE_MEMBER: message,
        })
    };
    let mut response = RtResponse::new(
        StatusCode::try_from(UNCONVERTIBLE_STATUS).expect("the status is a valid constant"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", content_type.to_string());
    if errortype_header {
        response
            .headers_mut()
            .insert("x-amzn-errortype", CONVERSION_ERROR_TYPE);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{is_decimal_number, read_value, Value};

    /// How a row states what reading one value answers.
    fn reading(value: &str) -> Option<&'static str> {
        match read_value(value) {
            Value::AsSent => Some("as sent"),
            Value::AsDateTime(_) => Some("rewritten"),
            Value::NoInstant => None,
        }
    }

    /// Every form the wire names an instant in binds, and nothing else does.
    ///
    /// The rows differ along what the rule generalises over: the FORM of the value (the
    /// `date-time` the framework's own reader takes, the epoch-seconds number, the
    /// numeric-offset zone RFC 3339 admits), the sign and magnitude of the offset, the
    /// presence of a fractional second, and the forms that name no instant at all.
    #[test]
    fn every_wire_form_of_an_instant_is_read_and_nothing_else_is() {
        let rows: [(&str, Option<&str>); 21] = [
            // The form the framework's reader already takes: never rewritten.
            ("2023-10-31T19:27:37Z", Some("as sent")),
            ("2023-10-31T19:27:37.250Z", Some("as sent")),
            // Epoch seconds, whole and fractional, at and before the epoch.
            ("1698778057", Some("rewritten")),
            ("1698778057.5", Some("rewritten")),
            ("0", Some("rewritten")),
            ("-86400", Some("rewritten")),
            // RFC 3339 with a numeric zone, in each suffix form and both signs.
            ("2023-10-31T19:27:37+00:00", Some("rewritten")),
            ("2023-10-31T19:27:37+05:30", Some("rewritten")),
            ("2023-10-31T19:27:37-08:00", Some("rewritten")),
            ("2023-10-31T19:27:37+0530", Some("rewritten")),
            ("2023-10-31T19:27:37-08", Some("rewritten")),
            ("2023-10-31T19:27:37.125-08:00", Some("rewritten")),
            // Nothing else names an instant.
            ("1.6e9", None),
            ("+1", None),
            ("-", None),
            ("noon", None),
            ("", None),
            ("1698778057.", None),
            ("2023-10-31T19:27:37", None),
            ("2023-13-31T19:27:37Z", None),
            // A number of a magnitude no calendar renders: read as a number, and then
            // named no instant because nothing can state it.
            (
                "1000000000000000000000000000000000000000000000000000000000000000",
                None,
            ),
        ];
        for (value, expected) in rows {
            assert_eq!(
                reading(value),
                expected,
                "case {value:?}: expected {expected:?}, got {:?}",
                reading(value)
            );
        }
    }

    /// A value in another form is rewritten into the `date-time` rendering of the SAME
    /// instant, encoded for a query string.
    ///
    /// The rows differ along the form read and the instant it names, and the offset rows
    /// are the same instant as the `Z` row they pair with — which is what proves the
    /// offset is applied rather than dropped.
    #[test]
    fn a_rewritten_value_renders_the_instant_it_names() {
        let rows: [(&str, &str); 8] = [
            ("1698778057", "2023-10-31T18%3A47%3A37Z"),
            ("1698778057.5", "2023-10-31T18%3A47%3A37.5Z"),
            ("0", "1970-01-01T00%3A00%3A00Z"),
            ("-86400", "1969-12-31T00%3A00%3A00Z"),
            ("2023-10-31T18:47:37+00:00", "2023-10-31T18%3A47%3A37Z"),
            ("2023-10-31T23:47:37+05:00", "2023-10-31T18%3A47%3A37Z"),
            ("2023-10-31T13:47:37-05:00", "2023-10-31T18%3A47%3A37Z"),
            ("2023-11-01T00:17:37+0530", "2023-10-31T18%3A47%3A37Z"),
        ];
        for (value, expected) in rows {
            let rendered = match read_value(value) {
                Value::AsDateTime(rendered) => rendered,
                _ => panic!("case {value:?}: expected the value to be rewritten"),
            };
            assert_eq!(
                rendered, expected,
                "case {value:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    #[test]
    fn a_decimal_number_is_digits_with_at_most_one_point() {
        for value in ["0", "7", "-7", "1698778057", "1698778057.5", "-0.25"] {
            assert!(
                is_decimal_number(value),
                "case {value:?}: expected a decimal number"
            );
        }
        for value in ["", "-", ".", "1.", ".5", "1.2.3", "+1", "1e9", " 1", "1 "] {
            assert!(
                !is_decimal_number(value),
                "case {value:?}: expected NOT a decimal number"
            );
        }
    }
}

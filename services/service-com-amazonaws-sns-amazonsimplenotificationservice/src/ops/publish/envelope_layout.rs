// SPDX-License-Identifier: CC0-1.0

//! The LAYOUT of a delivered notification: which order its members stand in, and how the
//! document is written out for a subscriber that receives it as a message body.
//!
//! The members of an SNS notification appear in a FIXED order, whichever endpoint receives
//! it — the order every published sample of the envelope carries ("Parsing message formats",
//! Amazon SNS Developer Guide): the message's own metadata (`Type`, `MessageId`, `TopicArn`,
//! `Subject` when the publish carried one, `Message`, `Timestamp`), then the three members a
//! subscriber verifies the notification with (`SignatureVersion`, `Signature` and the
//! signing certificate's URL), then the link that cancels the subscription, and last the
//! published message attributes. A subscriber reading the document with a parser that keeps
//! member order — or matching the body as text — sees that order rather than the order the
//! service happened to assemble the members in, so it is applied here once for every
//! delivery path.
//!
//! The URL members are spelled differently per endpoint kind (`SigningCertURL` /
//! `UnsubscribeURL` in the JSON envelope, `SigningCertUrl` / `UnsubscribeUrl` in a
//! function's event record), so the caller names the two it uses and the rest of the layout
//! is shared.
//!
//! The `Timestamp` a notification carries belongs to the same layout: the samples stamp it
//! in UTC to the millisecond, with exactly three fractional digits, which is the width the
//! parsers a subscriber reads the member with expect.

use super::state;

/// The members a notification carries BEFORE the ones that seal it, in the order the
/// envelope lays them out. `Subject` is present only when the publish carried one, and
/// `SequenceNumber` only for a message published to a FIFO topic; a member the notification
/// does not carry is simply absent, which is what makes this one order serve every
/// notification.
const LEADING_MEMBERS: &[&str] = &[
    "Type",
    "MessageId",
    "TopicArn",
    "Subject",
    "Message",
    "Timestamp",
    "SequenceNumber",
];

/// The member the published message attributes are carried under, which stands last.
const MESSAGE_ATTRIBUTES_MEMBER: &str = "MessageAttributes";

/// `envelope`'s members in the order a delivered notification carries them, with
/// `certificate_member` and `unsubscribe_member` naming the two URL members as this
/// endpoint kind spells them.
///
/// Called with the envelope COMPLETE, because the order is a property of the document that
/// is handed over rather than of the way it was assembled: the signature is taken over the
/// members' VALUES ([`state::notification_signature`]), so re-ordering them changes nothing
/// a subscriber verifies.
pub(super) fn in_canonical_order(
    envelope: ::serde_json::Map<String, ::serde_json::Value>,
    certificate_member: &str,
    unsubscribe_member: &str,
) -> ::serde_json::Map<String, ::serde_json::Value> {
    let layout = LEADING_MEMBERS.iter().copied().chain([
        state::notification_signature::SIGNATURE_VERSION_MEMBER,
        state::notification_signature::SIGNATURE_MEMBER,
        certificate_member,
        unsubscribe_member,
        MESSAGE_ATTRIBUTES_MEMBER,
    ]);
    let mut held: Vec<(String, ::serde_json::Value)> = envelope.into_iter().collect();
    let mut ordered = ::serde_json::Map::with_capacity(held.len());
    for member in layout {
        if let Some(at) = held.iter().position(|(name, _)| name == member) {
            let (name, value) = held.remove(at);
            ordered.insert(name, value);
        }
    }
    // A member this layout does not name keeps its place after the ones it does, so a
    // notification carrying one still carries it.
    ordered.extend(held);
    ordered
}

/// `envelope` written out as a subscriber RECEIVES it: one member per line, two-space
/// indentation and a space on each side of the colon — the layout every published sample of
/// an SNS notification body shows ("Parsing message formats", Amazon SNS Developer Guide).
///
/// The document a queue subscriber receives is its message BODY, so its text is what a
/// consumer reads, matches and logs; it is written through `serde_json`'s own pretty
/// serializer, with the member separator this layout uses in place of that serializer's.
pub(super) fn delivered(envelope: &::serde_json::Map<String, ::serde_json::Value>) -> String {
    let mut written = Vec::new();
    let mut serializer = ::serde_json::Serializer::with_formatter(
        &mut written,
        NotificationFormatter {
            pretty: ::serde_json::ser::PrettyFormatter::with_indent(INDENT),
            depth: 0,
        },
    );
    if ::serde::Serialize::serialize(envelope, &mut serializer).is_err() {
        return compact(envelope);
    }
    String::from_utf8(written).unwrap_or_else(|_| compact(envelope))
}

/// `envelope` written out as ONE line, which is the form a record in a stream or an event
/// payload carries.
pub(super) fn compact(envelope: &::serde_json::Map<String, ::serde_json::Value>) -> String {
    ::serde_json::Value::Object(envelope.clone()).to_string()
}

/// `at` as the `Timestamp` a notification carries it by: the instant in UTC to the
/// MILLISECOND, with the three fractional digits always written out and anything finer
/// dropped — the form every published sample of the envelope shows
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`), which is also what the fixed-width millisecond parsers a
/// subscriber reads the member with take.
///
/// `None` for an instant the timestamp type cannot render, which leaves the member off
/// rather than carrying something that is not an instant.
pub(super) fn timestamp(at: ::std::time::SystemTime) -> Option<String> {
    let since_epoch = at.duration_since(::std::time::UNIX_EPOCH).ok()?;
    let seconds = i64::try_from(since_epoch.as_secs()).ok()?;
    // The whole second alone, so the milliseconds written below are the only fraction the
    // instant carries.
    let whole = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let stem = whole.strip_suffix(UTC_SUFFIX)?;
    let milliseconds = since_epoch.subsec_millis();
    Some(format!("{stem}.{milliseconds:03}{UTC_SUFFIX}"))
}

/// What marks the instant a notification's `Timestamp` carries as UTC.
const UTC_SUFFIX: &str = "Z";

/// What one nesting level of a delivered notification is indented by.
const INDENT: &[u8] = b"  ";

/// What stands between a member's name and its value in a delivered notification: a colon
/// with a space on each side.
const MEMBER_SEPARATOR: &[u8] = b" : ";

/// The pretty layout a delivered notification is written in: `serde_json`'s own pretty
/// formatter for the indentation and the line breaks of the document and of the message
/// attribute MAP, with [`MEMBER_SEPARATOR`] where that formatter would write a colon and a
/// space — and every node deeper than that written on ONE line, which is how the samples
/// write an attribute's own `{"Type":…,"Value":…}` pair.
struct NotificationFormatter<'a> {
    /// The indentation and line breaks of the laid-out nodes.
    pretty: ::serde_json::ser::PrettyFormatter<'a>,
    /// How many objects or arrays deep the writer stands, so a node past
    /// [`LAID_OUT_DEPTH`] is written on one line.
    depth: usize,
}

/// How deep the layout breaks a node over lines: the document itself (depth 1) and the
/// message attribute map (depth 2), while an attribute's own value object stands on the
/// map's line.
const LAID_OUT_DEPTH: usize = 2;

impl NotificationFormatter<'_> {
    /// Whether the node the writer stands in is broken over lines.
    fn laid_out(&self) -> bool {
        self.depth <= LAID_OUT_DEPTH
    }
}

impl ::serde_json::ser::Formatter for NotificationFormatter<'_> {
    fn begin_object_value<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if self.laid_out() {
            return writer.write_all(MEMBER_SEPARATOR);
        }
        writer.write_all(b":")
    }

    fn begin_array<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth += 1;
        if self.laid_out() {
            return self.pretty.begin_array(writer);
        }
        writer.write_all(b"[")
    }

    fn end_array<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        let laid_out = self.laid_out();
        self.depth = self.depth.saturating_sub(1);
        if laid_out {
            return self.pretty.end_array(writer);
        }
        writer.write_all(b"]")
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if self.laid_out() {
            return self.pretty.begin_array_value(writer, first);
        }
        if first {
            return Ok(());
        }
        writer.write_all(b",")
    }

    fn end_array_value<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if self.laid_out() {
            return self.pretty.end_array_value(writer);
        }
        Ok(())
    }

    fn begin_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth += 1;
        if self.laid_out() {
            return self.pretty.begin_object(writer);
        }
        writer.write_all(b"{")
    }

    fn end_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        let laid_out = self.laid_out();
        self.depth = self.depth.saturating_sub(1);
        if laid_out {
            return self.pretty.end_object(writer);
        }
        writer.write_all(b"}")
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if self.laid_out() {
            return self.pretty.begin_object_key(writer, first);
        }
        if first {
            return Ok(());
        }
        writer.write_all(b",")
    }

    fn end_object_value<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if self.laid_out() {
            return self.pretty.end_object_value(writer);
        }
        Ok(())
    }
}

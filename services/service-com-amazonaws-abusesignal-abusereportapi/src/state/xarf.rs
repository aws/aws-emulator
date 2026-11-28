// SPDX-License-Identifier: CC0-1.0

//! The reading of the XARF document an abuse report carries.
//!
//! `CreateAbuseReport` takes its report as `ReportData$xarf`, "a JSON object that conforms
//! to the XARF v4 specification" — an untyped `Document`, so the framework's own constraint
//! walk cannot say anything about what is inside it. Three questions about that document
//! are answered here, and nowhere else, so the create path and the stored record cannot
//! disagree about them:
//!
//! * [`payload_fingerprint`] — the digest two submissions under one idempotency token are
//!   compared by, which is what tells a repeat of a call from "an idempotency token was
//!   reused with a different payload";
//! * [`schema_violations`] — the XARF core fields the document has to declare, whose
//!   absence makes the report NOT well-formed (the model: `ValidationException` "indicates
//!   that the request is not well-formed");
//! * [`unmet_requirements`] — the submission requirements the model publishes for a report
//!   that IS well-formed, each with the dot-namespaced code and the JSON Pointer the model
//!   gives it (`smithy_query(query="doc",shape="SubmissionRequirement",member="code")`).

use std::net::IpAddr;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;

/// The XARF core fields the model names as required: "the document must include the XARF
/// core fields, including the required `category` and `type` fields that classify the
/// report (for example, `category` `content` with `type` `phishing`)".
pub const XARF_CORE_FIELDS: [&str; 2] = ["category", "type"];

/// `rule.source_identifier.too_long`'s threshold, as the model states it: "The value
/// exceeds the maximum accepted length of 253 characters (the DNS hostname limit)."
pub const SOURCE_IDENTIFIER_MAX_CHARS: usize = 253;

/// The characters `rule.source_identifier.forbidden_characters` names: "The value contains
/// characters that are never valid in an address (`{`, `}`, `*`, or `` ` ``)."
pub const FORBIDDEN_SOURCE_IDENTIFIER_CHARACTERS: [char; 4] = ['{', '}', '*', '`'];

/// `rule.timestamp.in_future`'s tolerance, as the model states it: "The value is dated
/// further into the future than the accepted clock-skew tolerance (a default of 1 hour)."
pub const CLOCK_SKEW_TOLERANCE: Duration = Duration::from_secs(60 * 60);

/// `rule.timestamp.too_old`'s window, as the model states it: "The value predates the
/// submission lookback window (a default of 90 days)."
pub const SUBMISSION_LOOKBACK_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The submission-requirement codes the model publishes, each "a stable, dot-namespaced
/// identifier for the submission requirement that the report did not meet".
pub const SOURCE_IDENTIFIER_TOO_LONG: &str = "rule.source_identifier.too_long";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const SOURCE_IDENTIFIER_FORBIDDEN_CHARACTERS: &str =
    "rule.source_identifier.forbidden_characters";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const SOURCE_IDENTIFIER_MALFORMED: &str = "rule.source_identifier.malformed";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const TIMESTAMP_UNPARSEABLE: &str = "rule.timestamp.unparseable";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const TIMESTAMP_IN_FUTURE: &str = "rule.timestamp.in_future";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const TIMESTAMP_TOO_OLD: &str = "rule.timestamp.too_old";
/// See [`SOURCE_IDENTIFIER_TOO_LONG`].
pub const EVIDENCE_CONTENT_TYPE_MISMATCH: &str = "rule.evidence.content_type_mismatch";

/// How many clauses a refusal states: `SubmissionRequirementList` is `@length(1..50)`, so
/// a report that breaks more requirements than that (one evidence item each, say) is still
/// refused with a list the model admits rather than one no client can read.
const MAX_UNMET_REQUIREMENTS: usize = 50;

/// The report member `rule.source_identifier.*` is about, as the JSON Pointer the model
/// says `path` carries.
const SOURCE_IDENTIFIER_PATH: &str = "/source_identifier";

/// The report member `rule.timestamp.*` is about; see [`SOURCE_IDENTIFIER_PATH`].
const TIMESTAMP_PATH: &str = "/timestamp";

/// What separates the parts of one digested field, so a value carrying the separator of
/// the field list cannot be read as two fields.
const FIELD_SEPARATOR: char = '\u{1f}';

/// One thing wrong with a submitted report: the requirement code (absent where the
/// requirement publishes none), the JSON Pointer to the member it is about, and the
/// sentence a client reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDefect {
    /// The published identifier of the requirement, where it has one.
    pub code: Option<String>,
    /// A JSON Pointer to the report member; the empty string is the report as a whole.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
}

impl ReportDefect {
    /// A defect of the member at `path` that publishes no code.
    fn at(path: &str, message: String) -> Self {
        Self {
            code: None,
            path: path.to_string(),
            message,
        }
    }

    /// A defect of the member at `path` that the model publishes `code` for.
    fn requirement(code: &str, path: &str, message: String) -> Self {
        Self {
            code: Some(code.to_string()),
            path: path.to_string(),
            message,
        }
    }
}

/// Every way `report` fails to be a well-formed XARF document — an empty list when it is
/// one.
///
/// Only the fields the model calls required are checked: the document is otherwise
/// opaque, so a field this service knows nothing about is carried as submitted.
pub fn schema_violations(report: &Document) -> Vec<ReportDefect> {
    if !matches!(report, Document::Object(_)) {
        return vec![ReportDefect::at(
            "",
            "The report must be a JSON object that conforms to the XARF v4 specification."
                .to_string(),
        )];
    }
    XARF_CORE_FIELDS
        .iter()
        .filter(|field| {
            member(report, field)
                .and_then(text)
                .is_none_or(|value| value.trim().is_empty())
        })
        .map(|field| {
            ReportDefect::at(
                &format!("/{field}"),
                format!(
                    "The report must declare the XARF core field {field} as a non-empty string."
                ),
            )
        })
        .collect()
}

/// Every submission requirement `report` does not meet at `now` — an empty list when it
/// meets all of them, and never more clauses than the model's own list admits
/// ([`MAX_UNMET_REQUIREMENTS`]).
///
/// A requirement is about a member, so one the document does not carry is not checked:
/// the model declares only `category` and `type` required, and reports nothing about a
/// member that is absent.
pub fn unmet_requirements(report: &Document, now: SystemTime) -> Vec<ReportDefect> {
    let mut unmet = Vec::new();
    if let Some(defect) = member(report, "source_identifier")
        .and_then(text)
        .and_then(source_identifier_defect)
    {
        unmet.push(defect);
    }
    if let Some(defect) = member(report, "timestamp")
        .and_then(text)
        .and_then(|value| timestamp_defect(value, now))
    {
        unmet.push(defect);
    }
    let remaining = MAX_UNMET_REQUIREMENTS.saturating_sub(unmet.len());
    unmet.extend(evidence_defects(report).into_iter().take(remaining));
    unmet
}

/// The requirement a `source_identifier` does not meet, if any.
///
/// The three are checked in the order the model lists them, and at most one is reported:
/// a value that is too long, or carries a character never valid in an address, is also
/// not an address, and the more specific finding is the useful one.
fn source_identifier_defect(value: &str) -> Option<ReportDefect> {
    if value.chars().count() > SOURCE_IDENTIFIER_MAX_CHARS {
        return Some(ReportDefect::requirement(
            SOURCE_IDENTIFIER_TOO_LONG,
            SOURCE_IDENTIFIER_PATH,
            format!(
                "The source_identifier exceeds the maximum accepted length of \
                 {SOURCE_IDENTIFIER_MAX_CHARS} characters."
            ),
        ));
    }
    if value.contains(FORBIDDEN_SOURCE_IDENTIFIER_CHARACTERS) {
        return Some(ReportDefect::requirement(
            SOURCE_IDENTIFIER_FORBIDDEN_CHARACTERS,
            SOURCE_IDENTIFIER_PATH,
            "The source_identifier contains characters that are never valid in an address."
                .to_string(),
        ));
    }
    (!names_an_address(value)).then(|| {
        ReportDefect::requirement(
            SOURCE_IDENTIFIER_MALFORMED,
            SOURCE_IDENTIFIER_PATH,
            "The source_identifier is not a valid IPv4 address, IPv6 address, URL, or domain \
             name."
                .to_string(),
        )
    })
}

/// Whether `value` is one of the four forms the model admits for a `source_identifier`:
/// an IPv4 address, an IPv6 address, a URL, or a domain name.
///
/// The address literals come from the standard library's own parser, the URL from the
/// maintained `url` crate (a URL identifying an abused resource names a host), and the
/// domain name from the workspace's one reading of that grammar
/// ([`service_common::email_address::is_domain_name`]).
fn names_an_address(value: &str) -> bool {
    IpAddr::from_str(value).is_ok()
        || ::service_common::email_address::is_domain_name(value)
        || ::url::Url::parse(value).is_ok_and(|url| url.has_host())
}

/// The requirement a `timestamp` does not meet at `now`, if any.
fn timestamp_defect(value: &str, now: SystemTime) -> Option<ReportDefect> {
    let Some(stated) = rfc3339_epoch_seconds(value) else {
        return Some(ReportDefect::requirement(
            TIMESTAMP_UNPARSEABLE,
            TIMESTAMP_PATH,
            "The timestamp could not be parsed as an RFC 3339 date-time.".to_string(),
        ));
    };
    let now = epoch_seconds(now);
    if stated.saturating_sub(now) > seconds(CLOCK_SKEW_TOLERANCE) {
        return Some(ReportDefect::requirement(
            TIMESTAMP_IN_FUTURE,
            TIMESTAMP_PATH,
            "The timestamp is dated further into the future than the accepted clock-skew \
             tolerance of 1 hour."
                .to_string(),
        ));
    }
    (now.saturating_sub(stated) > seconds(SUBMISSION_LOOKBACK_WINDOW)).then(|| {
        ReportDefect::requirement(
            TIMESTAMP_TOO_OLD,
            TIMESTAMP_PATH,
            "The timestamp predates the submission lookback window of 90 days. For content \
             that is still accessible, set timestamp to the time you most recently observed \
             it."
                .to_string(),
        )
    })
}

/// Every evidence item whose payload is not what its `content_type` declares.
///
/// The claim is made only where it can be established: the item has to carry both a
/// declared `content_type` and a `payload` that decodes, and the decoded bytes have to be
/// RECOGNISED by the maintained `infer` sniffer. Nothing is reported about a payload the
/// sniffer has no signature for (plain text, say): a requirement that cannot be decided
/// is not a requirement the report failed.
fn evidence_defects(report: &Document) -> Vec<ReportDefect> {
    let Some(Document::Array(items)) = member(report, "evidence") else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        // The walk stops at the number of clauses a refusal can state, so a report
        // carrying thousands of evidence items cannot make one refusal arbitrarily large.
        .take(MAX_UNMET_REQUIREMENTS)
        .filter_map(|(position, item)| {
            let declared = member(item, "content_type").and_then(text)?;
            let payload = member(item, "payload").and_then(text)?;
            let decoded = ::aws_smithy_types::base64::decode(payload).ok()?;
            let detected = ::infer::get(&decoded)?.mime_type();
            (!declared.eq_ignore_ascii_case(detected)).then(|| {
                ReportDefect::requirement(
                    EVIDENCE_CONTENT_TYPE_MISMATCH,
                    &format!("/evidence/{position}"),
                    format!(
                        "The decoded payload of the evidence item does not match its declared \
                         content_type {declared}; its content is {detected}."
                    ),
                )
            })
        })
        .collect()
}

/// The digest two submissions are compared by: a function of the report document and the
/// `correlationId` the submission carried, and of nothing else.
///
/// The canonicalisation and the mixing are the workspace's one digest of a caller-supplied
/// document ([`service_common::document_fingerprint`]), so a document's member order — no
/// part of its value, and not even stable across two deserializations of one request —
/// cannot make one payload digest two ways.
pub fn payload_fingerprint(report: &Document, correlation_id: Option<&str>) -> String {
    // An absent `correlationId` is its own leading part, so a submission that omits it and
    // one that carries a value cannot digest alike.
    let correlation = match correlation_id {
        Some(value) => format!("correlationId{FIELD_SEPARATOR}{value}"),
        None => "correlationId-absent".to_string(),
    };
    ::service_common::document_fingerprint::document_fingerprint(&[&correlation], report)
}

/// The member `name` of `document`, or `None` when `document` is no object or carries no
/// such member.
fn member<'a>(document: &'a Document, name: &str) -> Option<&'a Document> {
    match document {
        Document::Object(members) => members.get(name),
        _ => None,
    }
}

/// `document` as the string it is, or `None` when it is any other kind of value.
fn text(document: &Document) -> Option<&str> {
    match document {
        Document::String(value) => Some(value.as_str()),
        _ => None,
    }
}

/// The instant an RFC 3339 date-time names, in seconds since the epoch, or `None` when
/// the text names none.
///
/// The calendar conversion comes from the maintained library, and the numeric UTC offset
/// RFC 3339 also admits is resolved through the tree's one reader of that offset
/// ([`service_common::rfc3339::split_utc_offset`]).
fn rfc3339_epoch_seconds(value: &str) -> Option<i64> {
    let (civil, offset_seconds) = ::service_common::rfc3339::split_utc_offset(value)?;
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    parsed.secs().checked_sub(offset_seconds)
}

/// `at` in seconds since the epoch.
fn epoch_seconds(at: SystemTime) -> i64 {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

/// `span` in whole seconds, as a comparison against an epoch second needs it.
fn seconds(span: Duration) -> i64 {
    i64::try_from(span.as_secs()).unwrap_or(i64::MAX)
}

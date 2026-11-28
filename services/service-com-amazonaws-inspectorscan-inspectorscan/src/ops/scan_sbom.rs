// SPDX-License-Identifier: CC0-1.0

//! @op ScanSbom
//!
//! Scans a submitted CycloneDX 1.5 SBOM against the advisory data held in
//! [`crate::state`] and answers the vulnerability report for it.
//!
//! The whole operation is: parse the document, refuse anything that is not a
//! CycloneDX 1.5 BOM, resolve each `components[]` entry to a package coordinate
//! (purl ecosystem + the distribution that ecosystem scopes its packages by,
//! where it scopes them by one + ecosystem-canonical name + version), look that
//! coordinate up in the state's advisory index, and render the matched
//! advisories — plus
//! the scan summary and the components nothing could be concluded about — in
//! the requested `outputFormat`. Nothing in the report is a literal: every
//! rating, CWE and fixed-in version is a field of the matched
//! [`crate::state::VulnRecord`], every count is a total over the matched set,
//! and every `affects[].ref` is a `bom-ref` this scan assigned to a component
//! of the report.
//!
//! The report ADDS to the submitted document rather than restating it, so a
//! member it is built from that carries a JSON type CycloneDX does not allow
//! there is refused with the same `ValidationException` a mistyped `components`
//! gets ([`validate_report_members`]) — a value of the wrong type cannot carry
//! the scan's own data beside it, and a report that wrote the scan's data in its
//! place would state the caller sent something it did not. The members the scan
//! reads a VALUE off are refused by the same rule ([`submitted_string`]): one of
//! the wrong type is not a value that member can hold, and reading it as absent
//! would resolve a component on a coordinate the caller never submitted.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata
//! only; this handler reads it for the presented access key id it stamps on the
//! audit record a completed scan emits.
//!
//! ## Authorization
//!
//! AWS decides a signed call against the CALLING principal's identity-based IAM
//! policies before the service acts on the request, so this handler asks
//! [`crate::service::authorization`] FIRST — before the submitted document is
//! read, and with the resource built from the request alone — and answers the
//! modeled `AccessDeniedException` (HTTP 403) with IAM's own sentence when no
//! readable policy grants `inspector-scan:ScanSbom`. A refused call carries no
//! `sbom` member and leaves no scan behind. The decision, the documents it is
//! taken from and the wording all live in that module and the shared evaluator
//! behind it; nothing of either is spelled here.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ScanSbom`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-inspectorscan-inspectorscan/smithy-model/com-amazonaws-inspectorscan-inspectorscan.json")
//! ```
//!
//! Scans a provided CycloneDX 1.5 SBOM and reports on any vulnerabilities discovered in
//! that SBOM. You can generate compatible SBOMs for your resources using the Amazon
//! Inspector SBOM generator. The output of this action reports NVD and CVSS scores when NVD
//! and CVSS scores are available. Because the output reports both scores, you might notice
//! a discrepency between them. However, you can triage the severity of either score
//! depending on the vendor of your choosing.
//!
//! ## Input
//!
//! * `outputFormat` (OutputFormat, one of: CYCLONE_DX_1_5 | INSPECTOR | INSPECTOR_ALT) — The
//!   output format for the vulnerability report.
//! * `sbom` (Sbom) **required** — The JSON file for the SBOM you want to scan. The SBOM must
//!   be in CycloneDX 1.5 format. This format limits you to passing 2000 components before
//!   throwing a ValidException error.
//!
//! ## Output
//!
//! * `sbom` (Sbom) — The vulnerability report for the scanned SBOM.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The request has failed validation due to missing
//!   required fields or having invalid inputs.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{OutputFormat, ScanRecord, VulnRecord};
use aws_smithy_types::{Document, Number};
use serde_json::{json, Map, Value};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ScanSbomError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspectorscan-inspectorscan.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ScanSbomError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.inspectorscan#ScanSbom")]
#[derive(Debug)]
pub enum ScanSbomErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::scan_sbom::ScanSbomError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ScanSbomError = crate::scaffolded::Scaffolded<ScanSbomErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ScanSbomErrorModeled> for ScanSbomError {
    fn from(modeled: ScanSbomErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

// ---------------------------------------------------------------------------
// Documented limits and constants
// ---------------------------------------------------------------------------

/// The maximum number of components one scan may carry, counting EVERY entry
/// of the component tree (a CycloneDX assembly may nest `components[]`), not
/// just the top level.
///
/// 5000 is what the service ENFORCES: a document of exactly this many components
/// is scanned and one more is refused. The `sbom` member's own modeled
/// documentation still says "This format limits you to passing 2000 components
/// before throwing a ValidException error", which is stale relative to the
/// service — a 2001-component document is accepted — so the limit stated in the
/// refusal and the limit checked here are this one.
const MAX_COMPONENTS: usize = 5000;

/// The deepest `components[]` nesting one scan resolves. A CycloneDX assembly
/// tree is shallow; this bounds the recursive walk over caller-chosen input.
const MAX_COMPONENT_NESTING: usize = 16;

/// The longest `bom-ref` a component may carry.
///
/// CHOSEN, not documented: CycloneDX states no length for `bom-ref` (it is a
/// document-local identifier, and a `urn:uuid:` one is 45 characters), so this
/// is an emulator bound, set well above any identifier a generator emits. It is
/// what keeps one request's allocation bounded: see [`submitted_bom_ref`].
const MAX_BOM_REF_LEN: usize = 512;

/// The deepest JSON nesting a submitted `sbom` document may carry.
///
/// `sbom` is a Smithy `document` — arbitrary caller-supplied JSON — and both
/// conversions below walk it recursively, so the depth is bounded and a deeper
/// document is refused with a `ValidationException` rather than risking the
/// thread stack. A CycloneDX 1.5 BOM needs only a handful of levels.
const MAX_SBOM_DEPTH: usize = 64;

/// Extra depth the rendered report may add over the document it was built from
/// (the `vulnerabilities[]` subtree this operation appends is a fixed four
/// levels deep).
const REPORT_DEPTH_HEADROOM: usize = 8;

/// The only `bomFormat` this operation accepts, per the `sbom` member's docs
/// ("The SBOM must be in CycloneDX 1.5 format") and the CycloneDX specification,
/// which fixes `bomFormat` to this literal.
const CYCLONEDX_BOM_FORMAT: &str = "CycloneDX";

/// The only CycloneDX `specVersion` this operation accepts, per the same doc
/// sentence.
const CYCLONEDX_SPEC_VERSION: &str = "1.5";

/// The values CycloneDX 1.5 allows for a component's required `type` member —
/// the specification's `classification` enumeration, in its own order.
///
/// A `components[]` entry whose `type` is absent or outside this list is not a
/// CycloneDX 1.5 component, so the document is refused rather than scanned.
const CYCLONEDX_COMPONENT_TYPES: &[&str] = &[
    "application",
    "framework",
    "library",
    "container",
    "platform",
    "operating-system",
    "device",
    "device-driver",
    "firmware",
    "file",
    "machine-learning-model",
    "data",
];

/// How a CVSS vector states the specification version it is written in: the
/// version follows this literal and is terminated by [`CVSS_VECTOR_SEPARATOR`],
/// per the CVSS specification's own vector-string grammar.
const CVSS_VECTOR_PREFIX: &str = "CVSS:";

/// The character separating a CVSS vector string's fields, and so the one
/// terminating the version its prefix states.
const CVSS_VECTOR_SEPARATOR: char = '/';

/// The CVSS notations a vector's version prefix can state: each specification
/// version as the vector spells it, paired with the method value the CycloneDX
/// 1.5 schema enumerates for that version.
///
/// ONE table, read by both report forms through [`cvss_notation`], so a notation
/// added here reaches the CycloneDX `ratings[].method` and the Inspector-native
/// `version` together instead of one form rating a vector the other drops.
const CVSS_NOTATIONS: &[(&str, &str)] = &[("4.0", "CVSSv4"), ("3.1", "CVSSv31"), ("3.0", "CVSSv3")];

/// The notation a CVSS vector is written in — its specification version and the
/// CycloneDX method spelling of that version — or `None` when the vector states
/// no version this recognises.
fn cvss_notation(vector: &str) -> Option<(&'static str, &'static str)> {
    let stated = vector.strip_prefix(CVSS_VECTOR_PREFIX)?;
    CVSS_NOTATIONS.iter().copied().find(|(version, _)| {
        stated
            .strip_prefix(version)
            .is_some_and(|rest| rest.starts_with(CVSS_VECTOR_SEPARATOR))
    })
}

/// The CycloneDX `vulnerability.ratings[].method` for a CVSS vector, read from
/// the vector's own version prefix. The returned values are the method values
/// the CycloneDX 1.5 schema enumerates; a vector in a notation this does not
/// recognise is reported as `other` rather than mislabelled.
fn cvss_method(vector: &str) -> &'static str {
    cvss_notation(vector).map_or("other", |(_, method)| method)
}

/// The CVSS specification version a vector is written in, or `None` when the
/// vector names none.
fn cvss_version(vector: &str) -> Option<&'static str> {
    cvss_notation(vector).map(|(version, _)| version)
}

/// The namespace Amazon Inspector uses for the CycloneDX `properties` it adds to
/// a scanned BOM.
const INSPECTOR_PROPERTY_NAMESPACE: &str = "amazon:inspector:sbom_scanner";

/// The rating source Amazon Inspector reports its own adjusted score under.
const INSPECTOR_RATING_SOURCE: &str = "Amazon Inspector";

/// The scoring vendor an exploit-probability rating is reported under: the
/// Exploit Prediction Scoring System, whose score is a probability in `0.0..=1.0`
/// rather than a 0-10 severity.
const EPSS_RATING_SOURCE: &str = "EPSS";

/// The CycloneDX `ratings[].method` an EPSS rating is reported under.
///
/// The CycloneDX 1.5 schema enumerates the scoring methods it names
/// (`CVSSv2`, `CVSSv3`, `CVSSv31`, `CVSSv4`, `OWASP`, `SSVC`, `other`); EPSS is
/// not among them, so the schema's own catch-all is what the method is, rather
/// than a value the schema does not allow there.
const EPSS_RATING_METHOD: &str = "other";

/// The `name` the scanning service reports itself under in the CycloneDX
/// report's `metadata.tools.services`.
const SCANNER_SERVICE_NAME: &str = "Amazon Inspector Scan SBOM API";

/// WHY a scan drew no conclusion about a component: the skip condition that
/// fired for it, which is what the report tells the caller so it can act on the
/// difference.
///
/// The two conditions are not interchangeable. A purl that names no package the
/// scanner looks up is a permanent gap — nothing in that ecosystem, or under
/// that half-stated coordinate, can ever be ruled on, so the caller must state
/// the coordinate differently or scan it elsewhere — while a package the
/// advisory data holds no rule for is in scope and simply matched nothing. The
/// condition is decided per component, from the component's own resolved
/// coordinate, so one scan reports both where a document carries both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    /// The component's purl does not name a package the scanner can look up:
    /// its ecosystem is outside [`SCANNED_ECOSYSTEMS`], or the coordinate
    /// carries no `namespace` where that ecosystem's packages are named with
    /// one ([`Namespace::Required`]).
    UnsupportedPurlNamespace,
    /// The purl names a package the scanner scans for, but the advisory data
    /// holds no rule for this package coordinate.
    NoSupportedRules,
}

impl SkipReason {
    /// The sentence this condition is reported as — fixed text, with no part
    /// taken from the component: the entry's `purl` is what names the component
    /// the sentence is about.
    ///
    /// One source for both renderings of one decision: the Inspector report's
    /// `messages[].info_message` and the component's CycloneDX
    /// `…:info` property, which therefore cannot disagree.
    fn info_message(self) -> &'static str {
        match self {
            Self::UnsupportedPurlNamespace => "Component skipped: unsupported purl namespace.",
            Self::NoSupportedRules => "Component skipped: no supported rules found.",
        }
    }
}

/// Why the scan drew no conclusion about `component`, or `None` when it was
/// ruled on (the advisory data holds rules for its package, whether or not this
/// release fell inside an affected range).
fn skip_reason(
    store: &crate::state::ComAmazonawsInspectorscanInspectorscanState,
    component: &ScannedComponent,
) -> Option<SkipReason> {
    // Whether the purl names a package the scanner looks up AT ALL is settled
    // first, and against the scanner's own coverage rather than against what the
    // advisory data happens to hold: a coordinate the scanner cannot resolve
    // holds no rules for exactly that reason, and reporting it as merely
    // rule-less would tell the caller to look for a rule that can never exist.
    // A component carrying no `purl` names no coordinate at all — there is no
    // ecosystem or namespace to judge — so it falls through to the lookup below.
    if component.purl.is_some() && !resolves_to_a_scanned_package(component) {
        return Some(SkipReason::UnsupportedPurlNamespace);
    }
    if !store.has_supported_rules(
        &component.package_type,
        component.distribution(),
        &component.package_name,
    ) {
        return Some(SkipReason::NoSupportedRules);
    }
    None
}

/// Does `component`'s purl name a package in an ecosystem this scanner scans?
///
/// Two conditions, both about the coordinate rather than about the advisory
/// data: the ecosystem is one of [`SCANNED_ECOSYSTEMS`], and the coordinate
/// carries the `namespace` that ecosystem names its packages with (see
/// [`Namespace`]).
fn resolves_to_a_scanned_package(component: &ScannedComponent) -> bool {
    match scanned_ecosystem(&component.package_type) {
        None => false,
        Some((_, Namespace::Required)) => component.purl_namespace.is_some(),
        Some((_, Namespace::Optional)) => true,
    }
}

/// The prefix of the document-local `bom-ref` a scan assigns every component of
/// the report (`comp-1`, `comp-2`, …).
const COMPONENT_REF_PREFIX: &str = "comp-";

/// The CWE a malicious-package detection is classified under: CWE-506,
/// "Embedded Malicious Code". A matched advisory carrying it reports a package
/// that is itself malicious rather than a defect in an otherwise legitimate
/// release, and a scan summarises the two separately.
const CWE_EMBEDDED_MALICIOUS_CODE: u32 = 506;

// ---------------------------------------------------------------------------
// Modeled failures
// ---------------------------------------------------------------------------

/// The fixed text an input-SBOM refusal's `message` opens with. What follows it
/// is the condition that failed, one clause per [`SbomDefect`].
const SBOM_VALIDATION_PREFIX: &str = "Input SBOM failed validation: ";

/// What an input SBOM was refused FOR: the class of defect, never an instance
/// of it.
///
/// The refusal text is a statement about the class alone — no index, no member
/// path and no value read back out of the submitted document — so two documents
/// defective the same way are refused with the same sentence.
#[derive(Debug, Clone, Copy)]
enum SbomDefect<'m> {
    /// `sbom` is not a document a scan can read at all: not a JSON object, or
    /// JSON this operation cannot take in (see [`MAX_SBOM_DEPTH`]).
    CannotParse,
    /// The document READS as a BOM but states, at member `m`, a BOM format or
    /// revision this operation does not scan — `bomFormat` naming a format
    /// other than CycloneDX, or `specVersion` naming another CycloneDX
    /// revision. `m` is the member's name as CycloneDX spells it.
    UnsupportedFormat(&'m str),
    /// The CycloneDX member `m` is absent where the specification requires it,
    /// or carries a value the specification does not allow there. `m` is the
    /// member's name as CycloneDX spells it (`bomFormat`, `components`,
    /// `bom-ref`, …), not a path into the submitted document.
    InvalidValue(&'m str),
    /// The document carries more components than one scan accepts; the limit is
    /// the clause's only variable part.
    TooManyComponents(usize),
}

impl SbomDefect<'_> {
    /// The condition that failed: the clause [`SBOM_VALIDATION_PREFIX`] is
    /// followed by in the refusal `message`, which is the whole of what the
    /// refusal states ([`refused`]).
    fn condition(self) -> String {
        match self {
            Self::CannotParse => "Invalid json".to_string(),
            Self::UnsupportedFormat(member) | Self::InvalidValue(member) => {
                format!("Invalid value for \"{member}\"")
            }
            Self::TooManyComponents(limit) => {
                format!("Max number of components exceeded ({limit})")
            }
        }
    }

    /// The refusal `message` for this class of defect.
    fn message(self) -> String {
        format!("{SBOM_VALIDATION_PREFIX}{}", self.condition())
    }

}

/// Refuse the submitted SBOM with `ValidationException` (HTTP 400), the status
/// and error code the model gives that failure.
fn invalid(defect: SbomDefect<'_>) -> ScanSbomError {
    refused(defect.message())
}

/// A `ValidationException` (HTTP 400) carrying `message` and nothing else.
///
/// What a refusal of this operation STATES is the sentence the caller fixes the
/// request from: Amazon Inspector Scan answers a `ScanSbom` it cannot scan with
/// that sentence alone, with no classification member and no per-member list
/// beside it, so a body that carried either would tell a client this service
/// reports something it does not. The model marks `ValidationException$reason`
/// required, which the modeled shape cannot leave out, so the refusal is
/// rendered through the error channel that carries the message alone
/// ([`crate::scaffolded::Scaffolded::validation`]) — the status (400) and the
/// wire error code (`ValidationException`) are the model's either way.
fn refused(message: impl Into<String>) -> ScanSbomError {
    crate::scaffolded::Scaffolded::validation(message)
}

/// The operation's own modeled HTTP path (`POST /scan/sbom`,
/// `smithy_query(query="operation", operation="ScanSbom")`), which is what a
/// call on it ADDRESSES: this service models no resource shape and mints no
/// resource ARN, so the resource an authorization decision is taken against is
/// this path under the service's namespace.
const SCAN_SBOM_PATH: &str = "/scan/sbom";

/// A modeled `AccessDeniedException` (HTTP 403), carrying the sentence IAM
/// refuses an unauthorized request with.
///
/// The modeled shape declares `message` alone, so the refusal carries nothing
/// else — in particular no part of the report the call asked for.
fn denied(message: String) -> ScanSbomError {
    ScanSbomErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// A modeled `InternalServerException` (HTTP 500).
fn internal(message: impl Into<String>) -> ScanSbomError {
    ScanSbomErrorModeled::InternalServerException(InternalServerException {
        message: message.into(),
        // From the model's `InternalServerExceptionReason` enum
        // (`super::types::INTERNAL_SERVER_EXCEPTION_REASON_VALUES`).
        reason: "OTHER".to_string(),
        retry_after_seconds: None,
    })
    .into()
}

// ---------------------------------------------------------------------------
// `aws_smithy_types::Document` <-> `serde_json::Value`
// ---------------------------------------------------------------------------

/// The modeled `Sbom` document as a `serde_json::Value`.
///
/// `Sbom` is a Smithy `document`, i.e. arbitrary JSON, and the whole operation is
/// JSON document surgery, so it is converted once on the way in and once on the
/// way out rather than walked as a `Document` tree.
///
/// `None` once `depth` passes [`MAX_SBOM_DEPTH`]: the input is caller-chosen, so
/// the recursion is bounded and the handler refuses a deeper document instead of
/// walking it.
///
/// `None` as well for a non-finite number, which JSON cannot express at all: the
/// document is REFUSED rather than silently rewritten to `null`, so the report
/// never restates a member the caller did not send, and two documents differing
/// only at such a member cannot collapse onto one digest below.
fn document_to_json(doc: &Document, depth: usize) -> Option<Value> {
    if depth > MAX_SBOM_DEPTH {
        return None;
    }
    Some(match doc {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::String(s) => Value::String(s.clone()),
        Document::Number(n) => match *n {
            Number::PosInt(v) => Value::Number(v.into()),
            Number::NegInt(v) => Value::Number(v.into()),
            Number::Float(v) => Value::Number(serde_json::Number::from_f64(v)?),
        },
        Document::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| document_to_json(item, depth + 1))
                .collect::<Option<Vec<Value>>>()?,
        ),
        Document::Object(members) => Value::Object(
            members
                .iter()
                .map(|(k, v)| document_to_json(v, depth + 1).map(|v| (k.clone(), v)))
                .collect::<Option<Map<String, Value>>>()?,
        ),
    })
}

/// The report as a modeled `Sbom` document.
///
/// Bounded for the same reason as [`document_to_json`], at the input limit plus
/// [`REPORT_DEPTH_HEADROOM`]: the report is the (already bounded) submitted
/// document plus a fixed-depth `vulnerabilities[]` subtree, so `None` here is
/// unreachable in practice and is answered as a server error rather than a
/// silently truncated document.
fn json_to_document(value: &Value, depth: usize) -> Option<Document> {
    if depth > MAX_SBOM_DEPTH + REPORT_DEPTH_HEADROOM {
        return None;
    }
    Some(match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::String(s) => Document::String(s.clone()),
        Value::Number(n) => {
            if let Some(v) = n.as_u64() {
                Document::Number(Number::PosInt(v))
            } else if let Some(v) = n.as_i64() {
                Document::Number(Number::NegInt(v))
            } else if let Some(v) = n.as_f64() {
                Document::Number(Number::Float(v))
            } else {
                Document::Null
            }
        }
        Value::Array(items) => Document::Array(
            items
                .iter()
                .map(|item| json_to_document(item, depth + 1))
                .collect::<Option<Vec<Document>>>()?,
        ),
        Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(k, v)| json_to_document(v, depth + 1).map(|v| (k.clone(), v)))
                .collect::<Option<HashMap<String, Document>>>()?,
        ),
    })
}

// ---------------------------------------------------------------------------
// The submitted document
// ---------------------------------------------------------------------------

/// One `components[]` entry resolved to the coordinate the advisory index is
/// keyed by.
#[derive(Debug, Clone)]
struct ScannedComponent {
    /// The document-local `bom-ref` this scan assigns the entry, which is the
    /// ref the report carries for it and the one every `affects[].ref` naming
    /// it resolves to.
    bom_ref: String,
    /// The purl ecosystem (`npm`, `maven`, …), empty when the entry carries no
    /// `purl` and so names no ecosystem.
    package_type: String,
    /// The ecosystem-canonical package name: `group:artifact` for maven, the
    /// full module path for golang, the plain name elsewhere.
    package_name: String,
    /// The resolved version — the purl's when it carries one, else the
    /// component's `version`.
    version: String,
    /// The submitted `purl`, when present.
    purl: Option<String>,
    /// The `namespace` of the submitted purl, `None` when the coordinate
    /// carries none. Kept beside the resolved name because whether the
    /// coordinate names a package at all depends on it (see [`Namespace`]),
    /// while the name itself may not include it (an OS package's namespace is
    /// the distribution, not part of the package name).
    purl_namespace: Option<String>,
}

impl ScannedComponent {
    /// The DISTRIBUTION part of this component's coordinate: the purl
    /// `namespace` where the ecosystem scopes its packages by one
    /// ([`namespace_is_distribution`]), and `None` everywhere else — either
    /// because the ecosystem does not scope its packages that way (there the
    /// namespace is already part of [`Self::package_name`]) or because the
    /// coordinate carries no namespace at all.
    ///
    /// Derived from the resolved coordinate rather than stored beside it, so the
    /// part of the namespace the NAME leaves out and the part the LOOKUP keys on
    /// cannot fall out of step.
    fn distribution(&self) -> Option<&str> {
        if namespace_is_distribution(&self.package_type) {
            self.purl_namespace.as_deref()
        } else {
            None
        }
    }
}

/// One `pkg:` coordinate, decomposed into the parts the purl specification
/// names.
///
/// Only the four parts a scan matches on are kept: the qualifiers and the
/// subpath are positional syntax this operation does not look up by, so they
/// are split off and dropped rather than retained.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPurl {
    /// The purl `type`, lowercased — the specification states the type is
    /// case-insensitive and canonically lowercase.
    ty: String,
    /// The `namespace`, percent-decoded, `None` when the coordinate carries
    /// none. Several path segments stay joined by `/`, as the specification
    /// writes them.
    namespace: Option<String>,
    /// The `name`, percent-decoded.
    name: String,
    /// The `version`, percent-decoded, `None` when the coordinate carries none.
    version: Option<String>,
}

/// Decode the `%XX` escapes of one purl component.
///
/// purl percent-encodes the segments of `namespace`, `name` and `version`, so a
/// coordinate's parts are decoded before they are matched or reported.
///
/// The escape handling is [`smithy_server_runtime::percent_decode`], this
/// crate's existing dependency, rather than a second copy of it. That helper
/// also folds `+` to a space, which is the FORM-encoding rule and not purl's:
/// `+` is a literal in a package version (`1.0+dfsg-1`) and in a type (`c++`).
/// Because a `+` can never occur inside an escape, splitting the component on
/// it, decoding each piece and rejoining with `+` keeps the literal while
/// leaving every escape to the helper.
fn percent_decode(component: &str) -> String {
    if !component.contains('%') && !component.contains('+') {
        return component.to_string();
    }
    component
        .split('+')
        .map(smithy_server_runtime::percent_decode)
        .collect::<Vec<String>>()
        .join("+")
}

/// Is `ty` a purl `type` at all?
///
/// The purl specification constrains the type to ASCII letters, digits, `.`,
/// `+` and `-`, and requires it to start with a letter. A string outside that
/// grammar names no ecosystem this scan can resolve.
fn is_purl_type(ty: &str) -> bool {
    ty.starts_with(|c: char| c.is_ascii_alphabetic())
        && ty
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// Decompose a `pkg:` coordinate, or say what is wrong with it.
///
/// The specification states the layout as
/// `pkg:<type>/<namespace...>/<name>@<version>?<qualifiers>#<subpath>`, and
/// decomposing it is the order the specification itself gives: the scheme is the
/// lowercase literal `pkg` (the slashes some generators write after it are not
/// part of the path), the subpath is what follows the first `#`, the qualifiers
/// what follow the first `?`, the version what follows the LAST `@` of the
/// remainder, and the rest is a path read right-to-left as name, then namespace,
/// with its first segment the type.
///
/// `None` when `raw` is not a package URL at all: the caller refuses the whole
/// document for the CLASS of defect (a `purl` member it cannot read), so there
/// is no per-defect sentence for this to carry.
fn parse_purl(raw: &str) -> Option<ParsedPurl> {
    // The specification fixes the scheme to the lowercase literal `pkg`.
    let rest = raw.strip_prefix("pkg:")?;
    let rest = rest.trim_start_matches('/');

    // Subpath, then qualifiers: both are suffixes of the coordinate, and
    // neither takes part in the lookup.
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let rest = rest.split_once('?').map_or(rest, |(before, _)| before);

    // The version is what follows the LAST `@`: a namespace may contain none,
    // but a version contains no `@` at all.
    let (path, version) = match rest.rsplit_once('@') {
        Some((path, version)) if !version.is_empty() => (path, Some(version)),
        Some((path, _)) => (path, None),
        None => (rest, None),
    };

    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    // A package URL names a type after the scheme, and the specification
    // constrains that type to its own grammar.
    let ty = segments.next()?;
    if !is_purl_type(ty) {
        return None;
    }
    let mut remaining: Vec<&str> = segments.collect();
    // A package URL names a package, as the last segment of its path.
    let name = percent_decode(remaining.pop()?);
    if name.is_empty() {
        return None;
    }

    let namespace = if remaining.is_empty() {
        None
    } else {
        let decoded: Vec<String> = remaining
            .iter()
            .map(|segment| percent_decode(segment))
            .collect();
        Some(decoded.join("/"))
    };

    let version = version.map(percent_decode);

    Some(ParsedPurl {
        ty: ty.to_ascii_lowercase(),
        namespace,
        name,
        version,
    })
}

/// The canonical package name for a purl's `namespace` + `name`.
///
/// purl specifies the namespace per type: for `maven` it is the groupId (joined
/// to the artifactId by `:`, the maven coordinate convention), for `npm` a scope
/// and for `golang` the module path prefix (both joined by `/`, the path
/// convention of each), and for the OS types (`deb`, `rpm`, `apk`) it is the
/// distribution VENDOR — `debian`, `redhat`, `alpine` — which is not part of the
/// package's name at all.
///
/// A namespace left out of the NAME is not left out of the COORDINATE: it is the
/// component's distribution ([`ScannedComponent::distribution`]), which the
/// advisory lookup is keyed by beside the type and the name.
fn canonical_package_name(package_type: &str, namespace: Option<&str>, name: &str) -> String {
    let lowered = package_type.to_ascii_lowercase();
    match namespace {
        None => name.to_string(),
        // The vendor namespace names the distribution, not the package.
        Some(_) if namespace_is_distribution(&lowered) => name.to_string(),
        Some(ns) if lowered == "maven" => format!("{ns}:{name}"),
        Some(ns) => format!("{ns}/{name}"),
    }
}

/// The component object of a `components[]` entry, or a refusal: an entry that
/// is not an object is not a CycloneDX component.
fn component_object(entry: &Value) -> Result<&Map<String, Value>, ScanSbomError> {
    entry
        .as_object()
        .ok_or_else(|| invalid(SbomDefect::InvalidValue("components")))
}

/// FIRST PASS over the component tree: count every entry and collect every
/// submitted `bom-ref`.
///
/// CycloneDX requires `bom-ref` to be unique within a document, so a repeat is
/// refused — left alone a submitted document could name two different
/// components by one ref, which is ambiguous however the report re-identifies
/// them.
///
/// The documented [`MAX_COMPONENTS`] limit is enforced HERE, as the count passes
/// it, rather than on the total afterwards: the refusal then costs what the
/// limit allows instead of what the caller submitted, so a document declaring
/// far more components than a scan accepts is not walked (and its refs not
/// retained) before it is refused.
fn collect_component_refs(
    entries: &[Value],
    depth: usize,
    submitted_refs: &mut BTreeSet<String>,
    counted: &mut usize,
) -> Result<(), ScanSbomError> {
    if depth > MAX_COMPONENT_NESTING {
        return Err(invalid(SbomDefect::InvalidValue("components")));
    }
    for entry in entries {
        let object = component_object(entry)?;
        *counted += 1;
        if *counted > MAX_COMPONENTS {
            return Err(invalid(SbomDefect::TooManyComponents(MAX_COMPONENTS)));
        }
        if let Some(submitted) = submitted_bom_ref(object)? {
            if !submitted_refs.insert(submitted.to_string()) {
                // CycloneDX requires `bom-ref` to be unique within a document,
                // so a repeat is not a value that member can hold.
                return Err(invalid(SbomDefect::InvalidValue("bom-ref")));
            }
        }
        if let Some(children) = nested_components(object)? {
            collect_component_refs(children, depth + 1, submitted_refs, counted)?;
        }
    }
    Ok(())
}

/// The nested `components` array of an assembly entry, `None` when it carries
/// none, or a refusal when the member is present and is not an array.
///
/// The same rule the top level applies: a `components` member that is not an
/// array is not a CycloneDX component list, and left to fall through it would
/// drop the whole subtree silently — neither counted against the documented
/// limit, nor matched, nor reported.
fn nested_components(
    object: &Map<String, Value>,
) -> Result<Option<&Vec<Value>>, ScanSbomError> {
    match object.get("components") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(children)) => Ok(Some(children)),
        Some(_) => Err(invalid(SbomDefect::InvalidValue("components"))),
    }
}

/// The `properties` array of a submitted object — a `components[]` entry's or
/// the document's `metadata` — `None` when it carries none, or a refusal when
/// the member is present and is not an array.
///
/// CycloneDX declares `properties` an array of name/value pairs at BOTH sites,
/// and the report APPENDS the scan's own entries to it ([`annotate_skipped_components`]
/// and the summary in [`cyclonedx_report`]), so one rule reads it for both: a
/// value of another type cannot carry the scan's entries beside what the caller
/// submitted, and left to fall through it would be REPLACED by an array holding
/// the scan's own entries alone — a 200 response stating the caller sent
/// something it did not.
fn submitted_properties(
    object: &Map<String, Value>,
) -> Result<Option<&Vec<Value>>, ScanSbomError> {
    match object.get("properties") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(properties)) => Ok(Some(properties)),
        Some(_) => Err(invalid(SbomDefect::InvalidValue("properties"))),
    }
}

/// The submitted `metadata`, `None` when the document carries none, or a
/// refusal when the member is present and is not an object.
///
/// The same rule [`nested_components`] applies, at the member CycloneDX
/// declares an object: the report is the submitted document with the scan's own
/// data added to it, so a `metadata` of another type left to fall through would
/// be REPLACED by the object the scan builds.
fn submitted_metadata(
    document: &Map<String, Value>,
) -> Result<Option<&Map<String, Value>>, ScanSbomError> {
    match document.get("metadata") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(metadata)) => Ok(Some(metadata)),
        Some(_) => Err(invalid(SbomDefect::InvalidValue("metadata"))),
    }
}

/// What CycloneDX allows a submitted `metadata.tools` to be: the 1.5 object
/// form, or the array of tool objects 1.5 deprecated but still reads.
#[derive(Debug, Clone, Copy)]
enum SubmittedTools<'d> {
    /// The document says nothing about the tools that produced it.
    Absent,
    /// The 1.5 object form, carrying `components` and `services`.
    Object(&'d Map<String, Value>),
    /// The deprecated 1.4 array of `{vendor, name, version}` tool objects.
    Entries(&'d [Value]),
}

/// Which form the submitted `metadata.tools` is in, or a refusal when the
/// member is present in neither.
///
/// An ENTRY of the array form is a tool object; one of another type names no
/// tool, and [`tool_component`] reads a member off it with none to read, so it
/// would be reported as a tool component carrying nothing the caller sent.
fn submitted_tools(metadata: &Map<String, Value>) -> Result<SubmittedTools<'_>, ScanSbomError> {
    match metadata.get("tools") {
        None | Some(Value::Null) => Ok(SubmittedTools::Absent),
        Some(Value::Object(tools)) => Ok(SubmittedTools::Object(tools)),
        Some(Value::Array(entries)) if entries.iter().all(Value::is_object) => {
            Ok(SubmittedTools::Entries(entries))
        }
        Some(_) => Err(invalid(SbomDefect::InvalidValue("tools"))),
    }
}

/// The `services` array of a submitted 1.5 `metadata.tools`, `None` when it
/// names none, or a refusal when the member is present and is not an array.
///
/// The scan appends ITSELF to this array, so the same rule as
/// [`submitted_properties`] holds for it.
fn submitted_services(tools: &Map<String, Value>) -> Result<Option<&Vec<Value>>, ScanSbomError> {
    match tools.get("services") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(services)) => Ok(Some(services)),
        Some(_) => Err(invalid(SbomDefect::InvalidValue("services"))),
    }
}

/// The string a submitted member carries, `None` when the object states none,
/// or a refusal when the member is present and is not a string.
///
/// CycloneDX types every member this scan reads a VALUE off a string —
/// `bomFormat`, `specVersion` and `serialNumber` on the document, and `type`,
/// `name`, `version`, `purl` and `bom-ref` on a component — so a value of
/// another JSON type is not a value that member can hold, and the document is
/// refused for the member it is, exactly as a mistyped `components` already is
/// ([`nested_components`]). Read as ABSENT instead, such a member would decide
/// the scan on a coordinate the caller never submitted: an unreadable `purl`
/// would resolve the component under no ecosystem and an unreadable `version`
/// under no release, so a package the advisory data does hold a rule for would
/// be answered 200 — reported clean, or annotated as having no supported rules —
/// and an unreadable `bom-ref` would escape the uniqueness rule the
/// specification states for it.
///
/// A member present as `null` states no value, which is read as absent exactly
/// as an absent member is.
fn submitted_string<'o>(
    object: &'o Map<String, Value>,
    member: &'static str,
) -> Result<Option<&'o str>, ScanSbomError> {
    match object.get(member) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(invalid(SbomDefect::InvalidValue(member))),
    }
}

/// The submitted `serialNumber`, `None` when the document carries none, or a
/// refusal when the member is present and is not a string.
///
/// CycloneDX declares `serialNumber` a string identifying the BOM instance, and
/// the report stamps its own only on a document that names none, so a value of
/// another type left to fall through would be replaced by the scan's URN.
fn submitted_serial_number(document: &Map<String, Value>) -> Result<Option<&str>, ScanSbomError> {
    submitted_string(document, "serialNumber")
}

/// The submitted document-level `version` — the BOM's own revision number —
/// `None` when the document states none, or a refusal when the one it carries
/// is not an integer.
///
/// CycloneDX types the document's `version` an integer, so a value of another
/// JSON type is not a value that member can hold and the document is refused
/// for the member it is, exactly as the `serialNumber` beside it already is
/// ([`submitted_serial_number`]). Left to fall through it would be ECHOED into
/// a report the response declares to be a CycloneDX 1.5 document, stating a
/// revision number no consumer of that document can read.
///
/// A number WRITTEN as a floating-point literal is refused with the rest: the
/// report preserves what the caller sent rather than rewriting it, so a value
/// the report cannot carry as the integer the schema declares is not one this
/// member can hold either — `1.0` and `1e3` are refused beside `1.5`.
///
/// A member present as `null` states no value, which is read as absent exactly
/// as an absent member is — so the report stamps the schema default over it
/// rather than carrying a revision number the caller never asked for.
fn submitted_bom_version(
    document: &Map<String, Value>,
) -> Result<Option<&serde_json::Number>, ScanSbomError> {
    match document.get("version") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) if number.is_i64() || number.is_u64() => Ok(Some(number)),
        Some(_) => Err(invalid(SbomDefect::InvalidValue("version"))),
    }
}

/// Refuse a submitted document that carries one of the members the report is
/// built from with a JSON type CycloneDX does not allow there.
///
/// Every member is read through the accessor [`cyclonedx_report`] reads it
/// with, so ONE rule decides both what is refused and what a report carries,
/// and the check runs for every `outputFormat`: whether a submitted document is
/// a CycloneDX 1.5 BOM is a property of the document, not of the form the
/// answer is rendered in. A member present as `null` states no value, which is
/// read as absent exactly as `components` already is.
fn validate_report_members(document: &Map<String, Value>) -> Result<(), ScanSbomError> {
    // Each accessor is called for its type check; the values themselves are
    // read again where the report is built.
    let _serial_number = submitted_serial_number(document)?;
    let _version = submitted_bom_version(document)?;
    let Some(metadata) = submitted_metadata(document)? else {
        return Ok(());
    };
    let _properties = submitted_properties(metadata)?;
    if let SubmittedTools::Object(tools) = submitted_tools(metadata)? {
        let _services = submitted_services(tools)?;
    }
    Ok(())
}

/// The `bom-ref` a component submitted, `None` when it carries none, or a
/// refusal when the one it carries is not a string ([`submitted_string`]) or is
/// longer than [`MAX_BOM_REF_LEN`].
///
/// The LENGTH bound is what keeps the ref a bounded cost: every submitted ref
/// is retained for the uniqueness check above, once per component of the
/// submitted tree, so an unbounded ref would be retained a bounded NUMBER of
/// times but at an unbounded SIZE. Bounding it here, at the one site that reads
/// it, is what makes neither the component count nor the nesting depth the only
/// limit on what one request can allocate.
fn submitted_bom_ref(object: &Map<String, Value>) -> Result<Option<&str>, ScanSbomError> {
    let Some(submitted) = submitted_string(object, "bom-ref")?.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    if submitted.chars().count() > MAX_BOM_REF_LEN {
        return Err(invalid(SbomDefect::InvalidValue("bom-ref")));
    }
    Ok(Some(submitted))
}

/// SECOND PASS over the component tree: resolve every entry to a scannable
/// coordinate, and return the entries with the `bom-ref` this scan assigns
/// stamped on each.
///
/// The scan IDENTIFIES the components of the report itself, by a sequential
/// document-local ref ([`COMPONENT_REF_PREFIX`] plus the entry's 1-based
/// position in the walk), rather than re-using whatever identifier the
/// submitted document happened to carry: the ref a report hands a consumer is
/// the scanner's, so a finding's `affects[].ref` names a component of THAT
/// document and does not depend on a submitted ref being present, unique or
/// usable as a key (generators commonly set it to the component's purl, which
/// repeats across two entries of one package at two versions).
///
/// CycloneDX 1.5 lets a component carry its own `components[]` (an assembly),
/// and the scanner resolves the whole tree, so a vulnerable package below the
/// first level is matched like any other. `out` is filled in document order,
/// parents before children, and `next_ref` counts across the whole tree so one
/// ref names exactly one component of the report.
///
/// `depth` is carried here as well as in [`collect_component_refs`], rather than
/// relying on that pass having already refused a deeper tree: the bound belongs
/// at every site that recurses over caller-chosen input, so reordering the two
/// passes cannot reintroduce an unbounded recursion.
fn resolve_component_tree(
    entries: &[Value],
    depth: usize,
    next_ref: &mut usize,
    out: &mut Vec<ScannedComponent>,
) -> Result<Vec<Value>, ScanSbomError> {
    if depth > MAX_COMPONENT_NESTING {
        return Err(invalid(SbomDefect::InvalidValue("components")));
    }
    let mut normalized: Vec<Value> = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = component_object(entry)?;

        let name = submitted_string(object, "name")?.unwrap_or_default();
        let declared_version = submitted_string(object, "version")?.unwrap_or_default();
        let purl = submitted_string(object, "purl")?;

        // CycloneDX 1.5 makes `type` and `name` REQUIRED on a component, and
        // constrains `type` to its `classification` enumeration, so a document
        // carrying neither is not a CycloneDX 1.5 BOM and is refused here rather
        // than scanned. `version` is NOT required (it stopped being so after
        // 1.3), so an entry without one is resolved and simply matches no
        // advisory — see below.
        match submitted_string(object, "type")? {
            Some(declared) if CYCLONEDX_COMPONENT_TYPES.contains(&declared) => {}
            // Absent, or not one of the specification's component types.
            _ => return Err(invalid(SbomDefect::InvalidValue("type"))),
        }
        if name.is_empty() {
            return Err(invalid(SbomDefect::InvalidValue("name")));
        }
        // The array the report appends the scanner's annotation to when this
        // component is one the scan draws no conclusion about: its type is
        // checked for every entry, so whether a document is accepted does not
        // depend on which of its components the advisory data happens to rule
        // on.
        let _properties = submitted_properties(object)?;

        let (package_type, package_name, version, purl_namespace) = match purl {
            Some(raw) => {
                let parsed =
                    parse_purl(raw).ok_or_else(|| invalid(SbomDefect::InvalidValue("purl")))?;
                let version = parsed
                    .version
                    .as_deref()
                    .filter(|v| !v.is_empty())
                    .unwrap_or(declared_version)
                    .to_string();
                (
                    parsed.ty.clone(),
                    canonical_package_name(&parsed.ty, parsed.namespace.as_deref(), &parsed.name),
                    version,
                    parsed.namespace.clone(),
                )
            }
            // No purl member at all (or one present as `null`): the entry names
            // no ecosystem, so it is scanned under its own name and matches only
            // an advisory recorded without one.
            None => (
                String::new(),
                name.to_string(),
                declared_version.to_string(),
                None,
            ),
        };

        // A coordinate with no version matches nothing: `version_in_range`
        // reduces the version first, and reduces an absent one to nothing to
        // compare, so an unversioned component is reported clean instead of
        // failing the scan. Only a component that resolves no NAME is refused,
        // because without one there is nothing to look up at all.
        if package_name.is_empty() {
            return Err(invalid(SbomDefect::InvalidValue("name")));
        }

        *next_ref += 1;
        let bom_ref = format!("{COMPONENT_REF_PREFIX}{next_ref}");

        out.push(ScannedComponent {
            bom_ref: bom_ref.clone(),
            package_type,
            package_name,
            version,
            purl: purl.map(str::to_string),
            purl_namespace,
        });

        // The stamped entry, with its children resolved the same way, so the
        // ref an `affects[].ref` points at appears on its component in the
        // report.
        let mut stamped = object.clone();
        stamped.insert("bom-ref".to_string(), Value::String(bom_ref.clone()));
        if let Some(children) = nested_components(object)? {
            let resolved = resolve_component_tree(children, depth + 1, next_ref, out)?;
            stamped.insert("components".to_string(), Value::Array(resolved));
        }
        normalized.push(Value::Object(stamped));
    }
    Ok(normalized)
}

/// The submitted document, resolved: every component of the tree as a scannable
/// coordinate, plus the `components` array with a `bom-ref` stamped on each
/// entry.
#[derive(Debug, Default)]
struct ParsedSbom {
    /// The normalized `components` array, absent when the document carries none.
    components_value: Option<Value>,
    /// Every component of the tree, in document order, parents before children.
    components: Vec<ScannedComponent>,
}

/// Refuse anything that is not a CycloneDX 1.5 BOM, and resolve its components.
fn parse_sbom(sbom: &Value) -> Result<ParsedSbom, ScanSbomError> {
    let object = sbom
        .as_object()
        .ok_or_else(|| invalid(SbomDefect::CannotParse))?;

    match submitted_string(object, "bomFormat")? {
        Some(CYCLONEDX_BOM_FORMAT) => {}
        // Naming a format that is not CycloneDX at all: a BOM type no scan
        // reads, rather than a member carrying something it cannot hold.
        Some(_) => return Err(invalid(SbomDefect::UnsupportedFormat("bomFormat"))),
        // Absent: the member the specification requires states no format.
        None => return Err(invalid(SbomDefect::InvalidValue("bomFormat"))),
    }

    match submitted_string(object, "specVersion")? {
        Some(CYCLONEDX_SPEC_VERSION) => {}
        // A CycloneDX revision other than the one a scan reads: again a BOM
        // type this operation does not scan.
        Some(_) => return Err(invalid(SbomDefect::UnsupportedFormat("specVersion"))),
        // Absent: the member the specification requires states no revision.
        None => return Err(invalid(SbomDefect::InvalidValue("specVersion"))),
    }

    // The document's own members the report is built from, before anything is
    // scanned: a member CycloneDX does not allow this type at is refused here
    // rather than rewritten when the report is rendered.
    validate_report_members(object)?;

    let components = match object.get("components") {
        None | Some(Value::Null) => return Ok(ParsedSbom::default()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(invalid(SbomDefect::InvalidValue("components"))),
    };

    // Count the WHOLE tree against the documented limit, not just the top
    // level: a nested assembly's components are scanned, so they count. The
    // limit is enforced inside the walk, as the count passes it.
    let mut submitted_refs = BTreeSet::new();
    let mut total = 0usize;
    collect_component_refs(components, 0, &mut submitted_refs, &mut total)?;

    let mut resolved = Vec::with_capacity(total);
    let mut next_ref = 0usize;
    let normalized = resolve_component_tree(components, 0, &mut next_ref, &mut resolved)?;
    Ok(ParsedSbom {
        components_value: Some(Value::Array(normalized)),
        components: resolved,
    })
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

/// One advisory matched against one component of the submitted document.
#[derive(Debug, Clone)]
struct Finding {
    component: ScannedComponent,
    advisory: VulnRecord,
}

/// Write `value` into `out` in a canonical form: every object's members in
/// ascending key order, no insignificant whitespace.
///
/// The ordering is done HERE rather than left to `serde_json`'s own
/// serialization, which is only key-ordered while the crate is built without
/// its `preserve_order` feature — a feature any crate in the workspace graph
/// can turn on, since cargo features are additive. The document being hashed
/// came from `aws_smithy_types::Document::Object`, i.e. a `HashMap` whose
/// iteration order differs per instance, so under `preserve_order` an
/// insertion-ordered map would give the SAME document two different
/// serializations and break the digest below. Sorting at this site makes the
/// canonical form independent of how any map iterates.
///
/// `None` when a value cannot be rendered at all (a non-finite number), which
/// the caller answers rather than hashing a form that drops it, and once `depth`
/// passes the bound the submitted document was parsed under: the bound belongs at
/// every site that recurses over caller-chosen input, so a future caller hashing
/// a document [`document_to_json`] did not already bound cannot reintroduce an
/// unbounded recursion here.
fn write_canonical_json(value: &Value, out: &mut String, depth: usize) -> Option<()> {
    if depth > MAX_SBOM_DEPTH {
        return None;
    }
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&n.to_string()),
        // `to_string` of a `Value::String` is the JSON-escaped literal, so two
        // documents differing only inside a string stay distinguishable.
        Value::String(_) => out.push_str(&serde_json::to_string(value).ok()?),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out, depth + 1)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).ok()?);
                out.push(':');
                write_canonical_json(members.get(key)?, out, depth + 1)?;
            }
            out.push('}');
        }
    }
    Some(())
}

/// Hex sha-256 over the canonical JSON form of the submitted document.
///
/// Two requests carrying the same SBOM with different member order or
/// whitespace share a digest, because [`write_canonical_json`] renders both the
/// same way. That is what makes a repeat of this `@idempotent` operation
/// recognisable, and what keeps the derived `serialNumber` stable for one
/// document across runs and toolchains.
///
/// `None` when the document cannot be rendered canonically: a degenerate digest
/// would be shared by every such document, which would make two different
/// documents look like a repeat of one another, so the failure is reported
/// instead.
fn sbom_digest(sbom: &Value) -> Option<String> {
    let mut canonical = String::new();
    write_canonical_json(sbom, &mut canonical, 0)?;
    Some(hex::encode(sha2::Sha256::digest(canonical.as_bytes())))
}

/// A UUID URN for the report's CycloneDX `serialNumber`, derived from the
/// document digest.
///
/// Content-derived rather than random so that a repeat of this `@idempotent`
/// scan answers with the identical report. The version nibble is 8 (RFC 9562
/// "custom" UUID) and the variant nibble is `a` (`0b10`), so the value is a
/// well-formed UUID in the `urn:uuid:` form CycloneDX requires.
///
/// The 32 nibbles are grouped from the character vector itself rather than by
/// slicing the joined string, so the grouping cannot depend on each character
/// being one byte wide.
fn serial_number_from_digest(digest: &str) -> String {
    let mut nibbles: Vec<char> = digest.chars().take(32).collect();
    if nibbles.len() < 32 {
        nibbles.resize(32, '0');
    }
    nibbles[12] = '8';
    nibbles[16] = 'a';
    let group = |range: std::ops::Range<usize>| -> String { nibbles[range].iter().collect() };
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        group(0..8),
        group(8..12),
        group(12..16),
        group(16..20),
        group(20..32)
    )
}

/// `instant` as an RFC 3339 timestamp, the form CycloneDX `metadata.timestamp`
/// takes.
fn rfc3339(instant: SystemTime) -> String {
    let since_epoch = instant.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    aws_smithy_types::DateTime::from_secs_and_nanos(seconds, since_epoch.subsec_nanos())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The CycloneDX `ratings[].severity` spelling of a stored severity.
///
/// The CycloneDX 1.5 schema enumerates the severity values in lowercase
/// (`critical`, `high`, `medium`, `low`, `info`, `none`, `unknown`); the store
/// keeps Inspector's uppercase spelling, so this is the one place they are
/// mapped.
fn cyclonedx_severity(stored: &str) -> String {
    let lowered = stored.to_ascii_lowercase();
    match lowered.as_str() {
        "critical" | "high" | "medium" | "low" | "info" | "none" => lowered,
        "informational" => "info".to_string(),
        _ => "unknown".to_string(),
    }
}

/// The buckets a scan's findings are summarised over.
///
/// A scan summary reports a total per rated severity plus one `other` total,
/// which is what a finding rated outside the four named severities (or not
/// rated at all) is counted under — so every finding is counted exactly once,
/// however it is rated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeverityBucket {
    Critical,
    High,
    Medium,
    Low,
    Other,
}

impl SeverityBucket {
    /// The bucket a stored severity falls in. The store keeps Inspector's
    /// uppercase spelling; a report spells a severity in lowercase.
    fn of(stored: &str) -> Self {
        match stored.to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            _ => Self::Other,
        }
    }

    /// The bucket's reported spelling.
    fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Other => "other",
        }
    }
}

/// What a scan found, counted: how many findings fell in each severity bucket,
/// and how many of them report a malicious package.
///
/// Every total is a count over the matched [`Finding`] set, so a scan that
/// matched nothing summarises as zeros rather than as nothing: the report says
/// "scanned, nothing found", which is a different answer from "no counts
/// reported".
#[derive(Debug, Default, Clone, Copy)]
struct ScanSummary {
    critical: usize,
    high: usize,
    medium: usize,
    low: usize,
    other: usize,
    malicious_packages: usize,
}

impl ScanSummary {
    /// Summarise `findings`: each counts once in the bucket its reported
    /// severity falls in, and again under `malicious_packages` when the
    /// advisory it matched is a malicious-package detection
    /// ([`CWE_EMBEDDED_MALICIOUS_CODE`]).
    fn of(findings: &[Finding]) -> Self {
        let mut summary = Self::default();
        for finding in findings {
            let counter = match SeverityBucket::of(&finding.advisory.inspector_severity) {
                SeverityBucket::Critical => &mut summary.critical,
                SeverityBucket::High => &mut summary.high,
                SeverityBucket::Medium => &mut summary.medium,
                SeverityBucket::Low => &mut summary.low,
                SeverityBucket::Other => &mut summary.other,
            };
            *counter += 1;
            if finding
                .advisory
                .cwes
                .contains(&CWE_EMBEDDED_MALICIOUS_CODE)
            {
                summary.malicious_packages += 1;
            }
        }
        summary
    }

    /// The Inspector report's per-severity count object: one member per bucket,
    /// always present.
    fn vulnerability_count(&self) -> Value {
        json!({
            "critical": self.critical,
            "high": self.high,
            "medium": self.medium,
            "low": self.low,
            "other": self.other,
        })
    }

    /// The same summary as the CycloneDX report carries it: namespaced
    /// `metadata.properties` entries, each value the decimal count (CycloneDX
    /// types a `properties[].value` as a string).
    fn cyclonedx_properties(&self) -> Vec<Value> {
        [
            ("critical_vulnerabilities", self.critical),
            ("high_vulnerabilities", self.high),
            ("medium_vulnerabilities", self.medium),
            ("low_vulnerabilities", self.low),
            ("other_vulnerabilities", self.other),
            ("malicious_packages", self.malicious_packages),
        ]
        .into_iter()
        .map(|(name, count)| {
            json!({
                "name": format!("{INSPECTOR_PROPERTY_NAMESPACE}:{name}"),
                "value": count.to_string(),
            })
        })
        .collect()
    }
}

/// Whether a purl in one ecosystem NAMES its packages with a `namespace`.
///
/// purl defines the namespace per type, and in most ecosystems it is what
/// qualifies the name into a package: the maven groupId, the module path of a
/// Go module, a composer vendor, the distribution an OS package was built by.
/// A coordinate in such an ecosystem carrying no namespace identifies no
/// package at all — which is a different answer from the scanner holding no
/// rule for the package a coordinate does identify. Where the namespace is
/// optional (an npm scope) or the ecosystem has none (PyPI, RubyGems,
/// crates.io, NuGet), the `name` alone identifies the package.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Namespace {
    /// The coordinate names a package only together with its namespace.
    Required,
    /// The `name` alone names the package.
    Optional,
}

/// The purl ecosystems this scanner scans, each with the Amazon Inspector
/// `packageManager` it reports a finding in that ecosystem under and whether a
/// purl names a package in it with a `namespace`.
///
/// This is the scanner's COVERAGE, and it answers two questions at once: which
/// package-manager vocabulary a finding is reported in, and whether a component
/// could be ruled on at all (see [`SkipReason`]). Keeping it as one table is
/// what keeps those two answers from disagreeing — an ecosystem reported as
/// `OTHER` is precisely one the scan draws no conclusions in.
///
/// The purl `type` keys are the specification's own (lowercase) type names.
const SCANNED_ECOSYSTEMS: &[(&str, &str, Namespace)] = &[
    // An npm scope is optional, and PyPI, crates.io, RubyGems and NuGet have no
    // namespace at all: the name alone is the package.
    ("npm", "NPM", Namespace::Optional),
    ("pypi", "PIP", Namespace::Optional),
    ("cargo", "CARGO", Namespace::Optional),
    ("gem", "BUNDLER", Namespace::Optional),
    ("nuget", "NUGET", Namespace::Optional),
    // A maven artifact is named by its group, a Go module by its path and a
    // composer package by its vendor.
    ("maven", "POM", Namespace::Required),
    ("golang", "GOMOD", Namespace::Required),
    ("composer", "COMPOSER", Namespace::Required),
    // The OS package ecosystems are all reported under one package manager, and
    // each names the DISTRIBUTION its packages were built by: `debian`,
    // `redhat`, `alpine`. The same package name is a different package, with
    // different advisories, in each of them, so a coordinate naming none of
    // them identifies no OS package.
    ("deb", "OS", Namespace::Required),
    ("rpm", "OS", Namespace::Required),
    ("apk", "OS", Namespace::Required),
];

/// The Amazon Inspector `packageManager` the OS package ecosystems are reported
/// under, and so the entry of [`SCANNED_ECOSYSTEMS`] that says an ecosystem's
/// purl `namespace` names the DISTRIBUTION its packages were built by.
const OS_PACKAGE_MANAGER: &str = "OS";

/// Does a purl in this ecosystem name the DISTRIBUTION its packages were built
/// by in its `namespace`, rather than a part of the package's own name?
///
/// Read off [`SCANNED_ECOSYSTEMS`] — the OS package ecosystems are exactly the
/// ones reported under [`OS_PACKAGE_MANAGER`] — so the table that says WHICH
/// ecosystems those are is the one that says what their namespace means.
fn namespace_is_distribution(package_type: &str) -> bool {
    scanned_ecosystem(package_type)
        .is_some_and(|(package_manager, _)| package_manager == OS_PACKAGE_MANAGER)
}

/// The Inspector `packageManager` and namespace rule for a purl ecosystem this
/// scanner scans, and `None` for an ecosystem outside [`SCANNED_ECOSYSTEMS`].
///
/// The purl specification makes the `type` case-insensitive and canonically
/// lowercase, so the lookup folds it.
fn scanned_ecosystem(package_type: &str) -> Option<(&'static str, Namespace)> {
    let lowered = package_type.to_ascii_lowercase();
    SCANNED_ECOSYSTEMS
        .iter()
        .find(|(ty, _, _)| *ty == lowered)
        .map(|(_, package_manager, namespace)| (*package_manager, *namespace))
}

/// The Amazon Inspector `packageManager` name for a purl ecosystem.
///
/// Maps the purl `type` the SBOM states onto the package-manager vocabulary
/// Inspector reports findings under; an ecosystem outside that vocabulary is
/// reported as `OTHER` rather than invented.
fn inspector_package_manager(package_type: &str) -> &'static str {
    scanned_ecosystem(package_type).map_or("OTHER", |(package_manager, _)| package_manager)
}

/// The CycloneDX `vulnerability` object for one finding.
///
/// The node names the advisory's publisher once, with the page that publisher
/// published the advisory at beside it (`VulnRecord::source_url`) — the one link
/// a report carries. Everything else that could carry an address does not: a
/// RATING names only the scoring vendor that produced it, since the rating is
/// not a publication to dereference, and the node carries no `advisories`
/// member, because CycloneDX types each entry of that member as a reference URL
/// and this service publishes no reference pages.
fn cyclonedx_vulnerability(finding: &Finding) -> Value {
    let advisory = &finding.advisory;
    // Every rating is scored BY a vendor, so its source names that vendor and
    // nothing more.
    let rating_source = json!({ "name": advisory.source });
    let mut source = Map::new();
    source.insert("name".to_string(), json!(advisory.source));
    if let Some(page) = &advisory.source_url {
        source.insert("url".to_string(), Value::String(page.clone()));
    }
    let source = Value::Object(source);

    // The operation's documentation states the output "reports NVD and CVSS
    // scores when NVD and CVSS scores are available" and warns that the two can
    // disagree, so each is reported under the source that produced it — and a
    // rating whose vector the advisory does not carry is OMITTED rather than
    // claimed. `method` is read from the vector's own version prefix.
    let mut ratings: Vec<Value> = Vec::with_capacity(3);
    if !advisory.cvss_v3_vector.is_empty() {
        ratings.push(json!({
            // The advisory's own publisher is what scored it.
            "source": rating_source,
            "score": advisory.cvss_v3_score,
            "severity": cyclonedx_severity(&advisory.nvd_severity),
            "method": cvss_method(&advisory.cvss_v3_vector),
            "vector": advisory.cvss_v3_vector,
        }));
    }
    if !advisory.inspector_cvss_vector.is_empty() {
        ratings.push(json!({
            "source": { "name": INSPECTOR_RATING_SOURCE },
            "score": advisory.inspector_score,
            "severity": cyclonedx_severity(&advisory.inspector_severity),
            "method": cvss_method(&advisory.inspector_cvss_vector),
            "vector": advisory.inspector_cvss_vector,
        }));
    }
    // The exploit-probability rating, reported under its own scoring vendor
    // beside the severity ones: it answers a different question (how likely is
    // this to be exploited) on a different scale (a probability), so it is one
    // more entry of the per-vendor set and not a restatement of a CVSS score. An
    // advisory the scoring system has published no estimate for carries no such
    // entry rather than a claimed zero.
    if let Some(probability) = advisory.epss_score {
        ratings.push(json!({
            "source": { "name": EPSS_RATING_SOURCE },
            "score": probability,
            "method": EPSS_RATING_METHOD,
        }));
    }

    let mut properties = vec![json!({
        "name": format!("{INSPECTOR_PROPERTY_NAMESPACE}:package_type"),
        "value": inspector_package_manager(&finding.component.package_type),
    })];
    if let Some(fixed) = &advisory.fixed_in_version {
        properties.push(json!({
            "name": format!("{INSPECTOR_PROPERTY_NAMESPACE}:fixed_in_version"),
            "value": fixed,
        }));
    }

    json!({
        "bom-ref": format!("vuln:{}:{}", advisory.cve_id, finding.component.bom_ref),
        "id": advisory.cve_id,
        "source": source,
        "ratings": ratings,
        // CycloneDX 1.5 types `vulnerability.cwes` as an array of CWE integers.
        "cwes": advisory.cwes,
        "description": advisory.description,
        "published": advisory.published,
        "updated": advisory.modified,
        "affects": [ { "ref": finding.component.bom_ref } ],
        "properties": properties,
    })
}

/// The CycloneDX `metadata.tools.services` entry the scanning service reports
/// itself under.
///
/// `version` is the BUILD IDENTITY of the scanner that answered the request,
/// read from the service's own state ([`crate::state::ScannerBuild`]) — it
/// describes the scanner, so it is never taken from the packaging of whatever
/// is serving it.
fn scanner_service(scanner_version: &str) -> Value {
    json!({
        "name": SCANNER_SERVICE_NAME,
        "version": scanner_version,
    })
}

/// A deprecated 1.4-style `metadata.tools[]` entry as the tool COMPONENT the
/// 1.5 object form holds.
///
/// CycloneDX 1.5 replaces the array of `{vendor, name, version}` tool objects
/// with an object carrying `components` and `services`, and the scan reports
/// itself under the latter — so a submitted array is carried over into
/// `components` rather than dropped, mapping `vendor` onto the component
/// `publisher` the specification names.
fn tool_component(entry: &Value) -> Value {
    let mut component = Map::new();
    component.insert("type".to_string(), json!("application"));
    if let Some(vendor) = entry.get("vendor") {
        component.insert("publisher".to_string(), vendor.clone());
    }
    for member in ["name", "version", "hashes", "externalReferences"] {
        if let Some(value) = entry.get(member) {
            component.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(component)
}

/// Report, on every component of `components` the scan drew no conclusion
/// about, WHY it was skipped.
///
/// The annotation is appended to the component's own `properties`, so what the
/// caller submitted there is preserved beside it — never replaced: a
/// `properties` of any other type was refused when the document was validated
/// ([`submitted_properties`]), and is refused here too rather than overwritten,
/// so the rule holds at the site that writes as well as at the site that
/// checks. The walk is bounded exactly as the two resolution passes are: the
/// tree it walks came from [`resolve_component_tree`], which refuses anything
/// deeper.
fn annotate_skipped_components(
    components: &mut Value,
    skipped: &HashMap<String, SkipReason>,
    depth: usize,
) -> Result<(), ScanSbomError> {
    if depth > MAX_COMPONENT_NESTING {
        return Ok(());
    }
    let Some(entries) = components.as_array_mut() else {
        return Ok(());
    };
    for entry in entries {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let was_skipped = object
            .get("bom-ref")
            .and_then(Value::as_str)
            .and_then(|bom_ref| skipped.get(bom_ref))
            .copied();
        if let Some(reason) = was_skipped {
            let property = json!({
                "name": format!("{INSPECTOR_PROPERTY_NAMESPACE}:info"),
                "value": reason.info_message(),
            });
            match object.get_mut("properties") {
                Some(Value::Array(list)) => list.push(property),
                // The component carries no `properties` at all, so the
                // annotation is its first one.
                None | Some(Value::Null) => {
                    object.insert("properties".to_string(), json!([property]));
                }
                Some(_) => return Err(invalid(SbomDefect::InvalidValue("properties"))),
            }
        }
        if let Some(children) = object.get_mut("components") {
            annotate_skipped_components(children, skipped, depth + 1)?;
        }
    }
    Ok(())
}

/// What the SCAN stamps on a CycloneDX report, beside the findings: the instant
/// it answered at, the serial number it derived for a document that names none,
/// and the build identity of the scanner that answered it.
#[derive(Debug, Clone, Copy)]
struct ReportStamp<'s> {
    /// The content-derived `urn:uuid:` serial number, used only when the
    /// submitted document names none.
    serial_number: &'s str,
    /// The scan's instant, as `metadata.timestamp`.
    timestamp: &'s str,
    /// The scanning service's own build identity, as
    /// `metadata.tools.services[].version`.
    scanner_version: &'s str,
}

/// The input BOM augmented with the scan's results — the `CYCLONE_DX_1_5` form.
///
/// Every submitted member the report reads is read through the accessor that
/// states the type CycloneDX allows it, so a member this form would otherwise
/// have to replace is REFUSED (the document was already validated against the
/// same accessors, so this is the floor under that check, not a second rule).
fn cyclonedx_report(
    submitted: &Map<String, Value>,
    parsed: &ParsedSbom,
    findings: &[Finding],
    skipped: &HashMap<String, SkipReason>,
    summary: &ScanSummary,
    stamp: ReportStamp<'_>,
) -> Result<Value, ScanSbomError> {
    let mut report = submitted.clone();

    // `bomFormat`/`specVersion` are preserved: both were checked above, so they
    // already hold the only values this operation accepts.
    report.insert(
        "bomFormat".to_string(),
        Value::String(CYCLONEDX_BOM_FORMAT.to_string()),
    );
    report.insert(
        "specVersion".to_string(),
        Value::String(CYCLONEDX_SPEC_VERSION.to_string()),
    );
    // `serialNumber` identifies the BOM INSTANCE, so a submitted one is
    // PRESERVED for the same reason `version` is below — the report must not
    // misreport what the caller sent — and the content-derived URN is stamped
    // only when the submitted document names none (or names an empty one,
    // which identifies nothing). Either way the value is a function of the
    // submitted document, so a repeat of this `@idempotent` scan answers the
    // identical report.
    let submitted_serial =
        submitted_serial_number(&report)?.is_some_and(|serial| !serial.is_empty());
    if !submitted_serial {
        report.insert(
            "serialNumber".to_string(),
            Value::String(stamp.serial_number.to_string()),
        );
    }
    // `version` is the BOM's own revision number. It is PRESERVED as submitted —
    // the report must not misreport what the caller sent — and only defaulted
    // when the document states none, to the `1` the CycloneDX 1.5 schema
    // defaults it to. A member present as `null` states no value, so it is read
    // as absent and the default is stamped over it
    // ([`submitted_bom_version`]).
    if submitted_bom_version(&report)?.is_none() {
        report.insert("version".to_string(), json!(1));
    }

    // The resolved component tree, with the `bom-ref` this scan assigned on
    // every entry — the ref each `affects[].ref` resolves to — and the
    // scanner's own annotation on every component it drew no conclusion about.
    if let Some(components) = &parsed.components_value {
        let mut components = components.clone();
        annotate_skipped_components(&mut components, skipped, 0)?;
        report.insert("components".to_string(), components);
    }

    // The submitted `metadata` is the object the scan ADDS to: its own members
    // are carried over and the scan's are written beside them.
    let mut metadata = match submitted_metadata(&report)? {
        Some(existing) => existing.clone(),
        None => Map::new(),
    };
    metadata.insert(
        "timestamp".to_string(),
        Value::String(stamp.timestamp.to_string()),
    );

    // The scan reports WHO scanned the document: the scanning service, under
    // `metadata.tools.services`, beside whatever the submitted document already
    // said about the tools that produced it.
    let mut tools = match submitted_tools(&metadata)? {
        SubmittedTools::Absent => Map::new(),
        SubmittedTools::Object(existing) => existing.clone(),
        SubmittedTools::Entries(entries) => {
            let mut object = Map::new();
            object.insert(
                "components".to_string(),
                Value::Array(entries.iter().map(tool_component).collect()),
            );
            object
        }
    };
    let mut services = match submitted_services(&tools)? {
        Some(existing) => existing.clone(),
        None => Vec::new(),
    };
    services.push(scanner_service(stamp.scanner_version));
    tools.insert("services".to_string(), Value::Array(services));
    metadata.insert("tools".to_string(), Value::Object(tools));

    // And WHAT it found, as the namespaced scan summary, appended to the
    // document's own `metadata.properties` rather than replacing them.
    let mut properties = match submitted_properties(&metadata)? {
        Some(existing) => existing.clone(),
        None => Vec::new(),
    };
    properties.extend(summary.cyclonedx_properties());
    metadata.insert("properties".to_string(), Value::Array(properties));

    report.insert("metadata".to_string(), Value::Object(metadata));

    report.insert(
        "vulnerabilities".to_string(),
        Value::Array(findings.iter().map(cyclonedx_vulnerability).collect()),
    );

    Ok(Value::Object(report))
}

/// Which of the two Inspector-native forms a report is rendered in.
///
/// Both carry the same envelope and report the same matches; the form decides
/// how a `vulnerabilities` node is built. Either way that member is a LIST of
/// vulnerability nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorForm {
    /// `INSPECTOR`: one node per match — one component against one advisory —
    /// carrying no vendor-advisory data.
    PerMatch,
    /// `INSPECTOR_ALT`: one node per matched advisory, carrying every component
    /// of this scan that advisory affects plus the vendor advisories published
    /// for it.
    PerAdvisory,
}

/// The `vulnerablePackages[]` entry for one matched component.
fn inspector_vulnerable_package(finding: &Finding) -> Value {
    let component = &finding.component;
    json!({
        "name": component.package_name,
        "version": component.version,
        "packageManager": inspector_package_manager(&component.package_type),
        "fixedInVersion": finding.advisory.fixed_in_version,
        "purl": component.purl,
        "bomRef": component.bom_ref,
    })
}

/// The vendor advisories published for one matched advisory — the `INSPECTOR_ALT`
/// form's per-node vendor data.
///
/// One entry per matched advisory, every member of it a projection of the matched
/// [`VulnRecord`]: the advisory's own id, the publisher that issued it, the BASE
/// (NVD) severity the record holds — reported here beside the Inspector-adjusted
/// `severity` on the node rather than instead of it, which is exactly the
/// discrepancy the operation's documentation warns of — and the instants the
/// advisory was published and last revised. The entry names the advisory by
/// identifier and publisher and carries no link, for the reason
/// [`inspector_finding`] gives.
///
/// The member is modelled as a LIST of vendor advisories, so the single
/// projection is returned as a one-element list rather than as a bare object.
fn vendor_advisories(advisory: &VulnRecord) -> Vec<Value> {
    vec![json!({
        "id": advisory.cve_id,
        "source": advisory.source,
        "severity": SeverityBucket::of(&advisory.nvd_severity).as_str(),
        "vendorCreatedAt": advisory.published,
        "vendorUpdatedAt": advisory.modified,
    })]
}

/// One `INSPECTOR_ALT` vulnerability node: the advisory `matches` all share,
/// every component of this scan it affects, and the vendor advisories for it.
///
/// The node is the same projection of the advisory the `INSPECTOR` form reports,
/// so the two forms can never disagree about a match; it differs only in listing
/// one `vulnerablePackages[]` entry per affected component and in carrying
/// `vendorAdvisories`. `None` for an empty group, which names no advisory.
fn inspector_alt_vulnerability(matches: &[&Finding]) -> Option<Value> {
    let primary = matches.first()?;
    let mut node = inspector_finding(primary);
    if let Value::Object(members) = &mut node {
        members.insert(
            "vulnerablePackages".to_string(),
            Value::Array(
                matches
                    .iter()
                    .map(|finding| inspector_vulnerable_package(finding))
                    .collect(),
            ),
        );
        members.insert(
            "vendorAdvisories".to_string(),
            Value::Array(vendor_advisories(&primary.advisory)),
        );
    }
    Some(node)
}

/// The Inspector-native finding for one match.
///
/// The advisory is named by its `source` and `id` alone: this form carries
/// neither `sourceUrl` nor `referenceUrls`, so the only address any report form
/// publishes is the publisher's page the CycloneDX node names on its own
/// `source` (see [`cyclonedx_vulnerability`]).
fn inspector_finding(finding: &Finding) -> Value {
    let advisory = &finding.advisory;
    let component = &finding.component;
    let remediation = advisory
        .fixed_in_version
        .as_ref()
        .map(|fixed| format!("Update {} to {fixed} or later.", component.package_name));

    // The adjusted (Inspector) and base (advisory) scores are each reported
    // only when the advisory carries the vector they were computed from, and
    // with the CVSS version that vector names rather than an assumed one.
    let adjusted = cvss_version(&advisory.inspector_cvss_vector).map(|version| {
        json!({
            "score": advisory.inspector_score,
            "scoringVector": advisory.inspector_cvss_vector,
            "version": version,
            "scoreSource": INSPECTOR_RATING_SOURCE,
        })
    });
    let base: Vec<Value> = cvss_version(&advisory.cvss_v3_vector)
        .map(|version| {
            json!({
                "source": advisory.source,
                "version": version,
                "baseScore": advisory.cvss_v3_score,
                "scoringVector": advisory.cvss_v3_vector,
            })
        })
        .into_iter()
        .collect();

    json!({
        "id": advisory.cve_id,
        "title": format!("{} - {}", advisory.cve_id, component.package_name),
        // The report spells a severity the way it spells the per-severity
        // counts beside it, so a reader keying on one can read the other.
        "severity": SeverityBucket::of(&advisory.inspector_severity).as_str(),
        "description": advisory.description,
        "source": advisory.source,
        "cwes": advisory.cwes.iter().map(|cwe| format!("CWE-{cwe}")).collect::<Vec<String>>(),
        "vendorCreatedAt": advisory.published,
        "vendorUpdatedAt": advisory.modified,
        "inspectorScore": advisory.inspector_score,
        // The exploit-probability estimate, reported beside the severity scores
        // for the same reason the CycloneDX form reports it beside its CVSS
        // ratings: it is a different quantity on a different scale. Absent for
        // an advisory the scoring system has published no estimate for.
        "epss": advisory.epss_score.map(|probability| json!({ "score": probability })),
        "inspectorScoreDetails": { "adjustedCvss": adjusted },
        "cvss": base,
        "vulnerablePackages": [ inspector_vulnerable_package(finding) ],
        "remediation": { "recommendation": { "text": remediation } },
    })
}

/// The Inspector-native report — the `INSPECTOR` and `INSPECTOR_ALT` forms.
///
/// Both carry the same envelope and the same matches, computed from the same
/// findings as the CycloneDX form, so the formats can never disagree. In BOTH,
/// `vulnerabilities` is a LIST of vulnerability nodes, each an object naming at
/// least the advisory `id` and the `severity` it was rated at: a client reads a
/// report's collection member by indexing and iterating the entities in it, so
/// an entity collection is never presented as a map keyed by one of the
/// entities' own fields. The forms differ in how a node is built
/// ([`InspectorForm`]): `INSPECTOR` reports one node per match, in the report's
/// order, with no vendor-advisory data; `INSPECTOR_ALT` reports one node per
/// matched advisory, listing every component that advisory affects, and carries
/// `vendorAdvisories` on each node — the data a caller selects that form in
/// order to read.
///
/// What the report says about the scan as a whole is NOT chosen either: beside
/// `vulnerabilities` it carries the unconditional per-severity
/// `vulnerability_count`, and `messages`, one entry per component the scan drew
/// no conclusion about — each carrying the condition that fired for it — so a
/// reader can tell a clean scan from an uncovered one, and an uncovered
/// ecosystem from an uncovered package, instead of reading an empty report as
/// any of them.
///
/// The rest of the ENVELOPE IS CHOSEN, NOT MODELLED. `sbom` is a Smithy
/// `document`, so the wrapper members (`scanId`, `scanTimestamp`, `sbomFormat`,
/// `sbomSpecVersion`, `componentCount`, `findingCount`) are this emulator's
/// choice. `findingCount` counts MATCHES — one component against one advisory —
/// which is the `vulnerabilities` length in the `INSPECTOR` form and at least it
/// in `INSPECTOR_ALT`, where the matches of one advisory share a node. What is
/// NOT chosen is the content of a match: every field of it is a projection of a
/// matched [`crate::state::VulnRecord`] onto the component it was matched
/// against, the same records the CycloneDX form reports.
fn inspector_report(
    scan_id: &str,
    timestamp: &str,
    component_count: usize,
    findings: &[Finding],
    messages: Vec<Value>,
    summary: &ScanSummary,
    form: InspectorForm,
) -> Value {
    let vulnerabilities: Vec<Value> = match form {
        InspectorForm::PerMatch => findings.iter().map(inspector_finding).collect(),
        InspectorForm::PerAdvisory => {
            // Grouped by the matched ADVISORY — a vulnerability as it applies to
            // one package, which is what one stored record is — so every match
            // on a node shares the package the node names, and a vulnerability
            // affecting two packages reports one node per package rather than
            // one node naming whichever came first. The package is the
            // advisory's whole coordinate, distribution included, so one
            // vulnerability published for two distributions' builds of one name
            // reports a node per distribution. The key also orders the nodes (by
            // id, then by the package), so a repeat of this `@idempotent` scan
            // answers the identical list.
            let mut by_advisory: BTreeMap<(&str, &str, &str, &str), Vec<&Finding>> =
                BTreeMap::new();
            for finding in findings {
                let advisory = &finding.advisory;
                by_advisory
                    .entry((
                        advisory.cve_id.as_str(),
                        advisory.package_type.as_str(),
                        advisory.distribution.as_deref().unwrap_or_default(),
                        advisory.package_name.as_str(),
                    ))
                    .or_default()
                    .push(finding);
            }
            by_advisory
                .values()
                .filter_map(|matches| inspector_alt_vulnerability(matches))
                .collect()
        }
    };

    json!({
        "scanId": scan_id,
        "scanTimestamp": timestamp,
        "sbomFormat": CYCLONEDX_BOM_FORMAT,
        "sbomSpecVersion": CYCLONEDX_SPEC_VERSION,
        "componentCount": component_count,
        "findingCount": findings.len(),
        "vulnerabilities": vulnerabilities,
        "vulnerability_count": summary.vulnerability_count(),
        "messages": messages,
    })
}

// ---------------------------------------------------------------------------
// The operation
// ---------------------------------------------------------------------------

/// The `ScanSbom` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ScanSbom<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ScanSbom<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ScanSbom<Eng>
{
    type Input = ScanSbomInput;
    type Output = ScanSbomOutput;
    type Error = ScanSbomError;

    const NAME: &'static str = "ScanSbom";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_SBOM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_SBOM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // IAM decides a signed call before the service acts on the request, so
        // the gate is asked FIRST: with no store guard held, against the
        // resource this operation's own modeled path addresses, and before the
        // submitted document is read. A refused caller is told nothing the scan
        // would have computed and leaves no record behind.
        if let Some(message) = crate::service::authorization::refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            &crate::service::authorization::resource_addressed(ctx.parts(), SCAN_SBOM_PATH),
        ) {
            return Err(denied(message));
        }

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the inspector-scan vulnerability database is unavailable"))?;

        // `outputFormat` is OPTIONAL, and a request that names none is answered
        // the Inspector-native report: the format a scan reports in by default
        // is the service's own, not the CycloneDX document the request carried.
        //
        // An `outputFormat` outside the modeled enum never reaches here — the
        // boundary refuses it against the model's own value set, before any
        // handler runs — so this arm is the defensive floor under that check,
        // and it names the member and the values it accepts.
        let output_format = match input.output_format.as_deref() {
            None => OutputFormat::Inspector,
            Some(requested) => OutputFormat::from_wire(requested).ok_or_else(|| {
                // The member that failed is the request's own `outputFormat`,
                // not one of the submitted document's, so the refusal names
                // that member and states the same class of failure a
                // member-level defect of the document does.
                let condition = format!(
                    "outputFormat must be one of {}, got \"{requested}\"",
                    super::types::OUTPUT_FORMAT_VALUES.join(", ")
                );
                refused(condition)
            })?,
        };

        // A document this operation cannot take in at all (too deeply nested,
        // or carrying a number JSON cannot express) is refused for the same
        // class as one that is not a JSON object: it does not parse.
        let submitted =
            document_to_json(&input.sbom, 0).ok_or_else(|| invalid(SbomDefect::CannotParse))?;
        let parsed = parse_sbom(&submitted)?;
        let digest = sbom_digest(&submitted)
            .ok_or_else(|| internal("the submitted document could not be canonicalized"))?;
        let serial_number = submitted
            .get("serialNumber")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Match every component against the stored advisories, note WHY the
        // scan drew no conclusion about the ones it could not rule on, read the
        // scanner's build identity, then settle this scan's identity, all under
        // one lock.
        let (scan_id, received_at, findings, skipped, scanner_version) = {
            let mut store = state.lock();

            let mut findings: Vec<Finding> = Vec::new();
            // The components the scan could not rule on, each with the skip
            // condition that fired for it: the ecosystem is one the scanner
            // does not scan, or the store holds no advisory for the package at
            // all. Either is a different answer from "a rule applied and
            // cleared this version".
            let mut skipped: HashMap<String, SkipReason> = HashMap::new();
            for component in &parsed.components {
                if let Some(reason) = skip_reason(&store, component) {
                    skipped.insert(component.bom_ref.clone(), reason);
                    // A component the scan drew no conclusion about reports no
                    // finding: one report cannot both rule on a component and
                    // declare that it was not ruled on. The lookup is skipped
                    // rather than run and discarded, so one condition decides
                    // both halves of the report.
                    continue;
                }
                for index in store.matching_advisories(
                    &component.package_type,
                    component.distribution(),
                    &component.package_name,
                    &component.version,
                ) {
                    if let Some(advisory) = store.vulnerability_db.get(index) {
                        findings.push(Finding {
                            component: component.clone(),
                            advisory: advisory.clone(),
                        });
                    }
                }
            }
            // Deterministic order, so a repeat of this `@idempotent` scan
            // reports the identical set in the identical sequence: by the
            // affected component's ref, then by vulnerability id.
            findings.sort_by(|a, b| {
                (&a.component.bom_ref, &a.advisory.cve_id)
                    .cmp(&(&b.component.bom_ref, &b.advisory.cve_id))
            });
            findings.dedup_by(|a, b| {
                a.component.bom_ref == b.component.bom_ref
                    && a.advisory.cve_id == b.advisory.cve_id
            });

            // A repeat of an identical scan keeps the IDENTITY the first one
            // minted — same scan id, same instant. The findings themselves are
            // recomputed against the advisory data as it stands now, so a
            // repeat answers the identical report for an unchanged store and
            // reports the current advisories for a changed one, rather than
            // replaying a stale finding set under a fresh identity.
            let already_scanned = store
                .scan_for(&digest, output_format)
                .map(|record| (record.scan_id.clone(), record.received_at));
            let (scan_id, received_at) = match already_scanned {
                Some(identity) => identity,
                None => (store.next_scan_id(), SystemTime::now()),
            };
            // Recorded either way, so the stored record always describes the
            // report that was just answered. `record_scan` replaces the record
            // for a repeat and bounds the store.
            store.record_scan(ScanRecord {
                scan_id: scan_id.clone(),
                received_at,
                output_format,
                component_count: parsed.components.len(),
                sbom_serial_number: serial_number,
                finding_ids: findings
                    .iter()
                    .map(|f| f.advisory.cve_id.clone())
                    .collect(),
                sbom_digest: digest.clone(),
            });
            (
                scan_id,
                received_at,
                findings,
                skipped,
                store.scanner_build().version(),
            )
        };

        let timestamp = rfc3339(received_at);
        let component_count = parsed.components.len();
        let summary = ScanSummary::of(&findings);
        // One message per component the scan drew no conclusion about, carrying
        // the condition that fired for THAT component and naming it by the purl
        // the submitted document states for it — a component the document gives
        // no purl for has no coordinate to name it by, so the entry carries the
        // reason alone.
        let messages: Vec<Value> = parsed
            .components
            .iter()
            .filter_map(|component| {
                skipped
                    .get(&component.bom_ref)
                    .map(|reason| (component, *reason))
            })
            .map(|(component, reason)| {
                let mut message = Map::new();
                if let Some(purl) = &component.purl {
                    message.insert("purl".to_string(), Value::String(purl.clone()));
                }
                message.insert(
                    "info_message".to_string(),
                    Value::String(reason.info_message().to_string()),
                );
                Value::Object(message)
            })
            .collect();

        let report = match output_format {
            OutputFormat::CycloneDx15 => cyclonedx_report(
                submitted
                    .as_object()
                    .ok_or_else(|| internal("the submitted document is not an object"))?,
                &parsed,
                &findings,
                &skipped,
                &summary,
                ReportStamp {
                    serial_number: &serial_number_from_digest(&digest),
                    timestamp: &timestamp,
                    scanner_version: &scanner_version,
                },
            )?,
            OutputFormat::Inspector => inspector_report(
                &scan_id,
                &timestamp,
                component_count,
                &findings,
                messages,
                &summary,
                InspectorForm::PerMatch,
            ),
            OutputFormat::InspectorAlt => inspector_report(
                &scan_id,
                &timestamp,
                component_count,
                &findings,
                messages,
                &summary,
                InspectorForm::PerAdvisory,
            ),
        };

        let rendered = json_to_document(&report, 0).ok_or_else(|| {
            internal("the vulnerability report nests deeper than it can be serialized")
        })?;

        // Audit record for an operation that WRITES to state (the scan is
        // recorded in the audit store above): who asked, what was scanned, how
        // it ended. Emitted only once the report this reports on has been
        // rendered, so the audit trail never claims a scan the caller was
        // answered an error for. Wire metadata and server-minted identifiers
        // only — never the submitted document, a component coordinate or any
        // part of either.
        ::tracing::info!(
            audit = true,
            operation = "ScanSbom",
            outcome = "scanned",
            caller_akid = %::engine::extract_akid(&ctx.parts().headers),
            scan_id = %scan_id,
            output_format = output_format.as_wire(),
            component_count,
            finding_count = findings.len(),
            "sbom scanned"
        );

        Ok(ScanSbomOutput {
            sbom: Some(rendered),
        })
    }
}

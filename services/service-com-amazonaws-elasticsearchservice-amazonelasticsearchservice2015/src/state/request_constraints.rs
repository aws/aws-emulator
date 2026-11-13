// SPDX-License-Identifier: CC0-1.0

//! The input constraints THIS surface applies itself, and how it words a refusal of one.
//!
//! The boundary walk in front of a handler checks every constraint the model declares on
//! a member it can reach. Two classes it cannot reach are this module's:
//!
//! * a member whose string shape in THIS model declares no syntax while the RESOURCE it
//!   addresses declares one. `PackageID` here is a bare string
//!   (`smithy_query(query="shape",shape="PackageID")` on this model), while the shared
//!   package resource states the id's syntax — `com.amazonaws.opensearch#PackageID`
//!   `@pattern("^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$")` — and the `/2015-01-01/packages`
//!   routes address that one resource, so that is the syntax of a package id on this
//!   surface too.
//! * a MAP whose KEY shape is an enumerated type. The validating walk treats a map key as
//!   a path SEGMENT rather than as a constrained leaf, so nothing ahead of the handler
//!   checks the key against its shape's value set.
//!
//! Both are INPUT constraints, and an input constraint is evaluated BEFORE the request's
//! resources are resolved: a value outside a member's declared syntax is refused by
//! naming that member, never reported as a resource that does not exist — which is a
//! different claim, about a different thing, and sends a caller to fix the wrong half of
//! its request.
//!
//! The WORDS are the framework's and the fleet's rather than this crate's: the envelope and
//! the map-key clause ([`::service_common::validation_message::map_key_requirements_refusal`],
//! [`::aws_smithy_schema_server::envelope`]), the member paths
//! ([`::aws_smithy_schema_server::constraints::path::member`]) and every requirement
//! sentence ([`::service_common::validation_message::requirement`]) come from there, so a
//! clause a handler states cannot drift from one the boundary states for the same
//! constraint.

use std::collections::HashMap;
use std::sync::LazyLock;

use ::aws_smithy_schema_server::constraints::path;
use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use ::service_common::validation_message::{map_key_requirements_refusal, requirement};

use crate::ops::types::{LogPublishingOption, LOG_TYPE_VALUES};

/// The syntax a VPC endpoint id has to satisfy, as this model declares it
/// (`smithy_query(query="shape",shape="VpcEndpointId")`): the service's own `aos-` prefix
/// followed by an alphanumeric run.
///
/// Listed in [`PUBLISHED_AS_DECLARED`] because the API publishes this expression WITH the
/// pair: the `Pattern` the AWS API reference documents for a VPC endpoint id is
/// `^aos-[a-zA-Z0-9]*$`, anchors included, and a recorded refusal of a value outside it
/// quotes exactly that.
pub const VPC_ENDPOINT_ID_PATTERN: &str = r"^aos-[a-zA-Z0-9]*$";

/// The model `@pattern` texts this API publishes EXACTLY as the model writes them, anchors
/// included.
///
/// WHY A TABLE and not a rule over the text: the expression a clause quotes is the one the
/// SERVICE publishes for that member's shape — the `Pattern` its API reference documents —
/// and for part of this surface that text is the model's own while for the rest it is the
/// model's text without the whole-value pair ([`quoted_pattern`]). Which of the two a shape
/// is published with is not recoverable from the text: both are spelled `^…$` in the model,
/// so it is reference data about the shape, read off what the API publishes for it. The
/// EXPRESSION is the key rather than the member path, because what is published belongs to
/// the shape: every member targeting it — in any operation, bound to a URI label, to a body
/// member or to an element of a list — quotes the one expression alike, and so does the
/// element set stated at a collection of them.
///
/// Recorded both ways on this surface: `DomainName`'s `^[a-z][a-z0-9\-]+$` is published
/// `[a-z][a-z0-9\-]+` (AWS documents that member's `Pattern` without the pair) while
/// [`VPC_ENDPOINT_ID_PATTERN`] is published as declared. A shape absent from here is
/// published without the pair, which is every other shape of this model.
pub const PUBLISHED_AS_DECLARED: &[&str] = &[VPC_ENDPOINT_ID_PATTERN];

/// How this API quotes a `@pattern` requirement in a refusal: the expression the SERVICE
/// publishes for the member's shape.
///
/// For a shape [`PUBLISHED_AS_DECLARED`] names, that is the model's text verbatim. For every
/// other shape it is the model's text without the whole-value pair the model wrapped it in —
/// and the model's text unchanged where what that pair encloses carries an anchor of its
/// OWN, because the two ends of such a text are the first and last BRANCH's anchors and
/// dropping them would state a requirement of no branch.
///
/// Recorded all three ways on this surface: `DomainName`'s `^[a-z][a-z0-9\-]+$` is published
/// `[a-z][a-z0-9\-]+`, the branch-anchored alternations (`ElasticsearchVersionString`, the
/// package id below) are published exactly as the model declares them, and so is the VPC
/// endpoint id, which the pair-dropping rule would otherwise have stripped.
///
/// The RULE is the fleet's one home for it
/// ([`::service_common::validation::unanchored_unless_published_as_declared`]), called rather
/// than restated so this API cannot drift from another recorded stating both forms; what is
/// this crate's is the row above, which is about this model's shapes. ONE home for the
/// dialect, reached by the refusal seam (`crate::service`) and by the handler-stated clauses
/// here alike, so the boundary and a handler cannot quote one member's expression two ways.
/// The CHECK is always the model's pattern as written.
pub fn quoted_pattern(pattern: &str) -> String {
    ::service_common::validation::unanchored_unless_published_as_declared(
        pattern,
        PUBLISHED_AS_DECLARED,
    )
}

/// The syntax a package id has to satisfy, as the shared package resource declares it
/// (`com.amazonaws.opensearch#PackageID`): either a published-package id (`F`/`G` followed
/// by digits) or an account's own `pkg-` id over lowercase hex.
///
/// Both forms this emulator MINTS satisfy it — `crate::state::package_id` writes
/// `pkg-<16 hex>` and `crate::state::published_package_id` writes `pkg-<40 hex>` — so an
/// id handed back by a create is accepted by every later call that names it.
pub const PACKAGE_ID_PATTERN: &str = r"^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$";

/// [`PACKAGE_ID_PATTERN`] compiled once, as the pattern is a constant rather than anything
/// a request carries.
static PACKAGE_ID: LazyLock<::regex::Regex> = LazyLock::new(|| {
    ::regex::Regex::new(PACKAGE_ID_PATTERN).expect("the package id syntax is a valid expression")
});

/// `Some(message)` when `package_id` is outside the syntax a package id has to satisfy —
/// the whole `ValidationException` message, stating the member, the value the request
/// carried and the expression it had to satisfy.
///
/// A find rather than an anchored match is exactly Smithy's own `@pattern` semantics, and
/// this expression anchors its own branches, so the find decides the whole value.
pub fn package_id_refusal(package_id: &str) -> Option<String> {
    if PACKAGE_ID.is_match(package_id) {
        return None;
    }
    Some(member_refusal(
        "PackageID",
        package_id,
        ViolationKind::Pattern(quoted_pattern(PACKAGE_ID_PATTERN)),
    ))
}

/// The whole message refusing one member's value, in the framework's own envelope and
/// clause: `kind` is the requirement the value broke, and the member is spelled the way a
/// clause spells one (lowerCamel, so `PackageID` reads `packageID`).
fn member_refusal(member: &str, value: &str, kind: ViolationKind) -> String {
    let violation = Violation {
        path: path::member(member),
        kind,
        value: Some(value.to_string()),
        sensitive: false,
    };
    let clause = violation.message();
    envelope(std::slice::from_ref(&violation)).unwrap_or(clause)
}

/// The log types this surface admits, in the ORDER its refusal states them: the order the
/// front door's own hash table over the admissible values is walked
/// ([`::service_common::validation_message::hashed_enum_order`]), which is neither the
/// model's declaration order nor alphabetical. MEMBERSHIP is the model's — the set is the
/// generated [`LOG_TYPE_VALUES`], this map's key shape `LogType` — and the order is
/// COMPUTED from those values, so a value the enum gains is stated in its own place rather
/// than needing a row anywhere.
fn log_types_as_stated() -> Vec<String> {
    ::service_common::validation_message::hashed_enum_order(LOG_TYPE_VALUES)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The member a log-publishing refusal is stated at, as the model names it.
const LOG_PUBLISHING_OPTIONS: &str = "LogPublishingOptions";

/// `Some(message)` when `options` is keyed by a log type the model's `LogType` enum does
/// not declare — the whole `ValidationException` message, stating the member, the map the
/// request carried and the key set a log type has to come from.
///
/// The clause, the bracketed list and the envelope are the fleet's one home for them
/// ([`::service_common::validation_message::map_key_requirements_refusal`]), so a key clause
/// this handler states cannot drift from another service's. ONE validation error for the
/// member however many of its keys are outside the set: the clause is about the member's
/// value, and that value is the one map — quoted where the fleet's renderer will quote a map
/// that size, and stated without a value where it will not, so one request cannot size the
/// message it is refused with.
pub fn log_publishing_options_refusal(
    options: &HashMap<String, LogPublishingOption>,
) -> Option<String> {
    if options
        .keys()
        .all(|log_type| LOG_TYPE_VALUES.contains(&log_type.as_str()))
    {
        return None;
    }
    let submitted = rendered_log_publishing_options(options);
    Some(map_key_requirements_refusal(
        &path::member(LOG_PUBLISHING_OPTIONS),
        submitted.as_deref(),
        &[requirement(ViolationKind::EnumValue(log_types_as_stated()))],
    ))
}

/// `options` as the refusal quotes it back: the map form AWS echoes a map-valued member in
/// ([`::service_common::validation::rendered_map_in_key_order`], the fleet's one renderer of
/// that form, which answers `None` for a map larger than a clause quotes), each value the
/// printed form of the `LogPublishingOption` it carried.
fn rendered_log_publishing_options(
    options: &HashMap<String, LogPublishingOption>,
) -> Option<String> {
    let entries: Vec<(&str, String)> = options
        .iter()
        .map(|(log_type, option)| (log_type.as_str(), rendered_log_publishing_option(option)))
        .collect();
    ::service_common::validation::rendered_map_in_key_order(&entries)
}

/// One `LogPublishingOption` as the validation layer in front of the service prints a
/// modelled structure ([`::service_common::validation::rendered_record`]): the shape's name
/// over EVERY member the shape declares, in declaration order, with one the request left
/// unset stated as the fleet's `UNSET_MEMBER`.
fn rendered_log_publishing_option(option: &LogPublishingOption) -> String {
    let arn = option
        .cloud_watch_logs_log_group_arn
        .as_deref()
        .unwrap_or(::service_common::validation::UNSET_MEMBER);
    let enabled = option.enabled.map_or_else(
        || ::service_common::validation::UNSET_MEMBER.to_string(),
        |enabled| enabled.to_string(),
    );
    let arn_member = path::member("CloudWatchLogsLogGroupArn");
    let enabled_member = path::member("Enabled");
    ::service_common::validation::rendered_record(
        "LogPublishingOption",
        &[
            (arn_member.as_str(), arn),
            (enabled_member.as_str(), enabled.as_str()),
        ],
    )
}

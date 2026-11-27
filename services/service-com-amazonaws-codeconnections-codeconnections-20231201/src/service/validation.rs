// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran, and
//! the one input constraint it enforces more tightly than its public model states.
//!
//! The framework walks the modelled constraints and hands [`Refusal`] to `service.rs`
//! STRUCTURALLY — one [`Violation`] per offending value, carrying the path it sits at,
//! what it failed and what it held — and leaves the wording to the service, because no
//! model records it. This module is that wording for every modelled input constraint of
//! this service: ONE formatter, reached by every operation's boundary validation, so no
//! operation can word a refusal its own way.
//!
//! The dialect it implements, as CodeConnections states a refusal:
//!
//! * the envelope is the AWS framework's — `N validation error(s) detected: ` over one
//!   clause per violation — and a clause names the offending MEMBER PATH and the
//!   CONSTRAINT that member violated;
//! * a value outside a CONSTRAINT (`@length`, `@pattern`, `@range`, `@uniqueItems`) is
//!   reported WITHOUT the value: the clause reads `Value at '<path>' …`. The constraint
//!   and the member are what a caller has to act on, and the value is the caller's own
//!   request — often long (a 620-byte endpoint) and sometimes private — so quoting it
//!   back adds nothing and amplifies the request into the response;
//! * a value outside an ENUM's value set IS quoted (`Value '<value>' at '<path>' …`):
//!   such a value is one the service could not map to a member of the type at all, and
//!   the set it was checked against is only actionable beside the value that missed it;
//! * an ABSENT `@required` member is reported as the `null` it was — a fact of the
//!   request's shape rather than a value the caller sent;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors the
//!   model spells it with ([`service_common::validation::unanchored`], this emulator's
//!   one statement of that rendering): a caller that lifts the advertised expression out
//!   of the message and re-applies it locally must get the expression the service
//!   advertises;
//! * an enum requirement reads `Member must be one of: [ … ]`, over the values of the type
//!   in the order this service advertises that value set in ([`advertised_enum_values`]),
//!   which for one of its types is not the order the model declares them in.
//!
//! Nothing here is keyed to an operation or to a recorded value: the rule is applied to
//! whatever [`Violation`] the walk produces, so it holds for every constrained member of
//! every operation of this service — a `ConnectionName`'s `@length`, a `HostArn`'s or a
//! `RoleArn`'s `@pattern`, a `MaxResults`' `@range`, a `ProviderType`'s or a
//! `SyncConfigurationType`'s value set alike.
//!
//! # The connection-scoped ARN expression
//!
//! One member class is stated here as well as in the model. The published Smithy model
//! targets every connection-ARN member at `ConnectionArn`, whose `@pattern` is this
//! service's connection-scoped expression ([`CONNECTION_ARN_PATTERN`]) — so the framework's
//! own walk refuses a host or repository-link ARN passed where a connection ARN belongs,
//! and a clause about such a member advertises that expression whether it came from the
//! walk or from a handler. The REPOSITORY-LINK surface checks the same expression itself
//! ([`is_connection_arn`], run by its handlers), reading the ARN field by field: the
//! enforced check and the advertised text are the SAME constant, so a caller of that
//! surface that re-applies the expression it was given reproduces the decision the service
//! made, and a model that ever loosens that member again does not loosen what the surface
//! accepts.
//!
//! The advertised expression is the service's whichever operation refused, because the
//! framework hands this seam the violations and NOT the operation they came from, and
//! because the expression is a property of the ARN form rather than of one call. What
//! each operation ENFORCES is its own: the CONNECTION surface (`GetConnection`,
//! `DeleteConnection`) resolves every ARN its model admits and answers this service's
//! fixed "no connection exists" sentence for one that resolves to nothing — recorded
//! behaviour, pinned by `tests/all/refusal_messages.rs`.
//!
//! A violation class this module has no wording for declines the WHOLE refusal (`None`),
//! which keeps the framework's default sentence rather than inventing one:
//! [`ViolationKind`] is `#[non_exhaustive]`, so a class can be added upstream.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with.
///
/// This service's model declares no `ValidationException` of its own
/// (`smithy_query(query="digest")` lists none among its errors), so the code is the
/// synthetic `smithy.framework#ValidationException` the framework already names for a
/// boundary refusal — restated because `override_refusal` replaces code and message
/// TOGETHER, and matching `crate::scaffolded::Scaffolded::validation`, which is how a
/// handler of this crate answers the same refusal class.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The expression this service holds a connection ARN to, as it advertises it: this
/// service's own namespace (the current `codeconnections` or the legacy
/// `codestar-connections`), a region, a 12-digit account and a `connection/<id>`
/// resource.
///
/// Written WITHOUT the `^` / `$` pair a Smithy `@pattern` is spelled with, because that
/// is the form a clause quotes; [`matches_connection_arn_expression`] is the whole-value
/// check of the same expression, so the expression a caller is given is the expression the
/// caller's value was run against.
/// The partition branch (`aws(-[\w]+)*`), the namespace alternation, the region and account
/// bounds and the `connection/<id>` resource are the published model's own
/// (`smithy_query(query="shape", shape="ConnectionArn")`), restated here as the text a
/// clause quotes and as the check below runs.
pub(crate) const CONNECTION_ARN_PATTERN: &str = r"arn:aws(-[\w]+)*:(codestar-connections|codeconnections):[a-z0-9\-]+:[0-9]{12}:connection\/[\w\-]+";

/// The path the framework reports a connection-ARN member at.
///
/// Every member of this service that names a connection by ARN is modelled
/// `ConnectionArn` (`GetConnection`, `DeleteConnection`, `CreateRepositoryLink` and
/// `UpdateRepositoryLink`, from `smithy_query(query="operation")` on each), and the walk
/// spells a member path in lowerCamel, so this is that one spelling — pinned against the
/// framework's own speller by the test below rather than trusted as a literal.
pub(crate) const CONNECTION_ARN_MEMBER: &str = "connectionArn";

/// [`CONNECTION_ARN_PATTERN`] as a CHECK: the expression transcribed segment by segment,
/// so the value a caller sent is judged by the expression the refusal advertises.
///
/// Transcribed rather than run through a regex engine because this crate already reads an
/// ARN of this service field by field — [`crate::state`]'s own ARN predicates, which the
/// page-token reader uses — and the two readings of "is this a connection ARN" belong in
/// the same shape, not in two engines. The two are DELIBERATELY different rules and say so
/// at each site: `state::is_connection_arn` admits only an ARN this service MINTS (the
/// current `codeconnections` namespace, any resource id the minter produced), because a
/// page token is one the service issued itself, while this one admits every ARN the
/// PUBLISHED expression admits — the legacy namespace included — because that is what a
/// caller may legitimately hold.
///
/// `\w` is ASCII here (`[0-9A-Za-z_]`), which is what the `@pattern` of a Smithy model
/// means: the service compiles it as a Java expression, whose `\w` is ASCII unless the
/// pattern asks for Unicode classes, and this one does not.
fn matches_connection_arn_expression(value: &str) -> bool {
    // `arn:<partition>:<namespace>:<region>:<account>:connection/<id>` — six fields, the
    // last of which keeps any `:` it contains, as a resource id may.
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        // `aws(-[\w]+)*`
        && is_partition_segment(fields[1])
        // `(codestar-connections|codeconnections)`
        && SERVICE_NAMESPACES.contains(&fields[2])
        // `[a-z0-9\-]+`
        && is_region_segment(fields[3])
        // `[0-9]{12}`
        && fields[4].len() == 12
        && fields[4].bytes().all(|byte| byte.is_ascii_digit())
        // `connection\/[\w\-]+`
        && fields[5]
            .strip_prefix("connection/")
            .is_some_and(is_resource_id_segment)
}

/// The namespaces the expression's alternation names: this service's current one and the
/// legacy one it was renamed from, as the model's own `HostArn` pattern spells them.
const SERVICE_NAMESPACES: &[&str] = &["codestar-connections", "codeconnections"];

/// The expression's `aws(-[\w]+)*`: `aws`, then zero or more `-`-separated word groups.
fn is_partition_segment(segment: &str) -> bool {
    let mut groups = segment.split('-');
    groups.next() == Some("aws")
        && groups.all(|group| !group.is_empty() && group.bytes().all(is_word_byte))
}

/// The expression's `[a-z0-9\-]+`.
fn is_region_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The expression's `[\w\-]+`.
fn is_resource_id_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|byte| is_word_byte(byte) || byte == b'-')
}

/// One byte of the expression's `\w`: an ASCII letter, digit or underscore.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `value` is a connection ARN as this service's connection-scoped expression
/// spells one.
pub(crate) fn is_connection_arn(value: &str) -> bool {
    matches_connection_arn_expression(value)
}

/// Whether `path` names a member that holds a connection ARN — the last segment of the
/// path, so a connection ARN nested inside a structure or a collection is recognised too.
fn names_a_connection_arn(path: &str) -> bool {
    path.split('.').next_back() == Some(CONNECTION_ARN_MEMBER)
}

/// The expression a `@pattern` clause at `path` advertises: the connection-scoped one
/// for a connection-ARN member, and otherwise the model's own pattern with its anchors
/// dropped.
///
/// Keyed to the member's ARN FORM and not to an operation, because the seam this is
/// reached through is handed the violations without the operation they came from — see the
/// module docs for which operations then enforce that expression and which resolve an ARN
/// their model admits.
fn advertised_pattern(path: &str, model_pattern: &str) -> String {
    if names_a_connection_arn(path) {
        return CONNECTION_ARN_PATTERN.to_string();
    }
    ::service_common::validation::unanchored(model_pattern)
}

/// The order this service advertises the members of its `ProviderType` value set in.
///
/// The sequence inside the brackets of an enum clause is FIXED TEXT of the refusal, not a
/// rendering of the model: this service states these members in this order, which is
/// NOT the order the model declares them in (`smithy_query(query="shape",
/// shape="ProviderType")` gives `Bitbucket, GitHub, GitHubEnterpriseServer, GitLab,
/// GitLabSelfManaged, AzureDevOps, GitHubEnterpriseCloudDataResidency`). A caller that
/// reads the bracketed list back to offer a user the values it may send has to be given the
/// list the service states, so the sequence is stated here beside the message wording
/// rather than taken from the shape's declaration.
///
/// The membership is the model's, so a value the shape gains joins the sequence — otherwise
/// the clause falls back to declaration order and advertises none of this order at all (see
/// [`advertised_enum_values`] and its tests). The six values the recorded refusals state
/// keep the order they state them in, and a value those recordings could not have named
/// (`GitHubEnterpriseCloudDataResidency`, which the shape gained later) is stated after
/// them.
const PROVIDER_TYPE_ADVERTISED_ORDER: &[&str] = &[
    "GitLab",
    "GitHub",
    "GitHubEnterpriseServer",
    "AzureDevOps",
    "GitLabSelfManaged",
    "Bitbucket",
    "GitHubEnterpriseCloudDataResidency",
];

/// Every value set this service advertises in an order of its own.
///
/// A set that is not listed here is advertised in the order the walk reports it, which is
/// the model's declaration order: this service's other enum, `SyncConfigurationType`,
/// declares one value, so no order of its own can be observed.
const ADVERTISED_ENUM_ORDERS: &[&[&str]] = &[PROVIDER_TYPE_ADVERTISED_ORDER];

/// `allowed` — the value set the walk reports, in the model's declaration order — in the
/// order this service advertises that set in.
///
/// Matched on the SET and not on the member the violation sits at, so every member of a
/// type (a `ProviderType`, a `ProviderTypeFilter`, a provider named on any operation of
/// this service) advertises one sequence, and a type whose declaration gains or loses a
/// value falls back to the declared order rather than advertising a stale list.
fn advertised_enum_values(allowed: &[String]) -> Vec<String> {
    for order in ADVERTISED_ENUM_ORDERS {
        let same_set = order.len() == allowed.len()
            && order
                .iter()
                .all(|value| allowed.iter().any(|declared| declared == value));
        if same_set {
            return order.iter().map(|value| (*value).to_string()).collect();
        }
    }
    allowed.to_vec()
}

/// The requirement a violation states, worded as this service words it. `None` for a
/// class this service has no wording for.
///
/// Every class but the two this service words differently — an enum's value set, and the
/// expression a `@pattern` advertises — states the framework's own requirement text
/// verbatim. That text belongs in `service_common` beside the `@pattern` rendering (its
/// docs call it "the requirement half", not landed yet), which is where the ~20 service
/// crates carrying this same inlined copy are to be migrated to; until then it is
/// restated here rather than derived, so this service's dialect is one readable list.
fn requirement(kind: &ViolationKind, path: &str) -> Option<String> {
    let text = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            advertised_pattern(path, pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!(
                "Member must be one of: [{}]",
                advertised_enum_values(allowed).join(", ")
            )
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(text)
}

/// One violation as a clause of the envelope, or `None` for a class this service has no
/// wording for.
fn clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind, &violation.path)?;
    // The value the caller sent is quoted for an enum's value set and nowhere else; an
    // absent member is reported as the `null` it was. See the module docs.
    let subject = match (&violation.kind, &violation.value) {
        (ViolationKind::EnumValue(_), Some(value)) => format!("Value '{value}'"),
        (ViolationKind::MissingRequiredMember, _) => "Value null".to_string(),
        _ => "Value".to_string(),
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// The `ValidationException` message for a refused request: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, in the order the
/// walk found them.
///
/// `None` declines the whole refusal — see [`clause`].
fn envelope(violations: &[Violation]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        clauses.push(clause(violation)?);
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The message a handler answers for a connection-ARN member that is not a connection
/// ARN: the same envelope the boundary refusal renders, from the same formatter, naming
/// the member at `path` and the expression it had to satisfy.
///
/// A handler reaches for this because the constraint is TIGHTER than the one the model
/// declares, so the framework's walk passes a value this service refuses (a host or
/// repository-link ARN is a well-formed generic ARN). The clause is the one the walk
/// would have produced had the model carried the service's own expression.
pub(crate) fn connection_arn_refusal(path: &str) -> String {
    let violation = Violation {
        path: path.to_string(),
        kind: ViolationKind::Pattern(CONNECTION_ARN_PATTERN.to_string()),
        value: None,
        sensitive: false,
    };
    envelope(std::slice::from_ref(&violation))
        .expect("a @pattern violation always states one clause")
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with a different code and status, and nothing about it is this service's to
/// word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(violations)?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member path is the framework's own spelling of the model's member name, not a
    /// literal that can drift from it.
    #[test]
    fn the_connection_arn_member_is_spelled_as_the_framework_spells_it() {
        let spelled = ::aws_smithy_schema_server::constraints::path::member("ConnectionArn");
        assert_eq!(
            spelled, CONNECTION_ARN_MEMBER,
            "expected the framework's spelling {spelled:?}, got {CONNECTION_ARN_MEMBER:?}"
        );
    }

    /// The advertised sequence of a value set is a PERMUTATION of the set the model
    /// declares: the order is this service's, the membership is the model's. Rows are the
    /// sets this service's enums carry, plus a set no entry claims.
    #[test]
    fn an_advertised_value_set_holds_exactly_the_values_the_model_declares() {
        let rows: [(&str, &[&str], &[&str]); 3] = [
            (
                "ProviderType, which this service advertises in an order of its own",
                crate::ops::types::PROVIDER_TYPE_VALUES,
                PROVIDER_TYPE_ADVERTISED_ORDER,
            ),
            (
                "SyncConfigurationType, which declares one value",
                crate::ops::types::SYNC_CONFIGURATION_TYPE_VALUES,
                crate::ops::types::SYNC_CONFIGURATION_TYPE_VALUES,
            ),
            (
                "a value set no advertised order claims",
                crate::ops::types::BLOCKER_STATUS_VALUES,
                crate::ops::types::BLOCKER_STATUS_VALUES,
            ),
        ];

        for (case, declared, expected) in rows {
            let declared: Vec<String> = declared.iter().map(|value| value.to_string()).collect();
            let actual = advertised_enum_values(&declared);
            let expected: Vec<String> = expected.iter().map(|value| value.to_string()).collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the advertised sequence {expected:?}, got {actual:?}"
            );

            let mut sorted_actual = actual.clone();
            sorted_actual.sort();
            let mut sorted_declared = declared.clone();
            sorted_declared.sort();
            assert_eq!(
                sorted_actual, sorted_declared,
                "case {case:?}: expected exactly the declared values {sorted_declared:?}, \
                 got {sorted_actual:?}"
            );
        }
    }

    /// An advertised sequence states the SAME members the model declares for that type: a
    /// value added to or removed from the shape leaves the sequence stale, and the clause
    /// would silently fall back to declaration order, so the membership is asserted
    /// against the model here rather than only through [`advertised_enum_values`].
    #[test]
    fn every_advertised_sequence_matches_a_value_set_the_model_declares() {
        let declared_sets: [&[&str]; 3] = [
            crate::ops::types::PROVIDER_TYPE_VALUES,
            crate::ops::types::SYNC_CONFIGURATION_TYPE_VALUES,
            crate::ops::types::BLOCKER_STATUS_VALUES,
        ];

        for order in ADVERTISED_ENUM_ORDERS {
            let mut advertised: Vec<&str> = order.to_vec();
            advertised.sort_unstable();
            let matched = declared_sets.iter().any(|declared| {
                let mut declared: Vec<&str> = declared.to_vec();
                declared.sort_unstable();
                declared == advertised
            });
            assert!(
                matched,
                "the advertised sequence {order:?} states values no enum of this model \
                 declares: the sequence is stale"
            );
        }
    }

    /// The advertised expression, as the check transcribes it, decides the cases the loose
    /// model pattern cannot: rows differ in namespace, partition, region, account,
    /// resource type and resource-id form, and each states what the expression must say
    /// about it. The end-to-end refusals are in `tests/all/validation_message_rules.rs`.
    #[test]
    fn the_connection_scoped_expression_admits_a_connection_arn_and_nothing_else() {
        let account = "123456789012";
        let rows: [(&str, String, bool); 18] = [
            (
                "a connection ARN in this service's namespace",
                format!("arn:aws:codeconnections:us-east-1:{account}:connection/abc-123"),
                true,
            ),
            (
                "a connection ARN in the legacy namespace",
                format!("arn:aws:codestar-connections:eu-west-1:{account}:connection/abc-123"),
                true,
            ),
            (
                "a connection ARN in another partition",
                format!("arn:aws-us-gov:codeconnections:us-gov-west-1:{account}:connection/c1"),
                true,
            ),
            (
                "a partition of several groups, as `aws(-[\\w]+)*` admits",
                format!("arn:aws-iso-b:codeconnections:us-isob-east-1:{account}:connection/c1"),
                true,
            ),
            (
                "a connection id spelled as a uuid",
                format!(
                    "arn:aws:codeconnections:ap-southeast-2:{account}:connection/\
                     11111111-2222-3333-4444-555555555555"
                ),
                true,
            ),
            (
                "a connection id carrying the underscore `\\w` admits",
                format!("arn:aws:codeconnections:us-east-1:{account}:connection/a_b_1"),
                true,
            ),
            ("not an ARN at all", "not-an-arn".to_string(), false),
            ("the empty value", String::new(), false),
            (
                "a host ARN of this service",
                format!("arn:aws:codeconnections:us-east-1:{account}:host/h1"),
                false,
            ),
            (
                "a repository-link ARN of this service",
                format!("arn:aws:codeconnections:us-east-1:{account}:repository-link/r1"),
                false,
            ),
            (
                "another service's connection-shaped ARN",
                format!("arn:aws:codebuild:us-east-1:{account}:connection/c1"),
                false,
            ),
            (
                "a partition that merely starts with aws",
                format!("arn:awsfoo:codeconnections:us-east-1:{account}:connection/c1"),
                false,
            ),
            (
                "a partition with an empty group",
                format!("arn:aws-:codeconnections:us-east-1:{account}:connection/c1"),
                false,
            ),
            (
                "an account id that is not 12 digits",
                "arn:aws:codeconnections:us-east-1:1234:connection/c1".to_string(),
                false,
            ),
            (
                "an empty region",
                format!("arn:aws:codeconnections::{account}:connection/c1"),
                false,
            ),
            (
                "a region outside `[a-z0-9\\-]`",
                format!("arn:aws:codeconnections:US_EAST_1:{account}:connection/c1"),
                false,
            ),
            (
                "no resource after connection/",
                format!("arn:aws:codeconnections:us-east-1:{account}:connection/"),
                false,
            ),
            (
                "a resource id carrying a character the class excludes",
                format!("arn:aws:codeconnections:us-east-1:{account}:connection/c1/sub"),
                false,
            ),
        ];
        for (case, arn, expected) in rows {
            let actual = is_connection_arn(&arn);
            assert_eq!(
                actual, expected,
                "case {case:?} ({arn:?}): expected {expected}, got {actual}"
            );
        }
    }
}

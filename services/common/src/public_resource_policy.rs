// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of whether a resource-based policy document would grant PUBLIC access:
//! access to a principal nobody named, which is what a service refuses when its API models
//! a public-policy failure and what a service reports when its API describes a policy's
//! reach.
//!
//! This is neither of the two readings beside it. The GRAMMAR of a document — whether the
//! text is a policy at all — is [`super::iam_policy`]; what a document says about ONE
//! request is [`super::iam_authorization`]. The question here is WHO a document's grants
//! reach, asked without any request: two services handed the same document must not
//! disagree about whether it is public, so the reading lives here once rather than per
//! service.
//!
//! The reading, in the order AWS states a wide-open grant:
//!
//! * only an `Allow` grants anything, so a document that merely denies opens nothing;
//! * a grant reaches everybody when its principal element names every principal — the
//!   wildcard [`ANY_PRINCIPAL`], written bare or among the identifiers of any principal
//!   type, or the negated spelling `NotPrincipal`, which names everyone OUTSIDE the set it
//!   lists;
//! * unless the statement's `Condition` binds the request to a caller something names
//!   ([`CALLER_BINDING_CONDITION_KEYS`]): a wildcard principal under such a clause reaches
//!   exactly the callers that key admits, which is the documented way to open a resource to
//!   one account or one organization without opening it to the world.
//!
//! Nothing here reads a document's SIZE or its grammar: a caller applies the bound its own
//! member states and the grammar it wants before it asks this.
//!
//! Per-service readings this one is meant to replace, and which still have to be migrated
//! to it (they predate this module, and a service crate cannot call another service
//! crate): `service-com-amazonaws-s3-amazons3`'s
//! `state::refusals::policy_grants_public_access`,
//! `service-com-amazonaws-s3control-awss3controlservicev20180820`'s
//! `state::policy_grants_public_access`,
//! `service-com-amazonaws-secretsmanager-secretsmanager`'s
//! `state::policy_grants_public_access`,
//! `service-com-amazonaws-lambda-awsgirapiservice`'s
//! `service::public_access::grants_public_access`,
//! `service-com-amazonaws-dynamodb-dynamodb-20120810`'s `state::policies::is_public` and
//! `service-com-amazonaws-accessanalyzer-accessanalyzer`'s
//! `ops::check_no_public_access::grants_public_access` — the last of which answers a
//! different question (it REPORTS which statements are public, under that operation's own
//! budget and vocabulary) and may well stay its own.

/// The one value an IAM `Principal` element may name BESIDE a principal's identifier: the
/// wildcard that stands for every principal, which the IAM policy language admits as the
/// whole element or as one of a principal type's identifiers.
pub const ANY_PRINCIPAL: &str = "*";

/// Condition keys that bind a request to a caller something NAMES — one principal, one
/// account, one organization, or one calling service.
///
/// A wildcard principal under a clause over one of these reaches exactly the callers that
/// key admits rather than everybody, so such a statement is not a public grant. The names
/// are AWS's global condition context keys for the identity making the request and for the
/// service making it on that identity's behalf (IAM User Guide, "AWS global condition
/// context keys"), compared case-insensitively as IAM compares key names.
///
/// Two families are deliberately absent. The `aws:Resource…` keys describe the account that
/// OWNS the resource rather than the caller, so a clause over one of them holds for every
/// caller and narrows nothing. The NETWORK-path keys (`aws:SourceIp`, `aws:VpcSourceIp`,
/// `aws:SourceVpc`, `aws:SourceVpce`) name where a request came FROM rather than who made
/// it: a grant conditioned on an address range is still a grant to whoever reaches it, which
/// is why a public-access analysis does not read one as limiting the grant to named
/// principals.
pub const CALLER_BINDING_CONDITION_KEYS: &[&str] = &[
    "aws:PrincipalAccount",
    "aws:PrincipalArn",
    "aws:PrincipalOrgID",
    "aws:PrincipalOrgPaths",
    "aws:PrincipalIsAWSService",
    "aws:PrincipalServiceName",
    "aws:userid",
    "aws:username",
    "aws:SourceAccount",
    "aws:SourceArn",
    "aws:SourceOrgID",
    "aws:SourceOrgPaths",
    "aws:SourceOwner",
];

/// Condition key FAMILIES that bind the caller, each written up to the separator its
/// members are named after it by (`aws:PrincipalTag/<tag-key>`), from the same
/// documentation.
pub const CALLER_BINDING_CONDITION_KEY_FAMILIES: &[&str] = &["aws:PrincipalTag/"];

/// The spelling IAM gives every NEGATED comparison (`StringNotEquals`, `ArnNotLike`,
/// `StringNotEqualsIgnoreCase`, …). A negated clause says what the request must NOT be, so
/// it admits every caller that carries no value for the key and binds nobody.
const NEGATED_OPERATOR_INFIX: &str = "Not";

/// The value under which IAM's `Null` operator — which compares a key's PRESENCE rather
/// than its value — demands that the key be carried.
const NULL_KEY_MUST_EXIST: &str = "false";

/// The wildcards the IAM policy language admits INSIDE a condition value: a value carrying
/// one admits callers nothing named, so a clause accepting it is no narrower than the
/// wildcard principal it was supposed to narrow.
const VALUE_WILDCARDS: [char; 2] = ['*', '?'];

/// How deep a principal element is read before it is taken to name nobody.
///
/// The element's nesting is the caller's — a list of lists of identifiers — so the walk is
/// bounded here rather than left to recurse on whatever arrived: the IAM grammar nests a
/// principal two levels (a type mapping to a list of identifiers), and anything past this
/// describes no principal a policy engine could resolve.
const MAX_PRINCIPAL_NESTING: usize = 8;

/// Whether `document` grants public access: see the module docs for the reading.
///
/// Text that is not JSON grants nothing — whether it is a policy at all is the grammar's
/// answer ([`super::iam_policy::resource_policy_defect`]), not this one's.
#[must_use]
pub fn grants_public_access(document: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(document)
        .is_ok_and(|parsed| grants_public_access_of(&parsed))
}

/// [`grants_public_access`] for a document that is already parsed — for a caller that
/// parsed it to read something else out of it and must not parse it twice.
#[must_use]
pub fn grants_public_access_of(document: &::serde_json::Value) -> bool {
    super::iam_policy::statements(document)
        .into_iter()
        .any(statement_grants_public_access)
}

/// Whether ONE statement grants to everybody: it allows, its principal element names every
/// principal, and no clause of its `Condition` binds the caller.
fn statement_grants_public_access(statement: &::serde_json::Value) -> bool {
    allows(statement)
        && names_everybody(statement)
        && !conditions_bind_the_caller(statement)
}

/// Whether a statement's `Effect` is the one of the two effects the language defines that
/// GRANTS. Compared as written, the same reading the grammar makes of that element.
fn allows(statement: &::serde_json::Value) -> bool {
    statement.get("Effect").and_then(::serde_json::Value::as_str)
        == Some(super::iam_policy::POLICY_EFFECTS[0])
}

/// Whether a statement's principal element reaches every principal: `Principal` naming the
/// wildcard, or a `NotPrincipal` naming anybody at all — the negated spelling grants to
/// everyone outside the set it lists, which is everybody but a few.
fn names_everybody(statement: &::serde_json::Value) -> bool {
    if let Some(principal) = statement.get("Principal") {
        return names_any_principal(principal, 0);
    }
    statement
        .get("NotPrincipal")
        .is_some_and(|excluded| names_some_principal(excluded, 0))
}

/// Whether a principal element names [`ANY_PRINCIPAL`]: as the whole element, among the
/// entries of a list, or among the identifiers a principal type maps to.
fn names_any_principal(element: &::serde_json::Value, depth: usize) -> bool {
    if depth > MAX_PRINCIPAL_NESTING {
        return false;
    }
    match element {
        ::serde_json::Value::String(value) => value == ANY_PRINCIPAL,
        ::serde_json::Value::Array(entries) => entries
            .iter()
            .any(|entry| names_any_principal(entry, depth + 1)),
        ::serde_json::Value::Object(types) => types
            .values()
            .any(|identifiers| names_any_principal(identifiers, depth + 1)),
        _ => false,
    }
}

/// Whether a principal element names at least one principal, whichever it is.
fn names_some_principal(element: &::serde_json::Value, depth: usize) -> bool {
    if depth > MAX_PRINCIPAL_NESTING {
        return false;
    }
    match element {
        ::serde_json::Value::String(value) => !value.is_empty(),
        ::serde_json::Value::Array(entries) => entries
            .iter()
            .any(|entry| names_some_principal(entry, depth + 1)),
        ::serde_json::Value::Object(types) => types
            .values()
            .any(|identifiers| names_some_principal(identifiers, depth + 1)),
        _ => false,
    }
}

/// Whether a statement's `Condition` binds the request to a caller something names.
///
/// A `Condition` is a conjunction, so ONE clause that binds the caller is enough to keep the
/// statement from reaching everybody.
fn conditions_bind_the_caller(statement: &::serde_json::Value) -> bool {
    let Some(::serde_json::Value::Object(clauses)) = statement.get("Condition") else {
        return false;
    };
    clauses.iter().any(|(operator, keys)| {
        keys.as_object().is_some_and(|keys| {
            keys.iter()
                .any(|(key, values)| clause_binds_the_caller(operator, key, values))
        })
    })
}

/// Whether ONE condition clause binds the caller: its key is one of the caller-binding keys,
/// its operator is a POSITIVE comparison, and every value it accepts names something
/// concrete.
///
/// The clause's grammar — the set qualifier before the operator, the `IfExists` suffix after
/// it, the `Null` operator and the values a clause states — is read through the one
/// evaluation of a condition clause in this crate ([`super::iam_authorization`]), so the two
/// readers of a `Condition` cannot come to disagree about how one is written.
fn clause_binds_the_caller(
    operator: &str,
    key: &str,
    values: &::serde_json::Value,
) -> bool {
    if !binds_the_caller(key) {
        return false;
    }
    let (_, comparison) = super::iam_authorization::split_set_qualifier(operator);
    let comparison = comparison
        .strip_suffix(super::iam_authorization::IF_EXISTS_SUFFIX)
        .unwrap_or(comparison);
    if comparison.contains(NEGATED_OPERATOR_INFIX) {
        return false;
    }
    let accepted = super::iam_authorization::clause_values(values);
    if comparison == super::iam_authorization::NULL_OPERATOR {
        // Presence rather than value: demanding the key be CARRIED binds the request to a
        // caller that carries it, while demanding its absence binds nobody.
        return accepted
            .iter()
            .any(|value| value.eq_ignore_ascii_case(NULL_KEY_MUST_EXIST));
    }
    !accepted.is_empty()
        && accepted
            .iter()
            .all(|value| !value.contains(VALUE_WILDCARDS))
}

/// Whether a condition key names something about the CALLER: one of
/// [`CALLER_BINDING_CONDITION_KEYS`], or a member of one of
/// [`CALLER_BINDING_CONDITION_KEY_FAMILIES`]. Key names are compared
/// case-insensitively, as IAM compares them.
fn binds_the_caller(key: &str) -> bool {
    CALLER_BINDING_CONDITION_KEYS
        .iter()
        .any(|known| key.eq_ignore_ascii_case(known))
        || CALLER_BINDING_CONDITION_KEY_FAMILIES.iter().any(|family| {
            // `get` rather than a slice: the key is a caller's text, so a prefix length
            // that falls inside a multi-byte character answers `None` instead of
            // panicking.
            key.len() > family.len()
                && key
                    .get(..family.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(family))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-statement document carrying `statement`, as a client would send it.
    fn policy(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    #[test]
    fn the_reading_holds_across_the_forms_a_grant_takes() {
        // The rows differ along what the reading generalises over: the effect, the form of
        // the principal element (a bare wildcard, a type mapping to one identifier or to a
        // list, the negated spelling), how many statements the document carries, and
        // whether a condition binds the caller.
        let rows: [(&str, String, bool); 18] = [
            (
                "a bare wildcard principal",
                policy("{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\"}"),
                true,
            ),
            (
                "the wildcard under a principal type",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":{\"AWS\":\"*\"}}",
                ),
                true,
            ),
            (
                "the wildcard among a type's identifiers",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\",\"*\"]}}",
                ),
                true,
            ),
            (
                "one named account",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
                ),
                false,
            ),
            (
                "a service principal",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":{\"Service\":\"events.amazonaws.com\"}}",
                ),
                false,
            ),
            (
                "a wildcard principal that only DENIES",
                policy("{\"Effect\":\"Deny\",\"Action\":\"svc:A\",\"Principal\":\"*\"}"),
                false,
            ),
            (
                "everyone outside a named set",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"NotPrincipal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
                ),
                true,
            ),
            (
                "a public statement beside a narrow one",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}},\
                     {\"Effect\":\"Allow\",\"Action\":\"svc:B\",\"Principal\":\"*\"}",
                ),
                true,
            ),
            (
                "narrowed to one organization",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-abc123\"}}}",
                ),
                false,
            ),
            (
                "narrowed to one account, named in another case",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringEquals\":\
                     {\"AWS:PRINCIPALACCOUNT\":\"000000000000\"}}}",
                ),
                false,
            ),
            (
                "narrowed by one principal tag",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalTag/team\":\"red\"}}}",
                ),
                false,
            ),
            (
                "narrowed under a set qualifier",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"ForAnyValue:StringEquals\":\
                     {\"aws:PrincipalAccount\":[\"000000000000\",\"000000000001\"]}}}",
                ),
                false,
            ),
            (
                "a NEGATED comparison, which binds nobody",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringNotEquals\":\
                     {\"aws:PrincipalAccount\":\"000000000000\"}}}",
                ),
                true,
            ),
            (
                "a caller key accepted with a wildcard value",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringLike\":{\"aws:PrincipalArn\":\"*\"}}}",
                ),
                true,
            ),
            (
                "a condition over something that is not the caller",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:ResourceAccount\":\"000000000000\"}}}",
                ),
                true,
            ),
            (
                "a condition over one address range, which names no caller",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"IpAddress\":{\"aws:SourceIp\":\"198.51.100.0/24\"}}}",
                ),
                true,
            ),
            (
                "a condition over every address",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Condition\":{\"IpAddress\":{\"aws:SourceIp\":\"0.0.0.0/0\"}}}",
                ),
                true,
            ),
            ("text that is not a policy at all", "not json".to_string(), false),
        ];

        for (case, document, expected) in rows {
            let actual = grants_public_access(&document);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected grants_public_access {expected}, got {actual} \
                 for {document}"
            );
        }
    }

    #[test]
    fn the_null_operator_binds_only_when_it_demands_the_key() {
        let rows: [(&str, &str, bool); 2] = [
            ("the key must be carried", "false", false),
            ("the key must be absent", "true", true),
        ];
        for (case, demand, expected) in rows {
            let document = policy(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                 \"Condition\":{{\"Null\":{{\"aws:PrincipalArn\":\"{demand}\"}}}}}}"
            ));
            let actual = grants_public_access(&document);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected grants_public_access {expected}, got {actual} \
                 for {document}"
            );
        }
    }

    #[test]
    fn a_condition_key_in_any_text_is_read_without_panicking() {
        // The key is a caller's text, so it is read for what it names rather than sliced:
        // a key whose bytes end inside a multi-byte character, one longer than every
        // family, and one that is empty all have to answer.
        let keys = [
            "aws:PrincipalTag/équipe",
            "aws:PrincipalTa\u{00e9}",
            "aws:PrincipalTag/",
            "\u{00ff}",
            "",
        ];
        for key in keys {
            let document = policy(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                 \"Condition\":{{\"StringEquals\":{{{}:\"red\"}}}}}}",
                ::serde_json::Value::String(key.to_string())
            ));
            // Only the first key names a member of a caller-binding family, so only it
            // narrows the wildcard principal.
            let expected = key != "aws:PrincipalTag/équipe";
            let actual = grants_public_access(&document);
            assert_eq!(
                actual, expected,
                "case {key:?}: expected grants_public_access {expected}, got {actual} \
                 for {document}"
            );
        }
    }
}

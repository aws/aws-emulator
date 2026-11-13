// SPDX-License-Identifier: CC0-1.0

//! How IAM WRITES a policy document into a response, for every member that carries one.
//!
//! IAM holds a policy document as the text it was given (`super::trust_policy` for the one
//! spelling a save does change) and REPORTS it percent-encoded: "the policy document returned
//! in this structure is URL-encoded compliant with RFC 3986. You can use a URL decoding method
//! to convert the policy back to plain JSON text" (`PolicyVersion$Document`), stated again by
//! every operation that serves one — `GetRole`, `GetRolePolicy`, `GetUserPolicy`,
//! `GetGroupPolicy`, `GetPolicyVersion` and `GetAccountAuthorizationDetails` each document that
//! "policies returned by this operation are URL-encoded compliant with RFC 3986".
//!
//! The rule is about the MEMBER, not about any one operation: every response member whose target
//! is `policyDocumentType` carries the encoded form, so a caller that URL-decodes what it read —
//! which is what the contract tells it to do — gets back the document that was stored, byte for
//! byte, whether it read the role through `GetRole`, through a page of `ListRoles`, or embedded
//! in an instance profile the instance-profile listings serve. That is also why the transform
//! lives here rather than in each handler: one encode step, applied by every reader, so a
//! document cannot be reported in one form on one read and another form on the next.
//!
//! Only the REPORT is encoded. What the account holds stays the plain text every other reader of
//! the store — this service's own authorization pass, and the services that resolve a role
//! against IAM — evaluates, so the encoding is a property of the wire and of nothing else.
//!
//! A role TEMPLATE's `…Template` member is not one of these: it carries the pattern a role is
//! created from rather than a policy in force on an entity, and AWS reports it as the text it
//! published, so [`super::role_templates`] writes those members as they stand.

use crate::ops::types::{InstanceProfile, PolicyVersion, Role};

/// The characters RFC 3986 leaves alone: "unreserved characters" are the ASCII letters and
/// digits plus `-`, `.`, `_` and `~`. Everything else a policy document carries is written as
/// the `%XX` of each of its UTF-8 bytes — `{` as `%7B`, `"` as `%22`, `:` as `%3A`, `,` as
/// `%2C`, `[` as `%5B`, `]` as `%5D`, `*` as `%2A`, a space as `%20`, and the tab, line feed and
/// carriage return `policyDocumentType` admits as `%09`, `%0A` and `%0D`.
const UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The stored `document` as a response reports it: percent-encoded, preserving every byte of the
/// text — the whitespace a caller indented it with included, since those bytes are part of the
/// document and are simply encoded like any other.
///
/// Decoding the result returns the input, so a caller that follows the documented contract reads
/// back exactly what was saved; a document carrying a literal `%` or `+` survives that decode
/// here, where it would be corrupted if the raw text were reported.
#[must_use]
pub fn reported_policy_document(document: &str) -> String {
    ::percent_encoding::utf8_percent_encode(document, UNRESERVED).to_string()
}

/// One role as a response reports it: its trust policy
/// ([`reported_policy_document`]) and every other member as the account holds it.
///
/// `Role$AssumeRolePolicyDocument` targets `policyDocumentType`, so it is encoded wherever a
/// `Role` is written out — a role read on its own, a page of roles, and the roles embedded in an
/// instance profile.
#[must_use]
pub fn reported_role(role: Role) -> Role {
    Role {
        assume_role_policy_document: role
            .assume_role_policy_document
            .as_deref()
            .map(reported_policy_document),
        ..role
    }
}

/// One instance profile as a response reports it: each role it contains reported as
/// [`reported_role`] reports one, since `InstanceProfile$Roles` carries the ROLE and therefore
/// that role's trust policy.
#[must_use]
pub fn reported_instance_profile(instance_profile: InstanceProfile) -> InstanceProfile {
    InstanceProfile {
        roles: instance_profile
            .roles
            .into_iter()
            .map(reported_role)
            .collect(),
        ..instance_profile
    }
}

/// One managed-policy version as a response reports it: its `Document`
/// ([`reported_policy_document`]) when the reader carries one at all, and its metadata as it
/// stands.
#[must_use]
pub fn reported_policy_version(version: PolicyVersion) -> PolicyVersion {
    PolicyVersion {
        document: version.document.as_deref().map(reported_policy_document),
        ..version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a reported document is the stored text percent-encoded, and percent-decoding it
    /// returns that text byte for byte.
    #[test]
    fn a_reported_document_is_the_stored_text_percent_encoded() {
        // The rows differ along what the rule generalises over: which characters the document
        // carries (the punctuation JSON is made of, the whitespace a caller indents with, the
        // two characters a decode of an UNencoded document corrupts, the unreserved set, and a
        // byte above ASCII, which `policyDocumentType` admits up to U+00FF).
        let rows: [(&str, &str, &str); 8] = [
            (
                "a compact trust policy",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
                "%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D",
            ),
            (
                "the spaces a caller wrote between the tokens",
                r#"{"Version": "2012-10-17"}"#,
                "%7B%22Version%22%3A%20%222012-10-17%22%7D",
            ),
            (
                "a wildcard resource",
                r#"{"Resource":"*"}"#,
                "%7B%22Resource%22%3A%22%2A%22%7D",
            ),
            (
                "a resource ARN, whose slashes are not unreserved",
                r#"{"Resource":"arn:aws:s3:::bucket/key"}"#,
                "%7B%22Resource%22%3A%22arn%3Aaws%3As3%3A%3A%3Abucket%2Fkey%22%7D",
            ),
            (
                "a literal percent, which a decode of the raw text would misread",
                r#"{"Sid":"100%"}"#,
                "%7B%22Sid%22%3A%22100%25%22%7D",
            ),
            (
                "a literal plus, which a decode of the raw text would turn into a space",
                r#"{"Sid":"a+b"}"#,
                "%7B%22Sid%22%3A%22a%2Bb%22%7D",
            ),
            (
                "the unreserved characters, which are reported as they stand",
                "aZ09-._~",
                "aZ09-._~",
            ),
            (
                "the tab, line feed and carriage return the member admits",
                "{\t\n\r}",
                "%7B%09%0A%0D%7D",
            ),
        ];

        for (case, stored, encoded) in rows {
            let actual = reported_policy_document(stored);
            assert_eq!(
                actual, encoded,
                "case {case:?}: expected {encoded:?}, got {actual:?}"
            );
            let decoded = ::smithy_server_runtime::percent_decode(&actual);
            assert_eq!(
                decoded, stored,
                "case {case:?}: decoding the report must return the stored text, got {decoded:?}"
            );
        }
    }

    /// RULE: a byte above ASCII is encoded as the `%XX` of each of its UTF-8 bytes, so the
    /// decode still returns the document.
    #[test]
    fn a_document_carrying_a_non_ascii_character_round_trips() {
        let stored = "{\"Sid\":\"caf\u{e9}\"}";
        let actual = reported_policy_document(stored);
        assert_eq!(
            actual, "%7B%22Sid%22%3A%22caf%C3%A9%22%7D",
            "expected the UTF-8 bytes to be encoded, got {actual:?}"
        );
        let decoded = ::smithy_server_runtime::percent_decode(&actual);
        assert_eq!(
            decoded, stored,
            "decoding the report must return the stored text, got {decoded:?}"
        );
    }
}

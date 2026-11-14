// SPDX-License-Identifier: CC0-1.0

//! The one transcoding CloudFormation applies to the FREE TEXT a caller hands it to store:
//! the text is held as ASCII, so a codepoint outside ASCII reads back as one `?`.
//!
//! The stores in this service that hold caller-authored text — a stack's (change set's,
//! stack set's) `Tags` and an extension's type configuration — report such a codepoint back as
//! a single `?`, one question mark per codepoint, whatever its UTF-8 encoding takes, and so
//! does the TEMPLATE TEXT a read of a stored template serves ([`stored_template_body`]).
//! Nothing else about the text changes: its length in codepoints, its ordering and every ASCII
//! character in it survive, so a document's structure and a tag's shape are untouched and only
//! the characters the store cannot hold differ.
//!
//! Where it is applied differs between the two KINDS of text, because what each of them is
//! for differs:
//!
//! * a `Tags` list and a type configuration are text the store holds and nothing else reads,
//!   so the coercion is applied at the WRITE and every read reports what the store holds;
//! * a TEMPLATE is both: text a client reads back AND the document a deployment walks. Its
//!   resource properties travel on to the service that owns each resource as the request
//!   declared them (`core/router/tests/all/cloudformation_provisions_cloudwatch_dashboards.rs`
//!   holds a declared non-ASCII dashboard body to reaching Amazon CloudWatch as written, and
//!   `…/cloudformation_condition_gated_secrets_policy.rs` a secret's value to reaching AWS
//!   Secrets Manager as written), so the coercion is applied where the stored template's text
//!   is SERVED ([`stored_template_body`]) and the document the walk reads is left as declared.
//!
//! That is one divergence, and it is part of the rule rather than a gap in it: the template
//! text a client can OBSERVE — every read of a stored template, and the comparison that
//! decides whether an `UpdateStack` changes the stack at all — is the ASCII one, so a client
//! that resubmits what `GetTemplate` answered is told "No updates are to be performed." exactly
//! as CloudFormation tells it ([`stored_template_body`] names each site).

/// `text` as the store holds it: every codepoint outside ASCII replaced by one `?`.
///
/// Per CODEPOINT, not per byte: a two-, three- or four-byte encoding is one `?`, which is
/// what keeps the length in characters (the unit the `@length` constraints count in) the
/// same as the text submitted.
pub fn ascii_text(text: &str) -> String {
    text.chars()
        .map(|character| if character.is_ascii() { character } else { '?' })
        .collect()
}

/// Whether two texts are the SAME text to this store: equal once each is held as ASCII.
///
/// Compared CODEPOINT BY CODEPOINT, so neither side is copied to be compared — which matters
/// where the question is asked on a request path over caller-sized documents
/// ([`stored_template_body`] names the one such reader).
pub fn ascii_text_eq(left: &str, right: &str) -> bool {
    fn held(character: char) -> char {
        if character.is_ascii() {
            character
        } else {
            '?'
        }
    }
    left.chars().map(held).eq(right.chars().map(held))
}

/// The TEMPLATE TEXT a read of a stored template reports: [`ascii_text`] of the document the
/// caller submitted.
///
/// CloudFormation holds the template a caller handed it as ASCII, so the text it hands BACK
/// carries one `?` for every codepoint outside ASCII, whatever its UTF-8 encoding takes. One
/// question mark per codepoint, so the document's JSON (or YAML) structure, its ASCII
/// characters and its length in characters are untouched: a body that parsed before parses
/// after, declaring the same resources under the same logical ids.
///
/// Every site the template text a client can observe passes through, which is the whole of
/// what this rule governs:
///
/// * `GetTemplate`'s `TemplateBody`, for a stack at either stage and for a change set;
/// * `GetTemplateSummary` over a STORED template (a stack's or a stack set's), whose
///   `Description` and `Metadata` are that text;
/// * a stack set's `TemplateBody`, as `DescribeStackSet` reports it;
/// * the `Description` a stack reports out of its template
///   ([`super::StoredStack::description`], which both `DescribeStacks` and `ListStacks` read);
/// * the comparison `UpdateStack` makes to decide whether a submitted body changes the stack
///   at all, so a body differing from the stored one only in codepoints the store cannot hold
///   is the no-op CloudFormation refuses ([`ascii_text_eq`], which compares without copying
///   either side).
///
/// What it is NOT applied to is the document a deployment walks: see this module's header for
/// why, and for the two cross-service suites that pin it. Nor is it applied to the body a
/// REQUEST carries for an operation that stores nothing of it — `ValidateTemplate`, and
/// `GetTemplateSummary` over a submitted `TemplateBody` — which report on a document this
/// service never held; whether CloudFormation folds those answers too is a question no
/// recording here answers, so they are deliberately out of this rule's scope.
pub fn stored_template_body(submitted: &str) -> String {
    ascii_text(submitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_template_body_is_the_submitted_one_held_as_ascii() {
        // The template the recorded round trip submits is JSON whose free text carries
        // codepoints of every width; the structure around them is ASCII and survives.
        let submitted = "{\"Description\":\"caf\u{e9} \u{65e5}\u{672c} \u{1f680}\",\
             \"Resources\":{\"Q\":{\"Type\":\"AWS::SQS::Queue\"}}}";
        let served = stored_template_body(submitted);
        assert!(
            served.is_ascii(),
            "expected the text a read serves to be ASCII, got {served:?}"
        );
        assert_eq!(
            served.chars().count(),
            submitted.chars().count(),
            "expected the length in codepoints kept, got {served:?}"
        );
        assert!(
            served.contains(r#""Resources":{"Q":{"Type":"AWS::SQS::Queue"}}"#),
            "expected the document's own structure untouched, got {served:?}"
        );
        assert!(
            served.contains(r#""Description":"caf? ?? ?""#),
            "expected one question mark per codepoint past ASCII, got {served:?}"
        );
    }

    #[test]
    fn two_texts_are_the_same_text_once_each_is_held_as_ascii() {
        // Each row: the two texts, and whether the store holds them as one text.
        for (case, left, right, same) in [
            ("identical ASCII", "deploy/v1", "deploy/v1", true),
            ("the same text past ASCII", "caf\u{e9}", "caf\u{e9}", true),
            ("the served form against the submitted one", "caf?", "caf\u{e9}", true),
            ("two different codepoints past ASCII", "caf\u{e9}", "caf\u{fc}", true),
            ("a question mark against an ASCII letter", "caf?", "cafe", false),
            ("different lengths", "caf\u{e9}", "caf\u{e9}s", false),
            ("the empty text against a non-empty one", "", "?", false),
        ] {
            let actual = ascii_text_eq(left, right);
            assert_eq!(
                actual, same,
                "case {case:?}: expected ({left:?}, {right:?}) to be held as one text: \
                 {same}, got {actual}"
            );
            assert_eq!(
                actual,
                ascii_text(left) == ascii_text(right),
                "case {case:?}: expected the same answer as comparing the held texts"
            );
        }
    }

    #[test]
    fn ascii_text_survives_and_everything_past_it_is_one_question_mark() {
        // Each row: the text submitted, and the text the store holds for it.
        for (case, submitted, held) in [
            ("wholly ASCII", "Owner:team-1/build+v1.0", "Owner:team-1/build+v1.0"),
            ("the first ASCII codepoint", "a\u{0}b", "a\u{0}b"),
            ("the last ASCII codepoint", "a\u{7f}b", "a\u{7f}b"),
            ("the first codepoint past ASCII", "caf\u{80}", "caf?"),
            ("a Latin-1 letter", "Bj\u{f6}rk", "Bj?rk"),
            ("a two-byte encoding", "\u{41f}\u{440}\u{438}", "???"),
            ("a three-byte encoding", "\u{65e5}\u{672c}\u{8a9e}", "???"),
            ("a four-byte encoding, one codepoint", "\u{1f642}", "?"),
            ("the empty text", "", ""),
        ] {
            let actual = ascii_text(submitted);
            assert_eq!(
                actual, held,
                "case {case:?}: expected {held:?}, got {actual:?}"
            );
            assert_eq!(
                actual.chars().count(),
                submitted.chars().count(),
                "case {case:?}: expected the length in codepoints kept, got {actual:?}"
            );
        }
    }
}

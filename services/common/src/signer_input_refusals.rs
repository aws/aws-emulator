// SPDX-License-Identifier: CC0-1.0

//! How AWS Signer states a refusal of an input member a request supplied NOTHING for.
//!
//! # Why it is here
//!
//! AWS Signer is emulated as TWO service crates — the control plane
//! (`service-com-amazonaws-signer-wallabyservice`) and the data plane
//! (`service-com-amazonaws-signerdata-signerdataplane`) — and both of them model
//! `GetRevocationStatus`, which is one AWS endpoint with one set of refusals. A sentence
//! written out in each crate is a sentence that can come to differ between two views of
//! the same API, so the two the recordings pin are stated here once and re-exported by
//! each crate's `service.rs`.
//!
//! # The two sentences
//!
//! Both are recorded behaviour: the model declares no wording, and the framework's own
//! constraint clause (`Value null at '<member>' failed to satisfy constraint: Member must
//! not be null`) is not what this service answers for an absent member. Signer's own
//! validator states the member and what is wrong with it, and it distinguishes the two
//! cases the wire cannot tell apart from the ones it can:
//!
//! * a SINGLE-VALUE member, where absence really is a null value
//!   ([`absent_member_constraint`]);
//! * a member the model binds a LIST to on the query string, where an absent parameter and
//!   an empty one are the SAME request — an SDK serializing an empty list emits no
//!   parameter at all — so the refusal cannot claim the value was null and states instead
//!   that the member must carry something ([`missing_list_member_constraint`]).
//!
//! In both, the member's wire name (as `@httpQuery` binds it) is the one variable part and
//! the rest is fixed text.

/// The constraint a required SINGLE-VALUE member violated when the request supplied
/// nothing for it, as a sentence naming the member.
///
/// The `Invalid <member>: <what is wrong>` voice is the one AWS Signer refuses its own
/// rules in — a value that is not an ARN of the resource its member names, a signature
/// timestamp outside the window the service accepts — so an absent member reads as one
/// more of them rather than as a schema fact.
#[must_use]
pub fn absent_member_constraint(member_name: &str) -> String {
    format!("Invalid {member_name}: value must not be null")
}

/// The constraint a required LIST member violated when the request supplied nothing for
/// it, as a sentence naming the member.
///
/// Stated for BOTH forms of that request, the absent parameter and the empty list, because
/// on the query string they are the same bytes. Shared with the handlers that find such a
/// list empty, so a member the boundary validator refuses and one a handler refuses are
/// stated identically.
#[must_use]
pub fn missing_list_member_constraint(member_name: &str) -> String {
    format!("{member_name} must not be null or empty")
}

#[cfg(test)]
mod tests {
    use super::{absent_member_constraint, missing_list_member_constraint};

    /// Each sentence names the member it is about and is otherwise fixed. The rows differ
    /// along the member name, including the two the recordings pin and one neither cited.
    #[test]
    fn each_sentence_names_the_member_and_is_otherwise_fixed() {
        for member in ["signatureTimestamp", "certificateHashes", "revisionId"] {
            assert_eq!(
                absent_member_constraint(member),
                format!("Invalid {member}: value must not be null"),
                "case {member}: the absent-member sentence names the member"
            );
            assert_eq!(
                missing_list_member_constraint(member),
                format!("{member} must not be null or empty"),
                "case {member}: the list sentence names the member"
            );
        }
    }
}

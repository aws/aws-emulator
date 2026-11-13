// SPDX-License-Identifier: CC0-1.0

//! The REFERENCE FORM a `CustomID` member has to be written in, and the
//! request-validation refusal a value outside it is answered with.
//!
//! Every identifier this service takes from a caller to NAME an existing
//! resource — an asset, an asset model, a hierarchy, a property — targets
//! `CustomID`: `@length 13..139` plus a `@pattern` admitting exactly two
//! spellings, a non-nil lowercase-hexadecimal UUID or `externalId:` followed by
//! an external id (`smithy_query(query="shape", shape="CustomID")`). The value's
//! SYNTAX is therefore decided by the request validator, ahead of every lookup:
//! a value that is not a reference at all names no resource in any account, so
//! the answer is "this request is malformed" rather than "that resource is
//! missing".
//!
//! The framework's constraint walk cannot state that half of the rule. Its
//! `@pattern` check compiles the expression with the `regex` crate, whose syntax
//! has no negative lookahead — the one `CustomID` opens with, to exclude the nil
//! UUID — and an expression it cannot compile is treated as matching. So the
//! length bound is enforced at the boundary and the reference form is not,
//! which is why the operations that resolve such a label ask [`custom_id_clause`]
//! for themselves before they look anything up.
//!
//! The refusal is the framework's own request-validation report, assembled
//! through `service_common::validation_message` so this service's clauses and
//! envelope are the ones every other member's refusal carries: the clause names
//! the member and the expression it had to satisfy, and states no value (the
//! dialect IoT SiteWise refuses in).
//!
//! The STRUCTURAL fix is at the boundary rather than here: the walk fails OPEN
//! for every member whose `@pattern` it cannot compile, so a rule applied from
//! the model would cover every `CustomID` member of this service at once instead
//! of being asked for per operation. That belongs to the open issue
//! `20261103T025010Z-iotsitewise-validation-messages-quote-the-rejected-value-aws-omits.md`,
//! whose own recorded refusals require the boundary to state this very clause
//! beside the length clause of one member; the operations below ask for it
//! themselves until that lands.

use ::aws_smithy_schema_server::{Violation, ViolationKind};
use ::service_common::validation_message::{envelope_over, valueless_clause};

use super::EXTERNAL_ID_PREFIX;

/// The `@pattern` the model declares on `CustomID`, verbatim
/// (`smithy_query(query="shape", shape="CustomID")`).
///
/// Transcribed rather than read off the schema at runtime because the boundary
/// walk cannot apply it (see the module docs), and the CHECK below reads the two
/// branches it spells; this constant is what a refusal QUOTES.
pub const CUSTOM_ID_PATTERN: &str = r"^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+$";

/// The group lengths of the modelled `ID` form, as the model fixes it:
/// `@length 36..36` and five groups of lowercase hexadecimal digits separated by
/// hyphens (`smithy_query(query="shape", shape="ID")`).
const ID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The one UUID the `ID` and `CustomID` patterns exclude with their leading
/// negative lookahead, so it is no resource's id and no id spelling either.
const NIL_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Whether `label` is written in the modelled `ID` form: five lowercase
/// hexadecimal groups of [`ID_GROUP_LENGTHS`], and not the [`NIL_ID`] the
/// pattern's lookahead excludes.
#[must_use]
pub fn names_modelled_id(label: &str) -> bool {
    if label == NIL_ID {
        return false;
    }
    let mut groups = label.split('-');
    let shaped = ID_GROUP_LENGTHS.iter().all(|length| {
        groups.next().is_some_and(|group| {
            group.len() == *length
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    });
    shaped && groups.next().is_none()
}

/// Whether `label` is written in one of the two forms the `CustomID`
/// `@pattern` admits: the `ID` form ([`names_modelled_id`]), or
/// [`EXTERNAL_ID_PREFIX`] followed by an external id the pattern's second branch
/// admits.
///
/// The LENGTH half of the member's constraints (13..139) is not asked here: the
/// boundary walk enforces it, so a label that reaches a handler already satisfies
/// it.
#[must_use]
pub fn names_custom_id(label: &str) -> bool {
    match label.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external_id) => names_external_id(external_id),
        None => names_modelled_id(label),
    }
}

/// Whether `external_id` is what the pattern's `externalId:` branch admits:
/// `[a-zA-Z0-9_]` first, `[a-zA-Z0-9_]` last, and `[a-zA-Z_\-0-9.:]` throughout
/// — so at least two characters, neither end a separator.
fn names_external_id(external_id: &str) -> bool {
    let mut characters = external_id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let Some(last) = characters.next_back() else {
        return false;
    };
    word_character(first)
        && word_character(last)
        && external_id.chars().all(interior_character)
}

/// `[a-zA-Z0-9_]`: what the external-id branch admits at either end.
fn word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// `[a-zA-Z_\-0-9.:]`: what it admits anywhere.
fn interior_character(character: char) -> bool {
    word_character(character) || matches!(character, '-' | '.' | ':')
}

/// The request-validation CLAUSE a `CustomID` member outside its reference form
/// is refused with, or `None` for a label in one of the two admitted forms.
///
/// `member_path` is the member as the framework's own walk spells it, so a clause
/// this states and one the boundary states name a member the same way:
/// `assetModelId` for a top-level member, and
/// `assetModelHierarchies.1.member.childAssetModelId` for a member of a list's
/// element ([`element_member_path`]).
///
/// ONE wording for every operation that resolves such a label; the whole report
/// it goes into is [`custom_id_report`]'s.
#[must_use]
pub fn custom_id_clause(member_path: &str, label: &str) -> Option<String> {
    if names_custom_id(label) {
        return None;
    }
    let violation = Violation {
        path: member_path.to_string(),
        // A `@pattern` is a FIND rather than a whole-value match, and this
        // service's refusals quote the expression with the anchor that CLOSES
        // the match dropped while every anchor the expression writes around its
        // own branches stays as declared. That rendering is one of the dialects
        // `service_common::validation` owns, the shared home of this family, so
        // the rule is not spelled a second time here.
        kind: ViolationKind::Pattern(
            ::service_common::validation::unanchored_at_the_close(CUSTOM_ID_PATTERN),
        ),
        // The clause states no value; the echo is what `valueless_clause` strips.
        value: Some(label.to_string()),
        sensitive: false,
    };
    valueless_clause(&violation)
}

/// The request-validation report EVERY `CustomID` member of one request that is
/// outside its reference form is refused with, or `None` when all of them are
/// references.
///
/// Over the members TOGETHER, not one at a time, because that is what the
/// framework's own walk does: it collects the violations of the whole request and
/// states them in one report (`N validation errors detected: …`,
/// `service_common::validation_message::envelope_over`), so which member a
/// request broke first cannot change the answer. A caller that resolves these
/// references then asks this ONCE, before it resolves any of them, so a request
/// carrying both a malformed reference and a well-formed one that names nothing
/// is answered the same way whichever order they arrive in.
///
/// Each member is given as `(the member path, the label the request carried)`,
/// and the clauses are stated in the order the members are given — the order the
/// walk itself reports them in, which for a list is its element order. The path
/// of a member of a list ELEMENT is spelled by
/// `service_common::validation::element_member_path`, the shared home of that
/// plumbing, which each caller asks directly.
#[must_use]
pub fn custom_id_report<'m, M>(members: M) -> Option<String>
where
    M: IntoIterator<Item = (String, &'m str)>,
{
    let clauses: Vec<String> = members
        .into_iter()
        .filter_map(|(member_path, label)| custom_id_clause(&member_path, label))
        .collect();
    if clauses.is_empty() {
        return None;
    }
    envelope_over(&clauses)
}


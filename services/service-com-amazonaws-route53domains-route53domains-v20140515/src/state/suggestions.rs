// SPDX-License-Identifier: CC0-1.0

//! Where a suggested domain name comes from: the names this registrar offers a caller who
//! asked for alternatives to one name.
//!
//! `GetDomainSuggestions` "returns a list of suggested domain names", and the member a
//! caller submits is "[a] domain name that you want to use as the basis for a list of
//! possible domain names". So a suggestion is DERIVED from the requested name, and the two
//! derivations a registrar offers are the two this module computes:
//!
//! * the requested second-level label under ANOTHER top-level domain this registrar
//!   offers — the same name somewhere else;
//! * the requested label with a short affix, under the requested top-level domain and then
//!   under the others — a near name under the suffix the caller asked for.
//!
//! Three properties hold of every name this produces, because each is something a caller
//! can act on:
//!
//! * it is a name a registry would accept — every candidate goes through
//!   [`crate::state::resolve_domain_name`], so a suggestion is always a name the caller can
//!   turn round and pass to `CheckDomainAvailability` or `RegisterDomain`;
//! * it is never the requested name itself, and never a repeat of an earlier suggestion;
//! * the list is the same on every call for one requested name, because it is computed from
//!   that name and from fixed tables rather than from anything that varies per request.
//!
//! Whether a name is AVAILABLE, and what it costs, is not decided here: that is the
//! registry's answer ([`crate::state::ComAmazonawsRoute53domainsRoute53domainsV20140515State::is_registered`]
//! and [`crate::state::standard_registration_price`]), so a suggestion's `Availability`
//! cannot disagree with `CheckDomainAvailability`'s for the same name.

use crate::state::{request_problem, resolve_domain_name, STANDARD_REGISTRATION_RATES};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// How many suggestions one call may ask for
// ---------------------------------------------------------------------------

/// The fewest suggestions a request may ask for, as a count of names.
/// `SuggestionCount`'s own documentation: "Specify a value between 1 and 50."
pub const MIN_SUGGESTION_COUNT: usize = 1;
/// The most suggestions a request may ask for — the other end of the range
/// `SuggestionCount` documents (see [`MIN_SUGGESTION_COUNT`]).
pub const MAX_SUGGESTION_COUNT: usize = 50;

/// How many names the request asked for, or `None` when `SuggestionCount` is outside the
/// range the member documents.
///
/// A negative value is outside it by the same rule: the member is a number of names, so a
/// value below the documented minimum is no count at all, whichever side of zero it sits.
pub fn wanted_suggestions(requested: i32) -> Option<usize> {
    usize::try_from(requested)
        .ok()
        .filter(|count| (MIN_SUGGESTION_COUNT..=MAX_SUGGESTION_COUNT).contains(count))
}

/// The message the `InvalidInput` refusal of a `SuggestionCount` outside the documented
/// range carries.
///
/// The model declares no `@range` on the member, so the rule is the one the member's
/// documentation states — "Specify a value between 1 and 50" — and this service enforces
/// it as a problem with the REQUEST, stated in the envelope it words those in
/// ([`request_problem`]). The refusal states that rule and nothing about the particular
/// value: it names the count in prose and the two documented bounds, and does not echo
/// what the request carried, so one submitted value cannot reach the message.
pub fn suggestion_count_message() -> String {
    request_problem(&format!(
        "Suggestion count must be a value between {MIN_SUGGESTION_COUNT} and \
         {MAX_SUGGESTION_COUNT}"
    ))
}

// ---------------------------------------------------------------------------
// How a label is varied
// ---------------------------------------------------------------------------

/// The words this registrar puts IN FRONT of the requested label when suggesting a near
/// name under a suffix the label is taken under.
///
/// Which words those are is this emulator's own choice — Route 53 publishes no list — so
/// they are a fixed table for the same reason the per-TLD rates are one: the suggestions
/// for a name are then the same on every call. They are short, lower-case, letters only,
/// so a variant of a well-formed label is itself a well-formed label.
const SUGGESTION_PREFIXES: &[&str] = &["my", "the", "get", "go", "try"];

/// The words this registrar puts AFTER the requested label, on the same terms as
/// [`SUGGESTION_PREFIXES`].
const SUGGESTION_SUFFIXES: &[&str] = &[
    "app", "group", "hq", "now", "online", "shop", "site", "world",
];

/// The variants of `label`: each prefix in front of it, then each suffix after it, in table
/// order, so the sequence is the same on every call.
fn label_variants(label: &str) -> impl Iterator<Item = String> + '_ {
    SUGGESTION_PREFIXES
        .iter()
        .map(move |prefix| format!("{prefix}{label}"))
        .chain(
            SUGGESTION_SUFFIXES
                .iter()
                .map(move |suffix| format!("{label}{suffix}")),
        )
}

/// Add `candidate` to `offered` unless it is not a registrable name, or a name already
/// offered.
///
/// A candidate is kept only if [`resolve_domain_name`] accepts it — a variant can run a
/// label past the length DNS allows one, and a registrar cannot suggest a name no registry
/// would delegate — and it is kept in the resolved (lower-cased) form, which is the form
/// every other domain-name answer in this crate is stated in.
fn offer(candidate: &str, seen: &mut BTreeSet<String>, offered: &mut Vec<String>) {
    let Ok(resolved) = resolve_domain_name(candidate) else {
        return;
    };
    if seen.insert(resolved.clone()) {
        offered.push(resolved);
    }
}

/// The names this registrar suggests for the resolved domain name `name`, most similar
/// first and each one registrable, unique and different from `name`.
///
/// The order is the order the derivations are stated in the module documentation: the same
/// label under the other top-level domains this registrar offers, then the label's variants
/// under the top-level domain the caller asked for, then those variants under the other
/// top-level domains. It is a bounded list — the TLD table and the variant tables are both
/// fixed — so a caller asking for the documented maximum is answered from it without the
/// list ever having to grow with the request.
///
/// `name` is expected to have been through [`resolve_domain_name`] already, as every
/// handler that takes a `DomainName` puts it; a value that is not a domain name at all
/// suggests nothing.
pub fn suggestion_candidates(name: &str) -> Vec<String> {
    // A resolved name is exactly one label under a public suffix, so the FIRST dot is the
    // registrable boundary and the suffix is everything after it — which is itself more
    // than one label under a registry that delegates one (`co.uk`). A name of three labels
    // never reaches here: `resolve_domain_name` refuses it as an unsupported TLD.
    let Some((label, tld)) = name.split_once('.') else {
        return Vec::new();
    };

    let mut seen: BTreeSet<String> = BTreeSet::new();
    // The requested name is never suggested back: the caller asked for alternatives to it.
    seen.insert(name.to_ascii_lowercase());
    let mut offered: Vec<String> = Vec::new();

    // The top-level domains this registrar offers, other than the requested one: the
    // published per-TLD rate table is the registrar's own catalogue of them.
    let others: Vec<&str> = STANDARD_REGISTRATION_RATES
        .iter()
        .map(|(other, _)| *other)
        .filter(|other| !other.eq_ignore_ascii_case(tld))
        .collect();

    for other in &others {
        offer(&format!("{label}.{other}"), &mut seen, &mut offered);
    }
    for variant in label_variants(label) {
        offer(&format!("{variant}.{tld}"), &mut seen, &mut offered);
    }
    for other in &others {
        for variant in label_variants(label) {
            offer(&format!("{variant}.{other}"), &mut seen, &mut offered);
        }
    }
    offered
}

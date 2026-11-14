// SPDX-License-Identifier: CC0-1.0

//! WHERE a stage holds the result a `CUSTOM` authorizer returned for ONE identity.
//!
//! The entry is keyed by the authorizer and by the identity the request presented to it, and
//! every part of that identity comes off the REQUEST — a header, a query-string parameter
//! ([`super::identity_source`]) — so the key has to be unambiguous for any bytes a caller can
//! send: a key that two different identities can spell would serve one caller the decision the
//! authorizer returned for another for the rest of the TTL.

/// The separator between the key's components. It appears in no component, because each one
/// is written with its own byte length in front of it (see [`authorizer_cache_key`]), so it
/// is a reading aid rather than what makes the key unambiguous.
const COMPONENT_SEPARATOR: char = '|';

/// Where the stage holds the result `authorizer` returned for the identity
/// `identity_values`, or `None` when it holds none at all — an authorizer whose TTL is zero
/// ("if it equals 0, authorization caching is disabled") and one the request presented no
/// identity value to.
///
/// The key is UNAMBIGUOUS for every identity a request can present: each component is
/// written as its byte LENGTH, a `:` and then the component itself, and the number of
/// components is part of the key — so a component that carries the separator, or an identity
/// of a different arity, cannot spell the key of another identity. A plain join could: the
/// identities `["a|b", "c"]` and `["a", "b|c"]` flatten to the same string.
pub(super) fn authorizer_cache_key(
    authorizer: &crate::state::AuthorizerEntry,
    identity_values: &[String],
) -> Option<String> {
    if authorizer.authorizer_result_ttl_in_seconds <= 0 || identity_values.is_empty() {
        return None;
    }
    let mut key = framed(&authorizer.id);
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&identity_values.len().to_string());
    for value in identity_values {
        key.push(COMPONENT_SEPARATOR);
        key.push_str(&framed(value));
    }
    Some(key)
}

/// One component of the key, with its own byte length in front of it: `3:abc`.
fn framed(component: &str) -> String {
    format!("{len}:{component}", len = component.len())
}

#[cfg(test)]
mod tests {
    //! The one property this key has to have, over a set of identities rather than one
    //! recorded call: identities that differ spell keys that differ, and identities that are
    //! equal spell the same key.

    use super::*;

    /// An authorizer that CACHES, under the id a row names.
    fn caching(id: &str) -> crate::state::AuthorizerEntry {
        crate::state::AuthorizerEntry {
            id: id.to_string(),
            authorizer_result_ttl_in_seconds: 300,
            ..crate::state::AuthorizerEntry::default()
        }
    }

    /// The identity `values` as the resolver hands it over.
    fn identity(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn two_identities_that_differ_are_held_under_keys_that_differ() {
        // Rows span what the rule generalises over: a component carrying the separator at
        // each position, components whose concatenation is the same, identities of
        // different arity, an empty component, and a component carrying the framing
        // characters themselves.
        let rows: [(&str, Vec<&str>, Vec<&str>); 7] = [
            (
                "the separator inside the first component",
                vec!["a|b", "c"],
                vec!["a", "b|c"],
            ),
            (
                "the same characters split another way",
                vec!["ab", "c"],
                vec!["a", "bc"],
            ),
            (
                "one component against two that concatenate to it",
                vec!["abc"],
                vec!["ab", "c"],
            ),
            (
                "an empty component beside a value",
                vec!["", "abc"],
                vec!["abc", ""],
            ),
            (
                "a component spelling the framing of another",
                vec!["3:abc"],
                vec!["abc"],
            ),
            (
                "a trailing empty component",
                vec!["abc"],
                vec!["abc", ""],
            ),
            (
                "the separator alone against two empty components",
                vec!["|"],
                vec!["", ""],
            ),
        ];

        let authorizer = caching("auth1");
        for (case, left, right) in rows {
            let one = authorizer_cache_key(&authorizer, &identity(&left))
                .unwrap_or_else(|| panic!("case {case:?}: a caching authorizer has a key"));
            let other = authorizer_cache_key(&authorizer, &identity(&right))
                .unwrap_or_else(|| panic!("case {case:?}: a caching authorizer has a key"));
            assert_ne!(
                one, other,
                "case {case:?}: {left:?} and {right:?} must be held apart, both got {one:?}"
            );
        }
    }

    #[test]
    fn the_same_identity_of_the_same_authorizer_is_held_under_one_key() {
        let authorizer = caching("auth1");
        for values in [vec!["token"], vec!["a|b", "c"], vec!["", ""]] {
            let case = format!("{values:?}");
            let one = authorizer_cache_key(&authorizer, &identity(&values));
            let other = authorizer_cache_key(&authorizer, &identity(&values));
            assert_eq!(
                one, other,
                "case {case}: one identity is held under one key, got {one:?} then {other:?}"
            );
            assert!(
                one.is_some(),
                "case {case}: a caching authorizer holds a result"
            );
        }
    }

    #[test]
    fn one_identity_is_held_apart_per_authorizer() {
        // The authorizer's own id is framed too, so an id that ends in what the next
        // component begins with cannot move one authorizer's entry onto another's.
        let values = identity(&["token"]);
        let first = authorizer_cache_key(&caching("auth"), &values).expect("a caching authorizer");
        let second =
            authorizer_cache_key(&caching("auth|1"), &values).expect("a caching authorizer");
        assert_ne!(
            first, second,
            "case two authorizers: each holds its own entry, both got {first:?}"
        );
    }

    #[test]
    fn nothing_is_held_where_the_records_hold_nothing() {
        // Rows: the TTL the member documents as caching disabled, a stored value below it,
        // and an identity the request presented nothing for.
        let values = identity(&["token"]);
        for (case, ttl, presented) in [
            ("a TTL of zero", 0, values.clone()),
            ("a TTL below zero", -1, values.clone()),
            ("no identity value at all", 300, Vec::new()),
        ] {
            let authorizer = crate::state::AuthorizerEntry {
                id: "auth1".to_string(),
                authorizer_result_ttl_in_seconds: ttl,
                ..crate::state::AuthorizerEntry::default()
            };
            let key = authorizer_cache_key(&authorizer, &presented);
            assert!(
                key.is_none(),
                "case {case:?}: nothing is held, got {key:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The authenticator every address this service publishes for a client to FETCH carries.
//!
//! The three addresses this emulator serves itself — a catalog record's logo thumbnail
//! ([`super::listing_logo`]), the reviews page a review source keeps for a listing
//! ([`super::listing_reviews`]) and the buyer-facing pages of a SaaS fulfillment option
//! ([`super::saas_fulfillment`]) — are opened by whatever browser, probe or test holds the
//! address, with no credential of their own: that is how AWS serves them, and a fetch of one
//! therefore reaches `crate::service::override_routing_error` ahead of anything that could
//! resolve a caller.
//!
//! So the ADDRESS is what stands in for the read that handed it out, and an address a caller
//! could have derived stands in for nothing: the plane layout is in this source and a
//! record's own id travels in every response, so a path assembled from the two is a path
//! nobody was given. Each published path therefore carries one further segment, the
//! authenticator this process's own secret produces over the record part of that path — and
//! the route that answers there reads it back under the same secret BEFORE any catalog
//! record is looked up ([`authenticates`]), so a path a caller built, one another process
//! minted and one minted for a different record are all refused without the store being
//! touched.
//!
//! The secret is drawn from the system's randomness the first time an address is published,
//! held only in memory and never reported, so it cannot be read out of a response, a log or
//! this source; it is the one `service_common::page_tokens` keeps for a value a service
//! hands out and takes back, which is also what authenticates this service's pagination
//! cursors ([`super::page_cursor`]). It is per PROCESS rather than per store because an
//! address is published and served by one running emulator: the record the address names is
//! still resolved against the store that answers the fetch, so two stores in one process
//! cannot answer for each other's records.

use std::sync::LazyLock;

use ::service_common::page_tokens::PageTokenKey;

/// The secret this process mints the authenticator of a published address under.
static ADDRESS_KEY: LazyLock<PageTokenKey> = LazyLock::new(PageTokenKey::new);

/// The authenticator the published address of `subject` carries, where `subject` is the
/// record part of that address — the media id of a logo, the id of a reviews list, the id of
/// a fulfillment option.
///
/// Rendered as hexadecimal, which is one URL path segment carrying no character a URL
/// encodes, through the `hex` crate rather than a second rendering written out here.
#[must_use]
pub(super) fn authenticator(subject: &str) -> String {
    ::hex::encode(ADDRESS_KEY.mint(subject))
}

/// Whether `candidate` is the authenticator this process minted for `subject`.
///
/// The authenticator is verified with the constant-time comparison
/// `service_common::page_tokens` makes, so a refusal tells a caller nothing about how close
/// its value was; a value that is not hexadecimal at all, one this process did not mint, and
/// one minted for another record are refused alike.
#[must_use]
pub(super) fn authenticates(subject: &str, candidate: &str) -> bool {
    ::hex::decode(candidate)
        .ok()
        .and_then(|minted| ADDRESS_KEY.cursor_of(&minted))
        .is_some_and(|minted_for| minted_for == subject)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an address's authenticator reads back only for the record it was minted for,
    /// over a table spanning the record parts the three planes of this service put in a
    /// path, and no value a caller could assemble reads back at all.
    #[test]
    fn an_authenticator_reads_back_only_for_its_own_record() {
        let subjects = [
            "1d8e4f27-63ba-4c91-9f05-7c3e2ab4d618",
            "l-0001-analytics",
            "fo-0001-analytics-saas",
            "reviews-list-1",
            "",
        ];
        for subject in subjects {
            let minted = authenticator(subject);
            assert!(
                authenticates(subject, &minted),
                "case {subject:?}: a minted authenticator must read back"
            );
            assert!(
                minted
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "case {subject:?}: an authenticator is one hexadecimal path segment, got \
                 {minted:?}"
            );
            for other in subjects.iter().filter(|other| **other != subject) {
                assert!(
                    !authenticates(other, &minted),
                    "case {subject:?}: the authenticator must not read back for {other:?}"
                );
            }
            // Values a caller writes by hand: the record part itself, its hex, a changed
            // byte of a real authenticator, a truncated one, and text that is not hex.
            for (case, forged) in [
                ("the record part itself", subject.to_string()),
                ("hex of the record part", ::hex::encode(subject)),
                (
                    "a changed authenticator",
                    minted.replacen(['0', 'a'], "1", 1),
                ),
                (
                    "a truncated authenticator",
                    minted.get(..minted.len() - 2).unwrap_or("").to_string(),
                ),
                ("not hexadecimal", "not-an-authenticator".to_string()),
            ] {
                if forged == minted {
                    continue;
                }
                assert!(
                    !authenticates(subject, &forged),
                    "case {subject:?} {case}: a value this process did not mint must \
                     authenticate nothing, got {forged:?}"
                );
            }
        }
    }
}

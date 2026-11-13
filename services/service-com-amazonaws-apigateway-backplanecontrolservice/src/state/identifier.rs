// SPDX-License-Identifier: CC0-1.0

//! How this control plane draws the identifiers it ASSIGNS.
//!
//! Every id this service hands back for a record it created — a `RestApi`, an `ApiKey`, a
//! `UsagePlan`, a `Deployment`, a `DocumentationPart`, each of the keys a bulk import
//! creates — is an OPAQUE handle: the caller cannot derive it, holding one tells it nothing
//! about any other, and two records never share one. Each id is therefore DRAWN
//! independently here, one character at a time, rather than computed from how many ids the
//! process has minted before it: an id derived from a creation count is guessable from that
//! count, repeats itself in a second process, and makes the id a record carries a function
//! of the unrelated creations that happened to precede it.
//!
//! The record keeps the id it was drawn: the handler that mints one stores it with the
//! record, so every later read, listing and cross-reference reports the same characters.

use rand::Rng;

/// The characters an API Gateway id is written with: the lowercase letters and the digits.
const ID_ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The width of an API Gateway id that names a record of the ACCOUNT — a `RestApi`, an
/// `ApiKey`, a `UsagePlan`, an `Authorizer`, a `RequestValidator` — or the ROOT RESOURCE a
/// REST API is created with, which is minted with the API: ten characters.
pub const ACCOUNT_RECORD_ID_LEN: usize = 10;

/// The width of an API Gateway id that names a record created INSIDE one REST API — a
/// `Resource`, a `Model`, a `Deployment`: six characters, which is the width those records'
/// ids are recorded at and the width a client that length-bounds one expects.
pub const SUB_RESOURCE_ID_LEN: usize = 6;

/// `width` base-36 characters, each drawn independently and uniformly.
///
/// Drawing per CHARACTER rather than rendering one number is what keeps every position
/// uniform: a fixed-width id formatted from a counter, however the counter is scrambled
/// first, carries whichever characters the values that counter can reach happen to render
/// as — and the leading or trailing ones then never vary at all.
pub fn drawn_id(width: usize) -> String {
    let mut generator = rand::rng();
    (0..width)
        .map(|_| char::from(ID_ALPHABET[generator.random_range(0..ID_ALPHABET.len())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawn_id_is_the_requested_width_of_lowercase_alphanumerics() {
        // Rows differ along the width asked for: none at all, one character, and each of
        // the two widths this service spells an id at.
        for width in [0usize, 1, SUB_RESOURCE_ID_LEN, ACCOUNT_RECORD_ID_LEN] {
            for _ in 0..64 {
                let id = drawn_id(width);
                assert_eq!(
                    id.chars().count(),
                    width,
                    "case {width}: expected {width} characters, got {id:?}"
                );
                assert!(
                    id.chars()
                        .all(|digit| digit.is_ascii_lowercase() || digit.is_ascii_digit()),
                    "case {width}: expected lowercase alphanumerics, got {id:?}"
                );
            }
        }
    }

    #[test]
    fn an_assigned_id_is_drawn_rather_than_counted() {
        // Enough draws that a generator walking a sequence shows up as a repeat, and that
        // every position of a counter-rendered id would hold its constant characters.
        const DRAWS: usize = 1_000;
        for width in [SUB_RESOURCE_ID_LEN, ACCOUNT_RECORD_ID_LEN] {
            let mut ids: Vec<String> = (0..DRAWS).map(|_| drawn_id(width)).collect();
            for position in 0..width {
                let seen: std::collections::BTreeSet<Option<char>> =
                    ids.iter().map(|id| id.chars().nth(position)).collect();
                assert!(
                    seen.len() > 1,
                    "case width {width}: character {position} never varies, got {seen:?}"
                );
            }
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(
                ids.len(),
                DRAWS,
                "case width {width}: expected {DRAWS} distinct ids, got {}",
                ids.len()
            );
        }
    }
}

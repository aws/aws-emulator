// SPDX-License-Identifier: CC0-1.0

//! The concurrency precondition a link read hands out, and a conditional write
//! presents back.
//!
//! The modelled `LockToken` is opaque to the caller, carries no time limit, and
//! goes stale exactly when a change to the link is accepted — by any operation,
//! including the call that presented it. So it is not stored: it is COMPUTED from
//! the record it is a precondition on, out of that link's id and the number of
//! changes the link has accepted ([`Link::revision`](super::Link::revision)). Two
//! reads between which nothing was accepted therefore hand out the same token,
//! one taken after an accepted change hands out a different one, and a token no
//! longer current can never be mistaken for the current one.
//!
//! Stated once here so every link surface of this service — a link between two
//! gateways and a link with one end outside the fabric alike — hands out the same
//! kind of token under the same rule, rather than each reader inventing one.

/// The lock token of the link `link_id` while it holds the state its `revision`th
/// accepted change left.
///
/// A digest, so nothing of the record's internals travels in the token and the
/// caller has nothing to read out of it — the modelled `LockToken` is opaque and
/// passed back unchanged. It is a precondition rather than a credential: what it
/// decides is whether the link has changed since the caller read it, not whether
/// the caller may change the link, which the request's own signature decides.
/// Rendered hexadecimal, so it survives any transport, and 64 characters long,
/// within the modelled `LockToken` length (1..256).
pub fn lock_token(link_id: &str, revision: u64) -> String {
    hex::encode(super::idempotent_creates::digest(&[
        link_id.as_bytes(),
        &revision.to_be_bytes(),
    ]))
}

#[cfg(test)]
mod tests {
    use super::lock_token;

    /// A token names ONE revision of ONE link: it is stable while nothing is
    /// accepted, and no two (link, revision) pairs share one.
    #[test]
    fn a_token_names_one_revision_of_one_link() {
        for (case, link_id, revision) in [
            ("a minted id at its first revision", "link-0000000000000001", 0u64),
            ("the same link once changed", "link-0000000000000001", 1),
            ("the same link far along", "link-0000000000000001", 4_096),
            ("another link at the same revision", "link-0000000000000002", 1),
            ("the shortest id the pattern allows", "link-a", 0),
        ] {
            let token = lock_token(link_id, revision);
            assert_eq!(
                token,
                lock_token(link_id, revision),
                "case {case:?}: expected a read that changed nothing to hand out the same token, got {token:?}"
            );
            assert!(
                (1..=256).contains(&token.len()),
                "case {case:?}: expected a token within the modelled length, got {} characters",
                token.len()
            );
            assert_ne!(
                token,
                lock_token(link_id, revision.saturating_add(1)),
                "case {case:?}: expected the next revision of the link to hand out another token"
            );
            assert_ne!(
                token,
                lock_token("link-ffffffffffffffff", revision),
                "case {case:?}: expected another link's token to differ"
            );
        }
    }
}

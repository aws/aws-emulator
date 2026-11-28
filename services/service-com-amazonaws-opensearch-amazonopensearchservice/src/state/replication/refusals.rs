// SPDX-License-Identifier: CC0-1.0

//! What this service answers a refused call on a domain's REPLICATION arrangement with: one
//! builder per condition the five calls refuse, so every call that meets a condition states
//! it the same way and no two of them can word one miss differently.
//!
//! Each payload is the shape the operation declares for that class — the missing-resource
//! one, the request-validation one, the already-exists one — and its variable parts are the
//! domain's own ARN, the peer the request named and the Region the request was signed for.

use super::*;

/// What this service says of a domain that replicates with nobody, in whichever envelope
/// the call refusing it answers with: the ONE wording, so two of those calls cannot word
/// the same miss differently.
///
/// The subject is the CALLING domain's own ARN rather than the peer's — the domain the
/// request is addressed to is the one that is not replicating.
#[must_use]
pub fn domain_not_replicating_message(domain_arn: &str) -> String {
    format!("Domain is not replicating: {domain_arn}")
}

/// The `ResourceNotFoundException` payload for a call that acts on the arrangement of a
/// domain that replicates with nobody.
#[must_use]
pub fn domain_not_replicating(domain_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(domain_not_replicating_message(domain_arn)),
    }
}

/// The `ValidationException` payload for the same condition, for a call that refuses it with
/// the request-validation envelope rather than the missing-resource one.
#[must_use]
pub fn not_replicating_validation(domain_arn: &str) -> ValidationException {
    ValidationException {
        message: Some(domain_not_replicating_message(domain_arn)),
    }
}

/// What this service says of a domain already replicating with a peer when a second
/// relationship is asked for: one domain, one peer ([`PEER_QUOTA`]), and the peer it already
/// has is what a caller must release first.
#[must_use]
pub fn peer_already_established(
    domain_arn: &str,
    peer_domain_arn: &str,
) -> ResourceAlreadyExistsException {
    ResourceAlreadyExistsException {
        message: Some(format!(
            "Domain {domain_arn} is already replicating with {peer_domain_arn}"
        )),
    }
}

/// What it says of a domain named as its own peer: a relationship has two sides, and both of
/// these would be one domain.
#[must_use]
pub fn peer_is_the_domain_itself(domain_arn: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("Domain cannot replicate with itself: {domain_arn}")),
    }
}

/// What a call naming a peer the domain holds no relationship with says: "the request is
/// rejected if this domain is no longer the peer of the domain you are calling".
#[must_use]
pub fn not_the_peer(domain_arn: &str, peer_domain_arn: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Domain {peer_domain_arn} is not the peer of {domain_arn}"
        )),
    }
}

/// What a failover says of a request that does not acknowledge the writes it discards: "you
/// must set this to true; the request is rejected otherwise"
/// (`FailoverReplicationRequest$AllowDataLoss`).
#[must_use]
pub fn data_loss_not_allowed() -> ValidationException {
    ValidationException {
        message: Some(
            "AllowDataLoss must be true to fail over, because writes that had not yet \
             replicated are lost"
                .to_string(),
        ),
    }
}

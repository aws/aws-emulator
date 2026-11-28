// SPDX-License-Identifier: CC0-1.0

//! What one `RunInstances` `ClientToken` claimed, and how a replay of that token is judged.
//!
//! "Unique, case-sensitive identifier you provide to ensure the idempotency of the request"
//! (`RunInstancesRequest$ClientToken`): a token names the ONE launch it was spent on, so a
//! repeat of that launch under the same token answers the reservation the first one minted
//! instead of launching again — and a request that carries the same token with DIFFERENT
//! launch parameters is not that launch at all. EC2 refuses it
//! ([`super::idempotent_parameter_mismatch`]) rather than launching a second time, which is
//! what stops a retrying client duplicating its fleet.
//!
//! The comparison is over the WHOLE request rather than a list of members: every parameter of
//! a launch describes the instances it mints, so any difference makes it another launch. The
//! two members that are not part of the launch are left out of it — the token itself, which
//! is the same by definition, and `DryRun`, which asks about a launch rather than making one.
//! A DIGEST rather than a copy of the parameters ([`launch_parameters_digest`]), so the claim
//! a token holds is a fixed handful of bytes however large the request was.

use crate::ops::types::RunInstancesInput;
use crate::scaffolded::Scaffolded;

/// Refuse a LAUNCH that reuses a `ClientToken` for a launch that token did not name: HTTP 400
/// [`super::IDEMPOTENT_PARAMETER_MISMATCH`], in the sentence EC2 words that refusal with for a
/// launch.
///
/// The code is the error class's and is named once for the whole service; the SENTENCE is this
/// operation's, because EC2 does not word the class the same way on every operation that takes
/// a token — a reused `CreateVolume` token is refused the clause
/// [`super::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`] states, while a launch is refused the
/// condition itself. Both state the condition and name nothing of the request: not the token,
/// not the member that differs.
#[must_use]
pub fn launch_idempotent_parameter_mismatch<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        super::IDEMPOTENT_PARAMETER_MISMATCH,
        400,
        "The request uses the same client token as a previous, but non-identical request.",
    )
}

/// The launch one `ClientToken` was spent on: the reservation it minted and the digest of the
/// parameters it was spent with.
///
/// Held in [`super::ComAmazonawsEc2Amazonec2State::client_tokens`], the one place a launch
/// token is recorded, so the replay a token serves and the mismatch it refuses are read off
/// one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchClaim {
    /// The `ReservationId` the token's launch minted, which a replay answers with.
    pub reservation_id: String,
    /// [`launch_parameters_digest`] of the launch it was spent on.
    pub parameters: u64,
}

/// A deterministic digest of the launch `input` describes, ignoring the two members that are
/// not part of the launch: the `ClientToken` itself and `DryRun`.
///
/// The request's own debug rendering is what is digested: it carries every member of the
/// resolved launch, so no member can be left out of the comparison by omission here, and a
/// launch that differs in any of them digests differently. The standard library's hasher,
/// seeded by its own default rather than randomly, so the digest is a pure function of the
/// launch — the same launch always claims the same value, which is what makes a replay
/// recognisable. Nothing here is a secret and nothing depends on a digest being unforgeable.
#[must_use]
pub fn launch_parameters_digest(input: &RunInstancesInput) -> u64 {
    use ::std::hash::{Hash, Hasher};

    let mut compared = input.clone();
    compared.client_token = None;
    compared.dry_run = None;
    let mut hasher = ::std::hash::DefaultHasher::new();
    format!("{compared:?}").hash(&mut hasher);
    hasher.finish()
}

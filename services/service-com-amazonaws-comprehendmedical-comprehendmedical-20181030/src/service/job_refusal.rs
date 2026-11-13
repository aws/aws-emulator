// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request that names an asynchronous job it cannot find.
//!
//! Amazon Comprehend Medical reports that condition as the modelled
//! `ResourceNotFoundException` ("The resource identified by the specified Amazon Resource
//! Name (ARN) was not found") carrying one fixed sentence, and answers it with HTTP status
//! 400 — the status it answers a client-input refusal with — not the 404 the shape's own
//! `@httpError` trait declares. The shape, the sentence and that status live here, in the
//! one place every per-id job operation words the refusal through, so no handler words it
//! on its own and the service answers one status for the shape.
//!
//! The sentence itself is NOT written here: it is [`crate::state::JobNotFound`], beside
//! the family stores whose lookup produces it. The status override is the error channel's
//! own [`crate::scaffolded::Scaffolded::modeled_with_status`], so nothing about how the
//! error is rendered is re-implemented: the body and the `__type` stay the model's.

use crate::ops::types::ResourceNotFoundException;
use crate::scaffolded::Scaffolded;
use crate::state::JobNotFound;

/// The HTTP status this service answers the absent-job refusal with.
///
/// Recorded evidence, as [`crate::scaffolded::Scaffolded::ModeledWithStatus`] requires:
/// the replayed `DescribePHIDetectionJob` and `DescribeEntitiesDetectionV2Job` responses
/// for a well-formed `JobId` naming no job carry HTTP 400 with the
/// `ResourceNotFoundException` code, so a caller's retry classification and any handling
/// keyed on the status class branch on this, not on the shape's `@httpError 404`.
const JOB_NOT_FOUND_STATUS: u16 = 400;

/// An operation's modelled error enum, seen as the one that carries the not-found shape.
///
/// Implemented for every per-id asynchronous-job operation, which is what lets [`refuse`]
/// state the refusal once for all of them instead of once per handler.
pub trait CarriesJobNotFound {
    /// This enum's `ResourceNotFoundException` variant, holding `error`.
    fn job_not_found(error: ResourceNotFoundException) -> Self;
}

impl CarriesJobNotFound
    for crate::ops::describe_phi_detection_job::DescribePHIDetectionJobErrorModeled
{
    fn job_not_found(error: ResourceNotFoundException) -> Self {
        Self::ResourceNotFoundException(error)
    }
}

impl CarriesJobNotFound
    for crate::ops::describe_entities_detection_v2_job::DescribeEntitiesDetectionV2JobErrorModeled
{
    fn job_not_found(error: ResourceNotFoundException) -> Self {
        Self::ResourceNotFoundException(error)
    }
}

impl CarriesJobNotFound
    for crate::ops::stop_entities_detection_v2_job::StopEntitiesDetectionV2JobErrorModeled
{
    fn job_not_found(error: ResourceNotFoundException) -> Self {
        Self::ResourceNotFoundException(error)
    }
}

impl CarriesJobNotFound for crate::ops::stop_phi_detection_job::StopPHIDetectionJobErrorModeled {
    fn job_not_found(error: ResourceNotFoundException) -> Self {
        Self::ResourceNotFoundException(error)
    }
}

/// The refusal `absent` states, as the operation's error channel carries it: the modelled
/// not-found shape with the service's one fixed sentence — which echoes nothing of the
/// request — answered with [`JOB_NOT_FOUND_STATUS`].
pub fn refuse<E: CarriesJobNotFound>(absent: JobNotFound) -> Scaffolded<E> {
    Scaffolded::modeled_with_status(
        E::job_not_found(ResourceNotFoundException {
            message: Some(absent.message().to_string()),
        }),
        JOB_NOT_FOUND_STATUS,
    )
}

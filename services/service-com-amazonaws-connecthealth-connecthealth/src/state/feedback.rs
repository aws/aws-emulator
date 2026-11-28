// SPDX-License-Identifier: CC0-1.0

//! The feedback a `SubmitFeedback` records against one artifact this service generated.
//!
//! The operation answers an EMPTY body (`SubmitFeedbackResponse` declares no members) with
//! HTTP 202, and this model carries no operation that reads feedback back — so what the
//! call establishes is a RECORD, held under the `Domain` the feedback was submitted in and
//! gone with it. It is kept rather than dropped because an accepted submission that stored
//! nothing would make the 202 an empty success: the record is what the acceptance means.

use std::time::SystemTime;

/// The most feedback submissions ONE `Domain` holds.
///
/// The EMULATOR's bound on this store's growth rather than a published quota:
/// `SubmitFeedback` is the one write that adds a submission, and a caller looping it would
/// otherwise grow the shared store without limit. Past this count a submission is refused
/// with the `ValidationException` the operation models, naming the count it enforced.
pub const MAX_FEEDBACK_PER_DOMAIN: usize = 100;

/// One recorded feedback submission: the artifact it is about, who it concerns and what the
/// caller said, exactly as the request stated it.
#[derive(Debug, Clone)]
pub struct Feedback {
    /// The id this service minted for the submission, which is the key it is held under.
    pub feedback_id: String,
    pub domain_id: String,
    /// The artifact the feedback is about — `artifactSourceId`, which the model declares as
    /// the id of the source the artifact came from.
    pub artifact_source_id: String,
    /// The subscription the feedback was submitted under, when the request named one.
    pub subscription_id: Option<String>,
    /// The modelled `FeedbackArtifactType` the request named (`CLINICAL_DOC` or
    /// `MEDICAL_CODES`).
    pub artifact_type: String,
    pub signed_artifact_s3_uri: String,
    pub generated_artifact_s3_uri: Option<String>,
    /// The caller's comments. `@sensitive`, and reported by no operation of this model.
    pub comments: Option<String>,
    /// The `clientToken` the submission carried, which is what tells a RETRY of one
    /// submission from a second submission about the same artifact.
    pub client_token: Option<String>,
    pub submitted_at: SystemTime,
}

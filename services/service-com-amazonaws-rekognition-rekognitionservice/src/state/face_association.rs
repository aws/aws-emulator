// SPDX-License-Identifier: CC0-1.0

//! Binding the faces a collection holds to one of its users and taking them off it
//! again: the bounds and the defaults `AssociateFaces` works to, the one entry each of
//! `AssociateFaces` and `DisassociateFaces` reports per face it could not move, and the
//! answers each has already given.
//!
//! The two operations are the two directions of one binding — the `UserId` a stored face
//! carries — so the span of the update they leave pending, the reason values they report
//! and the ledger of answers a `ClientRequestToken` has already committed are written here
//! once and shared by both.
//!
//! `AssociateFaces` "associates one or more faces with an existing UserID … If a given
//! face is already associated with the given UserID, it will be ignored and will not be
//! returned in the response. If a given face is already associated to a different UserID,
//! isn't found in the collection, doesn't meet the UserMatchThreshold, or there are
//! already 100 faces associated with the UserID, it will be returned as part of an array
//! of UnsuccessfulFaceAssociations"
//! (`smithy_query(query="doc",shape="AssociateFaces")`). That is one rule per outcome, and
//! each outcome is one shape of entry, so the entries are built here rather than spelled
//! out in the handler.
//!
//! What the EMULATOR can decide about a candidate face is the same measurement the rest of
//! this service's face comparisons rest on: a stored face kept the appearance the detector
//! read off its own pixels ([`super::StoredFace::appearance`]), so the user match
//! confidence of a candidate is the resemblance measured between it and the faces the user
//! already holds. Nothing infers WHO a face belongs to; see this crate's [`super`] docs.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::ops::types::{
    AssociateFacesOutput, AssociatedFace, DisassociateFacesOutput, DisassociatedFace,
    UnsuccessfulFaceAssociation, UnsuccessfulFaceDisassociation,
};

use super::face_detection::FaceAppearance;
use super::idempotency::MAX_REMEMBERED_TOKENS;

/// The user match confidence a candidate face must reach when the request names none:
/// "the UserMatchThreshold parameter specifies the minimum user match confidence required
/// for the face to be associated with a UserID that has at least one FaceID already
/// associated … the value ranges from 0-100 and default value is 75"
/// (`smithy_query(query="doc",shape="AssociateFaces")`).
pub const DEFAULT_USER_MATCH_THRESHOLD: f32 = 75.0;

/// How many faces one user may have associated with it: "the total number of faces that
/// can be associated with a single UserID is also limited to 100. Once a UserID has 100
/// faces associated with it, no additional faces can be added. If more API calls are made
/// after the limit is reached, a ServiceQuotaExceededException will result"
/// (`smithy_query(query="doc",shape="AssociateFaces")`). AWS's own limit, not a bound this
/// emulator chose.
pub const MAX_FACES_PER_USER: usize = 100;

/// How long after the call that started it an update of a user's faces is complete.
///
/// `AssociateFaces` and `DisassociateFaces` both answer while the update is still being
/// applied — that is what the `UPDATING` status they answer with means — so the span is
/// never zero: a caller that reads the user straight after the call sees it updating. This
/// emulator owes the transition at a span of its own choosing, and it is this one.
pub const UPDATE_SPAN: Duration = Duration::from_secs(3);

/// The `UnsuccessfulFaceAssociationReason` values
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceAssociationReason")`:
/// FACE_NOT_FOUND | ASSOCIATED_TO_A_DIFFERENT_USER | LOW_MATCH_CONFIDENCE). The first two
/// are also the whole of `UnsuccessfulFaceDisassociationReason`
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceDisassociationReason")`:
/// FACE_NOT_FOUND | ASSOCIATED_TO_A_DIFFERENT_USER), which is why both directions name
/// them from here.
pub const REASON_FACE_NOT_FOUND: &str = "FACE_NOT_FOUND";
pub const REASON_ASSOCIATED_TO_A_DIFFERENT_USER: &str = "ASSOCIATED_TO_A_DIFFERENT_USER";
pub const REASON_LOW_MATCH_CONFIDENCE: &str = "LOW_MATCH_CONFIDENCE";

/// A `FaceId` the collection does not hold. Only the id is reported: no user holds the
/// face and nothing was measured for it.
pub fn face_not_found(face_id: &str) -> UnsuccessfulFaceAssociation {
    UnsuccessfulFaceAssociation {
        face_id: Some(face_id.to_string()),
        reasons: Some(vec![REASON_FACE_NOT_FOUND.to_string()]),
        ..UnsuccessfulFaceAssociation::default()
    }
}

/// A face another user of the collection already holds, reported with the `UserId` that
/// holds it — "a provided ID for the UserID"
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceAssociation")`), which for this
/// outcome is the user the association would have had to take the face from, and the one
/// piece of it the caller does not already know.
pub fn associated_to_a_different_user(
    face_id: &str,
    user_id: &str,
) -> UnsuccessfulFaceAssociation {
    UnsuccessfulFaceAssociation {
        face_id: Some(face_id.to_string()),
        user_id: Some(user_id.to_string()),
        reasons: Some(vec![REASON_ASSOCIATED_TO_A_DIFFERENT_USER.to_string()]),
        ..UnsuccessfulFaceAssociation::default()
    }
}

/// A face whose measured user match confidence stayed below the threshold, reported with
/// that confidence: `Confidence` is the member that "provides information regarding if a
/// face association was unsuccessful because it didn't meet UserMatchThreshold"
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceAssociation")`).
pub fn low_match_confidence(face_id: &str, confidence: f32) -> UnsuccessfulFaceAssociation {
    UnsuccessfulFaceAssociation {
        face_id: Some(face_id.to_string()),
        confidence: Some(confidence),
        reasons: Some(vec![REASON_LOW_MATCH_CONFIDENCE.to_string()]),
        ..UnsuccessfulFaceAssociation::default()
    }
}

/// A face left over because the user reached [`MAX_FACES_PER_USER`] part way through the
/// request, which the operation documents as an unsuccessful association ("there are
/// already 100 faces associated with the UserID"). `Reasons` stays absent: the modelled
/// reason enum names no value for this outcome, and a reason it does not name is not one
/// this service invents.
pub fn user_is_full(face_id: &str) -> UnsuccessfulFaceAssociation {
    UnsuccessfulFaceAssociation {
        face_id: Some(face_id.to_string()),
        ..UnsuccessfulFaceAssociation::default()
    }
}

/// A `FaceId` the collection does not hold, as `DisassociateFaces` reports it: "if a given
/// face is … not found in the collection it will be returned as part of
/// UnsuccessfulDisassociations" (`smithy_query(query="doc",shape="DisassociateFaces")`).
/// Only the id is reported: no user of the collection holds the face.
pub fn disassociation_face_not_found(face_id: &str) -> UnsuccessfulFaceDisassociation {
    UnsuccessfulFaceDisassociation {
        face_id: Some(face_id.to_string()),
        reasons: Some(vec![REASON_FACE_NOT_FOUND.to_string()]),
        ..UnsuccessfulFaceDisassociation::default()
    }
}

/// A face another user of the collection holds, which the request named a different user
/// for: "if a given face is already associated with a different User … it will be returned
/// as part of UnsuccessfulDisassociations"
/// (`smithy_query(query="doc",shape="DisassociateFaces")`). Reported with the `UserId` that
/// really holds it — "a provided ID for the UserID"
/// (`smithy_query(query="shape",shape="UnsuccessfulFaceDisassociation")`) — which is the
/// one piece of the outcome the caller does not already know, as on the association side.
pub fn disassociation_of_a_different_users_face(
    face_id: &str,
    user_id: &str,
) -> UnsuccessfulFaceDisassociation {
    UnsuccessfulFaceDisassociation {
        face_id: Some(face_id.to_string()),
        user_id: Some(user_id.to_string()),
        reasons: Some(vec![REASON_ASSOCIATED_TO_A_DIFFERENT_USER.to_string()]),
    }
}

/// How well a candidate face matches a user: the strongest resemblance measured between
/// it and the faces the user already holds, on the 0..100 scale a `Percent` carries.
///
/// A user is identified by every face associated with it, so a candidate matches the user
/// as closely as it matches the nearest of them; `None` when the user holds no face yet,
/// which is the case the threshold does not apply to. `held` is anything that yields the
/// appearances of those faces, so a caller that already holds them need not copy them to
/// ask.
pub fn user_match_confidence<'a, Held>(candidate: &FaceAppearance, held: Held) -> Option<f32>
where
    Held: IntoIterator<Item = &'a FaceAppearance>,
{
    held.into_iter()
        .map(|face| face.similarity_to(candidate))
        .fold(None, |best: Option<f32>, measured| {
            Some(best.map_or(measured, |best| best.max(measured)))
        })
}

/// One completed `AssociateFaces` answer, kept under the `ClientRequestToken` that
/// committed it: "if you use the same token with multiple AssociateFaces requests, the
/// same response is returned"
/// (`smithy_query(query="doc",shape="AssociateFacesRequest",member="ClientRequestToken")`),
/// so the answer itself is what the retry needs and the ledger of committed PARAMETERS
/// ([`super::idempotency`]) cannot supply.
#[derive(Debug, Clone, Default)]
pub struct RecordedAssociation {
    /// The `FaceId` of each face the call associated with the user.
    pub associated_face_ids: Vec<String>,
    /// One entry per face the call could not associate, as the constructors above built
    /// it.
    pub unsuccessful: Vec<UnsuccessfulFaceAssociation>,
    /// The `UserStatus` the call answered with.
    pub user_status: String,
}

impl RecordedAssociation {
    /// This answer as the modelled `AssociateFacesResponse`. Both lists are reported
    /// present even when empty: the answer HAS an associated and an unsuccessful list, and
    /// this is their content.
    pub fn modeled(&self) -> AssociateFacesOutput {
        AssociateFacesOutput {
            associated_faces: Some(
                self.associated_face_ids
                    .iter()
                    .map(|face_id| AssociatedFace {
                        face_id: Some(face_id.clone()),
                    })
                    .collect(),
            ),
            unsuccessful_face_associations: Some(self.unsuccessful.clone()),
            user_status: Some(self.user_status.clone()),
        }
    }
}

/// One completed `DisassociateFaces` answer, kept under the `ClientRequestToken` that
/// committed it: "if you use the same token with multiple DisassociateFaces requests, the
/// same response is returned"
/// (`smithy_query(query="doc",shape="DisassociateFacesRequest",member="ClientRequestToken")`),
/// the same rule, and the same reason for keeping the answer itself, as on the association
/// side ([`RecordedAssociation`]).
#[derive(Debug, Clone, Default)]
pub struct RecordedDisassociation {
    /// The `FaceId` of each face the call took off the user.
    pub disassociated_face_ids: Vec<String>,
    /// One entry per face the call could not take off the user, as the constructors above
    /// built it.
    pub unsuccessful: Vec<UnsuccessfulFaceDisassociation>,
    /// The `UserStatus` the call answered with.
    pub user_status: String,
}

impl RecordedDisassociation {
    /// This answer as the modelled `DisassociateFacesResponse`. Both lists are reported
    /// present even when empty: the answer HAS a disassociated and an unsuccessful list,
    /// and this is their content.
    pub fn modeled(&self) -> DisassociateFacesOutput {
        DisassociateFacesOutput {
            disassociated_faces: Some(
                self.disassociated_face_ids
                    .iter()
                    .map(|face_id| DisassociatedFace {
                        face_id: Some(face_id.clone()),
                    })
                    .collect(),
            ),
            unsuccessful_face_disassociations: Some(self.unsuccessful.clone()),
            user_status: Some(self.user_status.clone()),
        }
    }
}

/// The answers one operation of this service remembers by the token that committed them.
///
/// Keyed by the token alone: a ledger belongs to one operation, and a token sent to a
/// different operation commits a different request there. Bounded like the parameter
/// ledger beside it ([`MAX_REMEMBERED_TOKENS`]), so a caller sending a fresh token per
/// request cannot grow this store without limit; an answer that has been forgotten leaves
/// the token classified as a fresh one, which is the same answer the parameter ledger
/// gives for a token it no longer holds.
///
/// Generic over the answer, so the two directions of a face binding keep one
/// implementation of the remembering rather than a copy each.
#[derive(Debug, Default)]
pub struct AnswerLedger<Answer> {
    answers: BTreeMap<String, Answer>,
    /// The record order of the keys above, so the OLDEST answer is the one forgotten when
    /// the ledger is full.
    order: VecDeque<String>,
}

/// The `AssociateFaces` answers this service remembers; see [`AnswerLedger`].
pub type AssociationAnswers = AnswerLedger<RecordedAssociation>;

/// The `DisassociateFaces` answers this service remembers; see [`AnswerLedger`].
pub type DisassociationAnswers = AnswerLedger<RecordedDisassociation>;

impl<Answer> AnswerLedger<Answer> {
    /// The answer `token` committed, if this ledger still holds one.
    pub fn recorded(&self, token: &str) -> Option<&Answer> {
        self.answers.get(token)
    }

    /// Remember `answer` as what `token` was answered with, evicting the oldest remembered
    /// answer once the ledger is full.
    pub fn record(&mut self, token: &str, answer: Answer) {
        while self.order.len() >= MAX_REMEMBERED_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.answers.remove(&oldest);
                }
                None => break,
            }
        }
        if self.answers.insert(token.to_string(), answer).is_none() {
            self.order.push_back(token.to_string());
        }
    }
}

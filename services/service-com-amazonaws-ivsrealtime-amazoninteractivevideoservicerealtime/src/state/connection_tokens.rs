// SPDX-License-Identifier: CC0-1.0

//! The REVOCATION of one connection token this service issued.
//!
//! A connection token is the token a client connects to a stage with, and the tokens this
//! service issues are the participant tokens `CreateStage` and `CreateParticipantToken`
//! mint ([`ParticipantTokenRecord`]): the id a revocation names is the id such a token
//! carries — `ParticipantToken$participantId`, "unique identifier for this participant
//! token, assigned by IVS" — which is why `ConnectionTokenId` and `ParticipantTokenId`
//! accept the same characters
//! (`smithy_query(query="shape",shape="ConnectionTokenId,ParticipantTokenId")`).
//!
//! Revoking one has two effects, and they are the two halves of this module: the token
//! stops admitting its holder to the stage it was issued against, and the connection the
//! holder has right now ends.

use super::*;

/// What one revocation left behind.
#[derive(Debug, Clone)]
pub struct RevokedConnectionToken {
    /// The stage the revoked token was issued against — the stage whose connections it
    /// could open, and the one the revocation ends them in.
    pub stage_arn: String,
    /// Whether THIS call is what revoked the token: `false` for one already revoked, which
    /// is left exactly as the call that revoked it left it.
    pub newly_revoked: bool,
    /// The sessions of that stage the token's holder was taken out of, in the order the
    /// stage holds them. Empty when it was connected in none.
    pub disconnected_sessions: Vec<String>,
}

/// Revoke the connection token `token_id` names in `region`'s view of the account, and end
/// the connection its holder has right now.
///
/// `None` when that view holds no such token: an id is an id of the region whose endpoint
/// issued the token it names ([`in_request_region`] over the stage it was issued against),
/// and at most one token of this store carries any one id, because every id is drawn from
/// the store's own sequence and re-drawn while it is already held
/// ([`issue_token_record`]).
///
/// What revoking DOES, from the operation's own doc ("Revokes the specified connection
/// token so that it can no longer connect to a stage. This operation disconnects any
/// participant currently connected with the revoked token"):
///
/// * the token can no longer connect: the record is marked revoked
///   ([`ParticipantTokenRecord::revoke`]), which takes it out of the identities this
///   service issued against the stage ([`stage_participant`]), so what the stage HAS under
///   that id afterwards is only what its own session records hold
///   ([`attached_participant`]);
/// * a participant connected with it is disconnected, through the one way a control-plane
///   call ends a participant's connection to a stage
///   ([`end_participant_connections`]) — the same departure `DisconnectParticipant`
///   records, so the record reports `DISCONNECTED`, a session it empties ends, and the
///   replications that publisher sourced end with it.
///
/// Revoking a token that is already revoked writes nothing further: the token is where the
/// first revocation left it, and its holder is already out of the stage.
pub fn revoke_connection_token(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    region: &str,
    token_id: &str,
    at: SystemTime,
) -> Option<RevokedConnectionToken> {
    let stage_arn = state
        .participant_tokens
        .iter()
        .filter(|(stage_arn, _)| in_request_region(stage_arn, region))
        .find(|(_, records)| {
            records
                .iter()
                .any(|record| record.participant_id == token_id)
        })
        .map(|(stage_arn, _)| stage_arn.clone())?;

    let newly_revoked = state
        .participant_tokens
        .get_mut(&stage_arn)
        .and_then(|records| {
            records
                .iter_mut()
                .find(|record| record.participant_id == token_id)
        })
        .is_some_and(|record| record.revoke(at));

    // The holder of a token is a participant of the stage it was issued against under the
    // id the token carries, so the connection to end is that participant's. A participant
    // publishing with an ingest configuration carries an id minted WITH that configuration
    // and not with a token ([`mint_ingest_credential`]), so no revocation can name one and
    // the stage association of a configuration is never what this ends.
    let disconnected_sessions = end_participant_connections(state, &stage_arn, token_id, at);

    Some(RevokedConnectionToken {
        stage_arn,
        newly_revoked,
        disconnected_sessions,
    })
}

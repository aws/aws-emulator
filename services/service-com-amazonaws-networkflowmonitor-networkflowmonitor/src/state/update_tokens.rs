// SPDX-License-Identifier: CC0-1.0

//! The `clientToken` window every UPDATE of this service recognises a replay
//! through.
//!
//! Each `@idempotent` update of this service carries an `@idempotencyToken`
//! (`clientToken`) and edits a record's lists, so one rule serves all of them: a
//! token a record has already applied THIS change under is a replay, answered with
//! the record as it stands, and a token that applied a DIFFERENT change is refused
//! ("Don't reuse the same client token for other API requests"). The window lives
//! here, next to the two functions that read and write it, so a record type added
//! later recognises a replay the same way rather than spelling the rule a second
//! time.

use std::collections::VecDeque;

use super::*;

/// What a `clientToken` on an update request means for the record it addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateReplay {
    /// No token, or a token this record has not applied: the request is a new
    /// update and is applied.
    New,
    /// The token already applied THIS change: the request is a replay, answered
    /// with the record as it stands without applying anything again.
    Replay,
    /// The token already applied a DIFFERENT change ("Don't reuse the same client
    /// token for other API requests"), so the request is refused rather than
    /// answered with a success for a write that was discarded.
    Mismatch,
}

/// What `token` means for a record whose applied updates are `applied`, given the
/// change `fingerprint` the request carries — the check an `@idempotent` replay
/// makes before anything is changed, so a retried update neither applies twice nor
/// moves `modifiedAt` again.
///
/// It compares the REQUEST against the remembered update, so a token reused for a
/// different change is not answered with a success for a write nothing applied. A
/// request without a `clientToken` is never a replay.
pub fn replay_of(
    applied: &VecDeque<(String, String)>,
    token: Option<&str>,
    fingerprint: &str,
) -> UpdateReplay {
    let Some(token) = token else {
        return UpdateReplay::New;
    };
    match applied
        .iter()
        .find(|(seen, _)| seen == token)
        .map(|(_, change)| change == fingerprint)
    {
        None => UpdateReplay::New,
        Some(true) => UpdateReplay::Replay,
        Some(false) => UpdateReplay::Mismatch,
    }
}

/// Remember `token` in `applied` as having applied the change `fingerprint`
/// describes, forgetting the oldest once the window is full ([`MAX_UPDATE_TOKENS`]).
pub fn remember(applied: &mut VecDeque<(String, String)>, token: String, fingerprint: String) {
    if applied.iter().any(|(seen, _)| seen == &token) {
        return;
    }
    if applied.len() >= MAX_UPDATE_TOKENS {
        applied.pop_front();
    }
    applied.push_back((token, fingerprint));
}

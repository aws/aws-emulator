// SPDX-License-Identifier: CC0-1.0

//! The Apache Airflow UI sessions a launch URL starts.
//!
//! `CreateAirflowUiUrl` "creates a launch URL that opens the Apache Airflow UI", and that
//! URL "starts a UI session that has this identifier" and the session "ends at
//! SessionExpiresAt" (`smithy_query(query="shape", shape="CreateAirflowUiUrlResponse")`).
//! So a call MINTS a session: the identifier it reports names a record kept HERE, with the
//! instant that session ends, and the record is removed once the clock reaches that instant
//! — the end the response already promised, materialised whether or not anybody calls
//! again ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::expire_airflow_ui_sessions`]).
//!
//! The launch URL's own validity is not kept here: a URL that leads back to this emulator
//! is query-signed for exactly as long as it may be opened, so the signature itself is what
//! stops being valid at `UrlExpiresAt`.
//!
//! The session store is BOUNDED ([`MAX_SESSIONS`]): sessions are caller-driven growth, and
//! one holds nothing of the request, so beyond this many live sessions the one ending
//! soonest is dropped to make room — indistinguishable, for a client, from a session that
//! had already ended.

use std::time::{Duration, SystemTime};

use super::{digest, ComAmazonawsMwaaserverlessAmazonmwaaserverlessState};

/// How long the launch URL stays valid when the request names no `ExpiresInSeconds`:
/// "The default is 300" (`smithy_query(query="doc", shape="CreateAirflowUiUrlRequest",
/// member="ExpiresInSeconds")`), in seconds.
pub const DEFAULT_URL_VALIDITY: Duration = Duration::from_secs(300);

/// How long the UI session lasts when the request names no `SessionDurationInSeconds`:
/// "The default is 43200 (12 hours)" (`smithy_query(query="doc",
/// shape="CreateAirflowUiUrlRequest", member="SessionDurationInSeconds")`), in seconds.
pub const DEFAULT_SESSION_DURATION: Duration = Duration::from_secs(43_200);

/// How many UI sessions this emulator holds at once.
///
/// Not a fidelity claim about any published quota: it is the emulator's own ceiling, set
/// well above the handful of sessions a caller or a test opens, so a caller looping on
/// `CreateAirflowUiUrl` cannot grow this process's memory without bound.
pub const MAX_SESSIONS: usize = 256;

/// One UI session a launch URL starts.
#[derive(Debug, Clone)]
pub struct AirflowUiSession {
    /// The `SessionId` the response reported, and the key this record is held under.
    pub session_id: String,
    /// The endpoint the session was opened through: "The UI shows the workflows in your
    /// account in the current AWS Region", so a session belongs to one region exactly as
    /// a workflow does.
    pub region: String,
    /// When the session was opened — the instant the call that minted it took the store
    /// at.
    pub created_at: SystemTime,
    /// When it ends: the `SessionExpiresAt` the response reported.
    pub expires_at: SystemTime,
}

/// Mint a `SessionId`: 32 lowercase hex characters, per the modelled `^[a-z0-9]{32}$`
/// (`smithy_query(query="shape", shape="SessionIdString")`).
///
/// Derived from the endpoint the session is opened through and the store's own mint nonce,
/// so two sessions never share an identifier.
pub fn mint_session_id(region: &str, nonce: &str) -> String {
    digest(&["airflow-ui-session", region, nonce])
        .chars()
        .take(32)
        .collect()
}

/// The path, on this emulator, the launch URL of `session_id` addresses.
///
/// Decoded, as a signed path is laid out; a `SessionId` is 32 lowercase alphanumerics, so
/// it carries no separator of its own and the path has one segment per part.
pub fn session_path(session_id: &str) -> String {
    format!("/airflow-ui/sessions/{session_id}")
}

impl ComAmazonawsMwaaserverlessAmazonmwaaserverlessState {
    /// Record the session a launch URL starts, and answer the stored record.
    ///
    /// Beyond [`MAX_SESSIONS`] live sessions the one ending soonest is dropped first, so
    /// the store keeps the sessions with the most life left in them.
    pub fn open_airflow_ui_session(
        &mut self,
        session_id: String,
        region: &str,
        created_at: SystemTime,
        duration: Duration,
    ) -> AirflowUiSession {
        while self.airflow_ui_sessions.len() >= MAX_SESSIONS {
            let Some(soonest) = self
                .airflow_ui_sessions
                .values()
                .min_by_key(|session| (session.expires_at, session.session_id.clone()))
                .map(|session| session.session_id.clone())
            else {
                break;
            };
            self.airflow_ui_sessions.remove(&soonest);
        }
        let session = AirflowUiSession {
            session_id: session_id.clone(),
            region: region.to_string(),
            created_at,
            // A duration no instant can represent ends the session at the instant it
            // opened: there is no later one to report.
            expires_at: created_at.checked_add(duration).unwrap_or(created_at),
        };
        self.airflow_ui_sessions.insert(session_id, session.clone());
        session
    }

    /// The session `session_id` names AS SEEN FROM `region`'s endpoint, and `None`
    /// otherwise — the same region scoping a workflow lookup has.
    pub fn airflow_ui_session_in(
        &self,
        region: &str,
        session_id: &str,
    ) -> Option<&AirflowUiSession> {
        self.airflow_ui_sessions
            .get(session_id)
            .filter(|session| session.region == region)
    }

    /// End every UI session whose `SessionExpiresAt` the clock has already passed, and
    /// report whether anything was removed.
    ///
    /// One of the writes a reporting operation may make: a session the response said ends
    /// at that instant was over then whether or not anybody called, so what the store
    /// holds is what it would hold unpolled.
    pub fn expire_airflow_ui_sessions(&mut self, now: SystemTime) -> bool {
        let before = self.airflow_ui_sessions.len();
        self.airflow_ui_sessions
            .retain(|_, session| now < session.expires_at);
        self.airflow_ui_sessions.len() != before
    }
}

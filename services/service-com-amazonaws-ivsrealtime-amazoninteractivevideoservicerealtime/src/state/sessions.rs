// SPDX-License-Identifier: CC0-1.0

//! The stage SESSIONS a stage's participants are grouped into: the session and
//! participant records, the occurrences this service records during a session, the
//! replications a publishing participant is replicated by, and the joins, departures and
//! clock-owed ends that move them.

use super::*;

/// `EventName` of a participant joining the session — one of the twelve values the
/// modelled `EventName` carries (`smithy_query(query="shape",shape="EventName")`), and
/// the occurrence [`join_stage_session`] records.
pub const EVENT_JOINED: &str = "JOINED";

/// `EventName` of a participant leaving the session, the counterpart of
/// [`EVENT_JOINED`] and what [`leave_stage_session`] records.
pub const EVENT_LEFT: &str = "LEFT";

/// `EventName` of a participant starting to publish to the session.
///
/// A join that publishes is a join AND a publish — `ParticipantSummary$published` is
/// "whether the participant ever published" — so the publisher's first media is its own
/// occurrence beside the join's.
pub const EVENT_PUBLISH_STARTED: &str = "PUBLISH_STARTED";

/// `EventName` of a publishing participant's media ending, the counterpart of
/// [`EVENT_PUBLISH_STARTED`]: a publisher that leaves the session stops publishing to
/// it, so the two names alternate for one participant
/// ([`StageSessionRecord::is_publishing`]).
pub const EVENT_PUBLISH_STOPPED: &str = "PUBLISH_STOPPED";

/// `EventName` of a publishing participant beginning to be replicated into another
/// stage, which is what `StartParticipantReplication` does. The two members the model
/// marks "applicable only if the event name is `REPLICATION_STARTED` or
/// `REPLICATION_STOPPED`" (`destinationStageArn`, `destinationSessionId`) are carried by
/// exactly this event.
pub const EVENT_REPLICATION_STARTED: &str = "REPLICATION_STARTED";

/// `EventName` of that replication ending, which is what `StopParticipantReplication`
/// does — the counterpart of [`EVENT_REPLICATION_STARTED`] and the second name the two
/// destination members apply to.
pub const EVENT_REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";

/// One occurrence during one stage session, as the data-plane seam that observed it
/// recorded it ([`record_participant_event`]) and `ListParticipantEvents` reports it.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// One of the modelled `EventName` values — the `EVENT_*` constants above.
    pub name: String,
    /// The participant the occurrence happened to: "the participant who triggered the
    /// event", as `Event$participantId` states, and the id
    /// `ListParticipantEvents` selects an event by.
    pub participant_id: String,
    /// When it occurred.
    pub event_time: SystemTime,
    /// The other participant the occurrence involves — "for a subscribe event, this is
    /// the publisher. For a publish or join event, this is null", as
    /// `Event$remoteParticipantId` states.
    pub remote_participant_id: Option<String>,
    /// The modelled `EventErrorCode` of an error event; "if the event is not an error
    /// event, this field is null", as `Event$errorCode` states.
    pub error_code: Option<String>,
    /// Where the participant is replicated, carried only by a replication event.
    pub destination_stage_arn: Option<String>,
    /// The session of that destination stage, carried only by a replication event.
    pub destination_session_id: Option<String>,
    /// Whether [`Self::participant_id`] is a replicated participant.
    pub replica: bool,
    /// Occurrence order within the session, and the stable key `ListParticipantEvents`
    /// pages over — the same role [`ParticipantRecord::sequence`] plays for
    /// `ListParticipants`.
    pub sequence: u64,
}

impl EventRecord {
    /// The modelled `Event` this record reports in a `ListParticipantEvents` page.
    ///
    /// `previousToken` and `newToken` describe a `TOKEN_EXCHANGED` event ("Source
    /// participant token for `TOKEN_EXCHANGED` event" / "Participant token created
    /// during `TOKEN_EXCHANGED` event"), which no occurrence of this emulator is, so
    /// they stay absent rather than reporting a token exchange that did not happen.
    pub fn to_event(&self) -> Event {
        Event {
            name: Some(self.name.clone()),
            participant_id: Some(self.participant_id.clone()),
            event_time: Some(::aws_smithy_types::DateTime::from(self.event_time)),
            remote_participant_id: self.remote_participant_id.clone(),
            error_code: self.error_code.clone(),
            destination_stage_arn: self.destination_stage_arn.clone(),
            destination_session_id: self.destination_session_id.clone(),
            replica: self.replica,
            previous_token: None,
            new_token: None,
        }
    }
}

/// What a data-plane seam observed occurring: the modelled name of the occurrence, the
/// participant it happened to, and only those members that name applies to.
///
/// Everything else an [`EventRecord`] holds — when it occurred and where it sits in the
/// session's occurrence order — is [`record_participant_event`]'s own doing, so no two
/// callers can record an occurrence differently.
#[derive(Debug, Default, Clone)]
pub struct ParticipantEvent {
    pub name: String,
    pub participant_id: String,
    pub remote_participant_id: Option<String>,
    pub error_code: Option<String>,
    pub destination_stage_arn: Option<String>,
    pub destination_session_id: Option<String>,
    pub replica: bool,
}

impl ParticipantEvent {
    /// The occurrence `name` happening to `participant_id`, with every member that does
    /// not apply to that name left unset.
    pub fn new(name: &str, participant_id: &str) -> Self {
        Self {
            name: name.to_string(),
            participant_id: participant_id.to_string(),
            ..Default::default()
        }
    }
}

/// Record `event` as having occurred at `at` in the session `session_id` of `stage_arn`.
///
/// The ONE place an event record is created, so every seam that observes an occurrence
/// writes the same representation of it: the instant is the one it occurred at — which is
/// `at`, supplied by the seam that observed it rather than read off the wall clock here,
/// so an occurrence the clock owed at a deadline is stamped with that deadline and not
/// with the call that happens to write it down — and the position is the session's next
/// occurrence position, which is what keeps a page of events in occurrence order.
///
/// `at` is the instant the occurrence HAPPENED, which for one a caller performs is that
/// call and for one the clock owed is the instant it fell due — never the later call that
/// writes it down — so what a reader is told about when something happened does not depend
/// on when something else looked.
///
/// `false` — nothing recorded — when this store holds no such session, or when the
/// session is at [`MAX_EVENTS_PER_SESSION`] events.
pub fn record_participant_event(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    event: ParticipantEvent,
    at: SystemTime,
) -> bool {
    if state
        .stage_session(stage_arn, session_id)
        .is_none_or(|session| session.events.len() >= MAX_EVENTS_PER_SESSION)
    {
        return false;
    }
    let sequence = state.next_sequence();
    let Some(session) = session_mut(state, stage_arn, session_id) else {
        return false;
    };
    session.events.push(EventRecord {
        name: event.name,
        participant_id: event.participant_id,
        event_time: at,
        remote_participant_id: event.remote_participant_id,
        error_code: event.error_code,
        destination_stage_arn: event.destination_stage_arn,
        destination_session_id: event.destination_session_id,
        replica: event.replica,
        sequence,
    });
    true
}

/// One started replication of one publishing participant into one destination stage, as
/// `StartParticipantReplication` persisted it and `ListParticipantReplicas` reports it.
#[derive(Debug, Clone)]
pub struct ParticipantReplicaRecord {
    /// The stage the replicated participant publishes in.
    pub source_stage_arn: String,
    /// The session of [`Self::source_stage_arn`] the publisher is in.
    pub source_session_id: String,
    /// The stage the participant is replicated into.
    pub destination_stage_arn: String,
    /// The session of [`Self::destination_stage_arn`] the replica joined.
    pub destination_session_id: String,
    /// The publisher being replicated — a participant of the source stage.
    pub participant_id: String,
    /// [`REPLICATION_STATE_ACTIVE`] while the replication is running.
    pub replication_state: String,
    /// The window a disconnected publisher may reconnect in before replication ends,
    /// or [`DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS`] when the request omitted it.
    /// It is the span the clock measures [`Self::reconnect_deadline`] with, which is what
    /// the caller asked replication to do.
    pub reconnect_window_seconds: i32,
    /// The instant the clock ends this replication at, because the publisher it
    /// replicates has left the session it publishes in and its reconnect window runs out
    /// then — `None` while that publisher is connected, which is every replication of a
    /// publisher that has not gone away.
    ///
    /// Written only by [`observe_replication_publisher`], the one place a publisher's
    /// connection is observed, and read only by [`materialize_due_replications`], the
    /// clock that acts on it.
    pub reconnect_deadline: Option<SystemTime>,
    /// The attributes the replica participant carries in the destination stage: the
    /// source participant's own, with the starting request's merged over them — the rule
    /// `StartParticipantReplicationRequest$attributes` states ("These attributes are
    /// merged with any attributes set for this participant when creating the token. If
    /// there is overlap in keys, the values in these attributes are replaced."). No
    /// member of `ParticipantReplica` reports them; the participant operations
    /// (`GetParticipant` / `ListParticipants`) are what report a replica participant's
    /// attributes.
    pub attributes: HashMap<String, String>,
    /// When replication started. Reported by no member of `ParticipantReplica`, and
    /// recorded for the same reason [`StageRecord::created_at`] is.
    pub started_at: SystemTime,
    /// Start order, and the stable key `ListParticipantReplicas` pages over — the same
    /// role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl ParticipantReplicaRecord {
    /// Move this replication to its stopped state.
    ///
    /// The ONE place a replication stops, whichever way it ended — a caller's
    /// `StopParticipantReplication`, a stage it names going away
    /// ([`end_replications_of_stage`]), or its publisher staying away for its whole
    /// reconnect window ([`materialize_due_replications`]) — so every end leaves the same
    /// record: [`REPLICATION_STATE_STOPPED`], which is what every reader reports from here
    /// on, and no deadline, because a replication that has ended is owed no further end.
    pub fn stop(&mut self) {
        self.replication_state = REPLICATION_STATE_STOPPED.to_string();
        self.reconnect_deadline = None;
    }

    /// Whether this replication is still running ([`REPLICATION_STATE_ACTIVE`]).
    pub fn is_active(&self) -> bool {
        self.replication_state == REPLICATION_STATE_ACTIVE
    }

    /// The modelled `ParticipantReplica` this record reports. Every member of the shape
    /// is `@required`, so each one is reported from the record and none is optional.
    pub fn to_participant_replica(&self) -> ParticipantReplica {
        ParticipantReplica {
            source_stage_arn: self.source_stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            source_session_id: self.source_session_id.clone(),
            destination_stage_arn: self.destination_stage_arn.clone(),
            destination_session_id: self.destination_session_id.clone(),
            replication_state: self.replication_state.clone(),
        }
    }
}

/// One stage session: the stretch of a stage's life its participants are grouped
/// into.
///
/// The model's `StageSession` doc fixes what one is: "A stage session begins when the
/// first participant joins a stage and ends after the last participant leaves the
/// stage." So a session is not created or deleted by an API call — [`join_stage_session`]
/// opens one when a stage has none, and [`leave_stage_session`] closes it when the last
/// participant has gone.
#[derive(Debug, Clone)]
pub struct StageSessionRecord {
    /// The modelled `StageSessionId` ([`stage_session_id`]), and the key
    /// `ListParticipants` resolves the session by.
    pub session_id: String,
    /// When the first participant joined.
    pub start_time: SystemTime,
    /// When the last participant left, or `None` while the session is active — "This
    /// is null if the stage is active", as `StageSession$endTime` states.
    pub end_time: Option<SystemTime>,
    /// Every participant that joined this session, in join order, the ones that have
    /// left included.
    pub participants: Vec<ParticipantRecord>,
    /// Every occurrence during this session, in occurrence order — what
    /// `ListParticipantEvents` reports. Written only by
    /// [`record_participant_event`].
    pub events: Vec<EventRecord>,
    /// Start order, the key a page over the sessions of one stage resumes from.
    pub sequence: u64,
}

impl StageSessionRecord {
    /// The modelled `StageSessionSummary` this record reports in a `ListStageSessions`
    /// page.
    ///
    /// `endTime` is reported only once the session has ended — "This is null if the
    /// stage is active", as `StageSessionSummary$endTime` states — which is exactly
    /// what [`Self::end_time`] holds.
    pub fn to_summary(&self) -> StageSessionSummary {
        StageSessionSummary {
            session_id: Some(self.session_id.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// The modelled `StageSession` this record reports when `GetStageSession` names it.
    ///
    /// Every member of `StageSession` is the same member of `StageSessionSummary` with
    /// the same doc, so the two are reported from one place and `GetStageSession` and
    /// `ListStageSessions` cannot disagree about a session: `endTime` is reported only
    /// once the session has ended ("This is null if the stage is active", as
    /// `StageSession$endTime` states).
    pub fn to_stage_session(&self) -> StageSession {
        let summary = self.to_summary();
        StageSession {
            session_id: summary.session_id,
            start_time: summary.start_time,
            end_time: summary.end_time,
        }
    }

    /// This session's participants in join order — the order `ListParticipants` pages
    /// over, so a record added or left mid-pagination cannot shift a later page onto
    /// an already-returned one.
    pub fn participants_in_join_order(&self) -> Vec<&ParticipantRecord> {
        let mut participants: Vec<&ParticipantRecord> = self.participants.iter().collect();
        participants.sort_by_key(|record| record.sequence);
        participants
    }

    /// This session's occurrences in the order they happened — the order
    /// `ListParticipantEvents` pages over, so an occurrence recorded mid-pagination
    /// cannot shift a later page onto an already-returned one.
    ///
    /// The stored order IS that order: the one writer
    /// ([`record_participant_event`]) only ever appends, under a strictly increasing
    /// position, so no ordering copy is needed.
    pub fn events_in_occurrence_order(&self) -> &[EventRecord] {
        &self.events
    }

    /// Whether `participant_id` is publishing to this session right now.
    ///
    /// Read off the occurrences themselves: [`EVENT_PUBLISH_STARTED`] and
    /// [`EVENT_PUBLISH_STOPPED`] are what begin and end one participant's media, so the
    /// later of the two is its publishing state and the two alternate rather than
    /// repeating. [`ParticipantRecord::published`] cannot answer this — it is "whether
    /// the participant EVER published", which stays set once set.
    pub fn is_publishing(&self, participant_id: &str) -> bool {
        // Over `self.events` directly, and from the back: the one writer
        // ([`record_participant_event`]) only ever appends, under an increasing
        // position, so the last matching record is the latest one and no ordering copy
        // is needed to find it.
        self.events
            .iter()
            .rfind(|event| {
                event.participant_id == participant_id
                    && (event.name == EVENT_PUBLISH_STARTED || event.name == EVENT_PUBLISH_STOPPED)
            })
            .is_some_and(|event| event.name == EVENT_PUBLISH_STARTED)
    }
}

/// What a stage's data plane observes about one participant joining a stage: the
/// identity its token or ingest credential carries, and what it does in the stage.
///
/// Everything else a [`ParticipantRecord`] holds is the join's own doing and is
/// applied by [`join_stage_session`], so no two callers can record a join
/// differently.
#[derive(Debug, Default, Clone)]
pub struct JoinedParticipant {
    /// The id the joining client's participant token carries, or the one its ingest
    /// credential publishes as. Both are minted by this service
    /// ([`issue_token_record`], [`mint_ingest_credential`]).
    pub participant_id: String,
    pub user_id: Option<String>,
    pub attributes: HashMap<String, String>,
    /// Whether this participant is publishing media, rather than only subscribing.
    pub published: bool,
    /// What the recorder reports for this participant, when something records it.
    /// `None` is [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`].
    pub recording_state: Option<String>,
    /// Whether the publisher reached the stage over redundant ingest.
    pub redundant_ingest: bool,
    /// The ingest configuration it broadcast with, when it used one.
    pub ingest_configuration_arn: Option<String>,
    /// Whether this participant IS a replica of a participant publishing in another
    /// stage, which is what `Event$replica` reports of the occurrences it is party to
    /// ("If true, this indicates the `participantId` is a replicated participant").
    /// What the participant itself reports about that replication is read off the
    /// replication ([`session_participant_replications`]), not from here.
    pub replica: bool,
    /// What the data plane observed about the joining client; see
    /// [`ParticipantClient`]. Nothing observed is the default.
    pub client: ParticipantClient,
    /// The broadcast protocol it was seen publishing with, when the data plane saw one
    /// — one of the values the modelled `ParticipantProtocol` carries. `None` leaves
    /// [`participant_protocol`] to resolve it.
    pub protocol: Option<String>,
}

/// Record the participant `joined` describes joining `stage_arn`, opening a session
/// for the stage when no session is active, and return the record the session now
/// holds for it.
///
/// The ONE place a participant record is created, so every operation that reports one
/// answers the same representation: a joining participant is
/// [`PARTICIPANT_STATE_CONNECTED`], its `firstJoinTime` is the instant it joined, and it
/// is recorded with the recording state the recorder reports (or
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]). What it reports about replication is
/// not recorded here: it is read off the replications this store holds
/// ([`session_participant_replications`]).
///
/// A participant the session already holds REJOINS it: the model documents
/// `firstJoinTime` as the first join and `published` as whether the participant ever
/// published, so the record keeps its id, its first join instant and its page
/// position, comes back to `CONNECTED`, and keeps `published` once set. Everything else
/// the join supplies is what the record reports from this join on — including the
/// identity it carries, its user id and its attributes — because a record reports the
/// connection it holds, not the one before it.
///
/// The session itself is opened by [`ensure_active_session`], the one place a session
/// of this service begins, so a participant that joins and a replica that is
/// replicated in are in the SAME session of the stage.
///
/// The join is also an occurrence of the session ([`EVENT_JOINED`]), and a join that
/// publishes is a publish occurrence beside it ([`EVENT_PUBLISH_STARTED`]), so
/// `ListParticipantEvents` reports what this recorded and nothing else.
///
/// `None` — nothing recorded — when this store holds no such stage, when the stage is
/// at [`MAX_STAGE_SESSIONS_PER_STAGE`] sessions, or when the session is at
/// [`MAX_PARTICIPANTS_PER_SESSION`] participants.
pub fn join_stage_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    joined: JoinedParticipant,
) -> Option<ParticipantRecord> {
    // One instant for the whole join, so the record, its occurrences and what the
    // replication and composition clocks are told all agree about when it happened.
    let now = SystemTime::now();
    // The session the stage is in, or a fresh one: "a stage session begins when the
    // first participant joins a stage".
    let session_id = ensure_active_session(state, stage_arn, now)?;
    // Read off before the join is applied, because applying it consumes `joined`.
    let participant_id = joined.participant_id.clone();
    let publishes = joined.published;
    let replica = joined.replica;

    // A participant the session already holds is REJOINING it, so it keeps its id, its
    // first join instant and its page position.
    let rejoined = {
        let session = session_mut(state, stage_arn, &session_id)?;
        if let Some(rejoined) = session
            .participants
            .iter_mut()
            .find(|participant| participant.participant_id == joined.participant_id)
        {
            rejoined.state = PARTICIPANT_STATE_CONNECTED.to_string();
            rejoined.published = rejoined.published || joined.published;
            // The identity this join carries is what the record reports from now on:
            // `attributes` is a member every join supplies, so the map it brings is the
            // participant's whole set of attributes — an empty one means this join
            // carries none, never that an earlier join's are kept — and a `user_id` the
            // join names replaces the earlier one under the optional-member rule the
            // rest of this block follows.
            rejoined.attributes = joined.attributes.clone();
            if joined.user_id.is_some() {
                rejoined.user_id = joined.user_id.clone();
            }
            if let Some(recording_state) = joined.recording_state.clone() {
                rejoined.recording_state = recording_state;
            }
            rejoined.redundant_ingest = joined.redundant_ingest;
            if joined.ingest_configuration_arn.is_some() {
                rejoined.ingest_configuration_arn = joined.ingest_configuration_arn.clone();
            }
            // A rejoin is a fresh connection, so what the data plane observes about it
            // — the client it came from and the protocol it publishes with — is what
            // the participant carries from now on. A member this connection did not
            // observe leaves the earlier observation standing, the rule every other
            // optional member of this block follows
            // ([`ParticipantClient::observe`]).
            rejoined.client.observe(&joined.client);
            if joined.protocol.is_some() {
                rejoined.protocol = joined.protocol.clone();
            }
            Some(rejoined.clone())
        } else {
            // Bound the store this join grows: the records are reclaimed only with the
            // stage.
            if session.participants.len() >= MAX_PARTICIPANTS_PER_SESSION {
                return None;
            }
            None
        }
    };

    let record = match rejoined {
        Some(record) => record,
        None => new_participant_record(state, stage_arn, &session_id, joined, now)?,
    };

    // Both a first join and a rejoin are a participant joining the session, so each is
    // recorded; a publish is recorded only where the media actually begins, which a
    // rejoin of a participant already publishing is not.
    //
    // Every occurrence of a join says whether the participant it happened to is a replica
    // of a publisher in another stage, which is what `Event$replica` reports.
    let occurrence = |name: &str| {
        let mut event = ParticipantEvent::new(name, &participant_id);
        event.replica = replica;
        event
    };
    record_participant_event(state, stage_arn, &session_id, occurrence(EVENT_JOINED), now);
    if publishes
        && !state
            .stage_session(stage_arn, &session_id)
            .is_some_and(|session| session.is_publishing(&participant_id))
    {
        record_participant_event(
            state,
            stage_arn,
            &session_id,
            occurrence(EVENT_PUBLISH_STARTED),
            now,
        );
    }
    // A publisher that is back is publishing again, so every replication it sources is
    // owed no end: "if the participant disconnects and then reconnects within the
    // specified interval, replication will continue to be `ACTIVE`", through the one place
    // a publisher's connection is observed.
    observe_replication_publisher(state, stage_arn, &session_id, &participant_id, now);
    // The stage now has somebody in it, so no composition of it is counting out the
    // empty-stage timeout the model's `StartComposition` doc ends a composition on.
    state.observe_stage_occupancy(stage_arn, now);
    Some(record)
}

/// Put the participant `joined` describes into the session `session_id` of `stage_arn`
/// as one NEW to it, and return the record the session now holds.
///
/// Split out of [`join_stage_session`] so the join's two paths — a rejoin and a first
/// join — meet again at one place that records the occurrence, rather than one of them
/// returning early.
pub(super) fn new_participant_record(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    joined: JoinedParticipant,
    at: SystemTime,
) -> Option<ParticipantRecord> {
    // Only a participant that is NEW to the session takes a page position, so the join
    // order of a session's participants stays contiguous.
    let sequence = state.next_sequence();
    let record = ParticipantRecord {
        participant_id: joined.participant_id,
        user_id: joined.user_id,
        attributes: joined.attributes,
        state: PARTICIPANT_STATE_CONNECTED.to_string(),
        first_join_time: at,
        published: joined.published,
        recording_state: joined
            .recording_state
            .unwrap_or_else(|| NOT_RECORDED_PARTICIPANT_RECORDING_STATE.to_string()),
        redundant_ingest: joined.redundant_ingest,
        ingest_configuration_arn: joined.ingest_configuration_arn,
        client: joined.client,
        protocol: joined.protocol,
        sequence,
    };
    session_mut(state, stage_arn, session_id)?
        .participants
        .push(record.clone());
    Some(record)
}

/// The session `session_id` of the stage `stage_arn`, mutably: the one lookup
/// [`join_stage_session`] and [`leave_stage_session`] resolve a session to write
/// through, keyed exactly as the reading
/// [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::stage_session`]
/// is.
pub(super) fn session_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
) -> Option<&'a mut StageSessionRecord> {
    state
        .stage_sessions
        .get_mut(stage_arn)?
        .iter_mut()
        .find(|session| session.session_id == session_id)
}

/// Record the participant `participant_id` leaving the session `session_id` of
/// `stage_arn` NOW, and close that session when it was the last one in it.
///
/// The counterpart of [`join_stage_session`], and the one place a participant leaves:
/// the record stays in the session and moves to
/// [`PARTICIPANT_STATE_DISCONNECTED`] — `ListParticipants` reports the participants
/// of a session, which the ones that have left are — and when every participant has
/// left, the session "ends after the last participant leaves the stage": its
/// `endTime` is set and the stage reports no active session again, so the next join
/// begins a new session.
///
/// The departure is also an occurrence of the session ([`EVENT_LEFT`]), preceded by the
/// end of its media ([`EVENT_PUBLISH_STOPPED`]) when the participant was publishing.
///
/// A departure that happened EARLIER than this call — one the clock owed — is recorded
/// through [`leave_stage_session_at`], which this is the "it happened now" case of.
///
/// `false` — nothing recorded — when this store holds no such stage, session or
/// participant, or when that participant has already left.
pub fn leave_stage_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> bool {
    leave_stage_session_at(
        state,
        stage_arn,
        session_id,
        participant_id,
        SystemTime::now(),
    )
}

/// [`leave_stage_session`] for a departure that happened at `at` rather than now.
///
/// A departure the CLOCK owed — the replica of a replication whose publisher stayed away
/// for its whole reconnect window ([`materialize_due_replications`]) — happened at the
/// instant it fell due, whether or not anybody called then, so everything it decides is
/// measured from THAT instant: the session's end, the occurrences it records, the reconnect
/// deadline it earns a replication the departing participant sources, and the empty-stage
/// deadline it earns every composition of the stage. Writing it down late therefore leaves
/// exactly the store an on-time materialisation would have left, which is what keeps a read
/// from changing what a later caller sees.
pub fn leave_stage_session_at(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    at: SystemTime,
) -> bool {
    let session_ended = {
        let Some(session) = session_mut(state, stage_arn, session_id) else {
            return false;
        };
        let Some(leaving) = session
            .participants
            .iter_mut()
            .find(|participant| participant.participant_id == participant_id)
        else {
            return false;
        };
        if leaving.state == PARTICIPANT_STATE_DISCONNECTED {
            return false;
        }
        leaving.state = PARTICIPANT_STATE_DISCONNECTED.to_string();
        let emptied = session
            .participants
            .iter()
            .all(|participant| participant.state == PARTICIPANT_STATE_DISCONNECTED);
        if emptied {
            session.end_time = Some(at);
        }
        emptied
    };
    // Every occurrence says whether the participant it happened to is a replica of a
    // publisher in another stage, which is what `Event$replica` reports. That is a fact
    // about the replications this store holds rather than about the departure, so it is
    // read off them ([`session_participant_replications`]) exactly as every other report
    // of it is — the same statement the join of that replica recorded.
    let replica = session_participant_replications(state, stage_arn, session_id)
        .get(participant_id)
        .is_some_and(|replication| replication.replication_type == REPLICATION_TYPE_REPLICA);
    let occurrence = |name: &str| {
        let mut event = ParticipantEvent::new(name, participant_id);
        event.replica = replica;
        event
    };
    // A participant that is publishing when it goes stops publishing first: the two
    // publish names alternate ([`StageSessionRecord::is_publishing`]), and the departure
    // is the last occurrence either way.
    if state
        .stage_session(stage_arn, session_id)
        .is_some_and(|session| session.is_publishing(participant_id))
    {
        record_participant_event(
            state,
            stage_arn,
            session_id,
            occurrence(EVENT_PUBLISH_STOPPED),
            at,
        );
    }
    record_participant_event(state, stage_arn, session_id, occurrence(EVENT_LEFT), at);
    if session_ended {
        if let Some(stage) = state.stages.get_mut(stage_arn) {
            if stage.active_session_id.as_deref() == Some(session_id) {
                stage.active_session_id = None;
            }
        }
    }
    // A replication replicates a PUBLISHING participant, so a publisher leaving puts every
    // replication it sources from this session on the reconnect deadline its own window
    // earns it, through the one place a publisher's connection is observed. The end itself
    // is the clock's ([`materialize_due_replications`]), so this records a deadline and
    // nothing else.
    observe_replication_publisher(state, stage_arn, session_id, participant_id, at);
    // A departure can leave the stage with no participant in it at all, which is when the
    // timeout the model's `StartComposition` doc ends a composition on starts running for
    // every composition of that stage ("After a 1-minute timeout, when all participants
    // are disconnected from the stage") — measured from the instant the stage was actually
    // left empty, which is this departure's.
    state.observe_stage_occupancy(stage_arn, at);
    true
}

/// Record the end of a now-ENDED replication, which happened AT `at`, as an occurrence of
/// the session its replicated publisher is in.
///
/// The ONE place the end of a replication is written into the SOURCE session, whichever way
/// that replication ended — a caller stopping it, or a stage it names going away — so the
/// session that recorded [`EVENT_REPLICATION_STARTED`] when the replication began always
/// records the counterpart when it ends, and a reader of that session's occurrences is told
/// why the replication it saw start is no longer running. The model marks
/// `Event$destinationStageArn` and `Event$destinationSessionId` "applicable only if the
/// event name is REPLICATION_STARTED or REPLICATION_STOPPED", so the occurrence carries the
/// destination the ended replication named. The publisher itself is not a replica — it
/// publishes in the source stage — so `replica` stays false.
///
/// `ended` must already have stopped being [`REPLICATION_STATE_ACTIVE`] in this store (the
/// caller has stopped it, or is about to drop its record), so that no replication still
/// running is reported as having ended. `at` is the instant it ended — a caller's stop, the
/// delete of a stage it names, or the deadline the clock ran out on it.
///
/// `false` — nothing recorded — when this store no longer holds that source session, which
/// is what the SOURCE stage of the replication being the one going away leaves.
pub fn record_replication_stopped(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ended: &ParticipantReplicaRecord,
    at: SystemTime,
) -> bool {
    let mut event = ParticipantEvent::new(EVENT_REPLICATION_STOPPED, &ended.participant_id);
    event.destination_stage_arn = Some(ended.destination_stage_arn.clone());
    event.destination_session_id = Some(ended.destination_session_id.clone());
    record_participant_event(
        state,
        &ended.source_stage_arn,
        &ended.source_session_id,
        event,
        at,
    )
}

/// Take the replica a now-ENDED replication put into its destination stage back out of
/// that stage's session, as of `at` — the instant that replication ended.
///
/// The ONE place an ended replication's replica leaves, whichever way the replication
/// ended — a caller stopping it, or a stage it names being deleted — so no participant
/// stays [`PARTICIPANT_STATE_CONNECTED`] in a stage on behalf of a replication this store
/// no longer runs. The departure itself is [`leave_stage_session`]: the record stays and is
/// reported [`PARTICIPANT_STATE_DISCONNECTED`], and the destination session ends when it
/// held nobody else. The publisher in the SOURCE stage is untouched — it is publishing
/// there whether or not anything replicates it.
///
/// `ended` must already have stopped being [`REPLICATION_STATE_ACTIVE`] in this store (the
/// caller has stopped it, or is about to drop its record), because the replica stays in the
/// destination stage while ANY replication still running puts that same participant into
/// that same session: only the last one to end takes the replica out. `at` is the instant
/// that last replication ended, which is the instant the replica left
/// ([`leave_stage_session_at`]).
///
/// `false` — nothing recorded — when another running replication still puts that
/// participant there, or when there is no such departure to record.
pub fn release_ended_replica(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ended: &ParticipantReplicaRecord,
    at: SystemTime,
) -> bool {
    let still_replicated = state.participant_replicas.values().flatten().any(|replica| {
        replica.participant_id == ended.participant_id
            && replica.destination_stage_arn == ended.destination_stage_arn
            && replica.destination_session_id == ended.destination_session_id
            && replica.is_active()
    });
    if still_replicated {
        return false;
    }
    leave_stage_session_at(
        state,
        &ended.destination_stage_arn,
        &ended.destination_session_id,
        &ended.participant_id,
        at,
    )
}

/// Tell the replications that `participant_id` SOURCES from the session `session_id` of
/// `stage_arn` what that publisher's connection is at `now`.
///
/// A replication replicates "a publishing participant", so it runs only while that
/// publisher is in the session it publishes in: the model states the rule on
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` — "If the participant
/// disconnects and then reconnects within the specified interval, replication will continue
/// to be `ACTIVE`" — so a publisher that has left owes each replication it sources its end
/// one window after it went, and the same publisher back inside that window takes that
/// deadline away again and the replication goes on running.
///
/// Called from the two places a participant's connection changes ([`join_stage_session`] and
/// [`leave_stage_session`]), so every way a publisher goes or comes back decides the
/// deadline in one place. It is the publisher of THAT session this answers about: a
/// replication names the source session its publisher publishes in, which
/// `ParticipantReplica$sourceSessionId` reports and which no later call changes, and a
/// session "ends after the last participant leaves the stage" — so a departure that ended
/// the session left no session to come back to and the end stands.
///
/// Recording the deadline is all this does — the end itself is the clock's, written down by
/// [`materialize_due_replications`], which is also what takes the replica out of the
/// destination stage — so this is never the call that ends a replication and cannot
/// re-enter the departure it is called from.
pub(super) fn observe_replication_publisher(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    now: SystemTime,
) {
    let connected = session_participant(state, stage_arn, session_id, participant_id)
        .is_some_and(|participant| participant.state == PARTICIPANT_STATE_CONNECTED);
    // The replications SOURCED from this stage are the ones this stage's key holds; a
    // replication INTO it is a replication of another stage's publisher, whose connection
    // this says nothing about.
    let Some(sourced) = state.participant_replicas.get_mut(stage_arn) else {
        return;
    };
    for replication in sourced.iter_mut().filter(|replication| {
        replication.participant_id == participant_id
            && replication.source_session_id == session_id
            && replication.is_active()
    }) {
        if connected {
            replication.reconnect_deadline = None;
        } else if replication.reconnect_deadline.is_none() {
            // The window is the one the start asked for, so a replication whose caller
            // asked for none is owed its end at the instant the publisher went.
            let window =
                Duration::from_secs(u64::try_from(replication.reconnect_window_seconds).unwrap_or(
                    u64::try_from(DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS).unwrap_or(0),
                ));
            replication.reconnect_deadline = Some(now + window);
        }
    }
}

/// Write down the end of every replication the clock has already made due at `now`.
///
/// A replication whose publisher has been gone for its whole reconnect window is over —
/// the rule the model states on `reconnectWindowSeconds`, read off the deadline
/// [`observe_replication_publisher`] recorded — and it ends exactly as a caller's stop ends
/// it: the record reaches its stopped state ([`ParticipantReplicaRecord::stop`]), the end is
/// an occurrence of the publisher's session ([`record_replication_stopped`]) and the replica
/// leaves the destination stage ([`release_ended_replica`]).
///
/// Each of those ends was owed at that deadline whether or not anybody called, so this
/// changes nothing a caller reading at the same instant would have seen differently — which
/// is why a reporting operation may call it, exactly as it may materialise what the clock
/// owes a composition
/// ([`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`]).
///
/// Every end is written down AS OF its own deadline rather than as of this call, the same
/// discipline [`CompositionRecord::materialize_due`] keeps: the consequences an end carries
/// — the session it closes, the occurrences it records, the empty-stage deadline it earns
/// every composition of the destination stage — all run out of that instant, so a store
/// materialised late holds exactly what one materialised on time held and no read can
/// change what a later caller sees.
pub fn materialize_due_replications(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    now: SystemTime,
) {
    // Ending one replication takes its replica out of the destination stage, and that
    // replica can itself be the publisher of a FURTHER replication sourced from that stage,
    // whose own deadline is then recorded — one window after the end that released it, so
    // still in the past of `now` — and can already be due, so the walk repeats until this
    // store holds no due replication. Every pass ends at least one of the finitely many
    // running replications, so it terminates.
    loop {
        // Each end beside the deadline it fell due at, which is the instant it happened.
        // It is carried here because stopping the record is what clears the deadline it is
        // read from ([`ParticipantReplicaRecord::stop`]).
        let mut due: Vec<(SystemTime, ParticipantReplicaRecord)> = Vec::new();
        for replication in state.participant_replicas.values_mut().flatten() {
            let owed = if replication.is_active() {
                replication
                    .reconnect_deadline
                    .filter(|deadline| *deadline <= now)
            } else {
                None
            };
            if let Some(deadline) = owed {
                replication.stop();
                due.push((deadline, replication.clone()));
            }
        }
        if due.is_empty() {
            return;
        }
        // In the order they ENDED, ties in start order, so what this writes down does not
        // depend on the order this store happens to hold its replications in: the
        // occurrences of one session are in occurrence order, the departures they cause are
        // applied in the order they happened, and two replications ending at the same
        // instant are recorded in the order they were started.
        due.sort_by_key(|(deadline, ended)| (*deadline, ended.sequence));
        // Each end is written over a store in which every replication ending in this pass
        // has ALREADY stopped, which is the premise both of these take: no replication
        // still running is reported as having ended, and a replica stays in the destination
        // stage while any other replication still puts it there.
        for (deadline, ended) in &due {
            record_replication_stopped(state, ended, *deadline);
        }
        for (deadline, ended) in &due {
            release_ended_replica(state, ended, *deadline);
        }
    }
}

/// End every replication the stage `stage_arn` is an end of AT `at`, and drop the records
/// of them.
///
/// A replica is a participant of its source stage replicated into its destination stage, so
/// neither end of a replication can outlive the stage it names: a stage going away ends
/// every replication it sources and every replication into it. Ending one is not just
/// dropping its record — the replica that replication put into the DESTINATION stage leaves
/// that stage's session, through the one place an ended replication's replica leaves
/// ([`release_ended_replica`]) — so the stage that survives this delete is left exactly as
/// it would be had each of those replications been stopped instead.
///
/// The replicas replicated INTO `stage_arn` need no departure: those records are records of
/// this stage's own sessions, which go with the stage. Their SOURCE session is what survives
/// that delete, and it is left as a stop would have left it: the end of each of those
/// replications is an occurrence of it ([`record_replication_stopped`]).
///
/// `at` is the instant the stage went away, which is when each of these replications ended.
pub fn end_replications_of_stage(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    at: SystemTime,
) {
    // Each replication this stage sources reaches its stopped state FIRST, as a caller
    // stopping it would leave it: that is what makes it no longer one of the running
    // replications [`release_ended_replica`] keeps a replica in a stage for, and it is
    // still the record every report of that departing participant reads its replication
    // members off.
    let mut sourced_here: Vec<ParticipantReplicaRecord> = Vec::new();
    if let Some(sourced) = state.participant_replicas.get_mut(stage_arn) {
        for replication in sourced.iter_mut().filter(|replication| replication.is_active()) {
            replication.stop();
            sourced_here.push(replication.clone());
        }
    }
    // Every replication INTO this stage ends with it too, and reaches its stopped state the
    // same way first. Only the ones still RUNNING end here: a replication a caller already
    // stopped recorded its end then.
    let mut into_here: Vec<ParticipantReplicaRecord> = Vec::new();
    for replications in state.participant_replicas.values_mut() {
        for replication in replications.iter_mut().filter(|replication| {
            replication.destination_stage_arn == stage_arn && replication.is_active()
        }) {
            replication.stop();
            into_here.push(replication.clone());
        }
    }
    for replication in sourced_here.iter().chain(into_here.iter()) {
        // The end of a replication is an occurrence of the session its publisher is in,
        // through the one place that is written — which records nothing for a replication
        // this stage SOURCED, whose source session went with the stage above.
        record_replication_stopped(state, replication, at);
    }
    for replication in &sourced_here {
        release_ended_replica(state, replication, at);
    }
    // The replications sourced from this stage go with its key; the ones replicated INTO it
    // are dropped from the stages that source them.
    state.participant_replicas.remove(stage_arn);
    for replicas in state.participant_replicas.values_mut() {
        replicas.retain(|replica| replica.destination_stage_arn != stage_arn);
    }
}

/// The sessions of the stage `stage_arn` in which `participant_id` is still
/// [`PARTICIPANT_STATE_CONNECTED`], in the order the stage holds them.
///
/// The ONE lookup for "where in this stage is this participant still present", beside the
/// [`leave_stage_session`] that acts on the answer, so an operation that takes a
/// participant out of a stage and the records `ListParticipants` reports for it cannot
/// disagree about which sessions it was in. Empty when this store holds no such stage, or
/// when every record it holds for that participant has already left.
pub fn connected_sessions_of(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Vec<String> {
    let Some(sessions) = state.stage_sessions.get(stage_arn) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter(|session| {
            session.participants.iter().any(|participant| {
                participant.participant_id == participant_id
                    && participant.state == PARTICIPANT_STATE_CONNECTED
            })
        })
        .map(|session| session.session_id.clone())
        .collect()
}

/// End every connection the stage `stage_arn` is holding for `participant_id` at `at`,
/// and return the sessions it was taken out of.
///
/// The ONE way a control-plane call ends a participant's connection to a stage, so the two
/// operations that do it — `DisconnectParticipant`, addressed to the participant, and
/// `RevokeConnectionToken`, addressed to the token it connected with — leave the same
/// store: every session of that stage the participant is still
/// [`PARTICIPANT_STATE_CONNECTED`] in ([`connected_sessions_of`]) records the departure
/// ([`leave_stage_session_at`]), so the record reports `DISCONNECTED` and a session the
/// departure empties ends, and what the clock then owes the replications that departure put
/// on their reconnect window is written down before the caller is answered
/// ([`materialize_due_replications`]).
///
/// Empty — and nothing written — for a participant that stage is holding no connection
/// for, which is what makes either call idempotent.
pub fn end_participant_connections(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
    at: SystemTime,
) -> Vec<String> {
    let connected = connected_sessions_of(state, stage_arn, participant_id);
    for session_id in &connected {
        leave_stage_session_at(state, stage_arn, session_id, participant_id, at);
    }
    // A replication replicates a PUBLISHING participant, so the departures above put every
    // replication this participant sources on the reconnect deadline its own window earns
    // it, and the end the clock already owes is written down here: with the window the model
    // documents as the default that end is owed at the instant the publisher went, so the
    // replication is reported stopped and its replica is out of the destination stage by the
    // time the caller is answered.
    materialize_due_replications(state, at);
    connected
}

/// The record the session `session_id` of `stage_arn` holds for `participant_id`, if it
/// holds one.
///
/// Keyed by all three, exactly as the reading
/// [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::stage_session`]
/// is: a participant is a participant OF one session of one stage.
pub(super) fn session_participant<'a>(
    state: &'a ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> Option<&'a ParticipantRecord> {
    state
        .stage_session(stage_arn, session_id)?
        .participants
        .iter()
        .find(|participant| participant.participant_id == participant_id)
}

/// The same lookup mutably, for the one write a caller makes to a participant already in
/// the session it names.
pub(super) fn session_participant_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> Option<&'a mut ParticipantRecord> {
    session_mut(state, stage_arn, session_id)?
        .participants
        .iter_mut()
        .find(|participant| participant.participant_id == participant_id)
}

/// Why the participants of a started replication could not both be recorded.
///
/// Each names the stage whose session could not take the record, so the operation that
/// asked can answer the modelled failure that condition is rather than one standing for
/// both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicatedParticipantRefusal {
    /// That stage's session is at [`MAX_PARTICIPANTS_PER_SESSION`] participants — this
    /// emulator's own resource guard on the records a session holds.
    SessionFull { stage_arn: String },
    /// This store holds no OPEN session of that id for that stage to record in.
    NoSuchSession { stage_arn: String },
}

/// Record the participants a started replication puts into the two sessions it names.
///
/// A replication replicates "a publishing participant from a source stage to a destination
/// stage", so the replicated publisher is a participant of BOTH sessions `replica` names:
/// the session it publishes in — which is why its stage has a session at all — and the
/// destination stage's session, where its replica is. Those are the records
/// `ListParticipants` reports, created through [`join_stage_session`], the one place a
/// participant record is created, and each of the two publishes: the participant being
/// replicated is a publisher, and its replica carries that same media into the destination
/// stage.
///
/// A session that already holds the participant as [`PARTICIPANT_STATE_CONNECTED`] keeps
/// the record it has, beyond its having published: that participant is in the stage, and a
/// replication starting is not it joining again. Where that record had not published yet,
/// the media this replication carries beginning is an occurrence of the session like any
/// other publish ([`EVENT_PUBLISH_STARTED`]). A record that has LEFT the session is not in
/// the stage any more, so a replication of that participant IS it joining again, recorded
/// the way any rejoin is ([`join_stage_session`]), and its state and the session's
/// occurrences stay in step.
///
/// What each record REPORTS about the replication is not written here — it is read off the
/// replication itself ([`session_participant_replications`]) — so `identity` supplies only what
/// issued the participant: the name its token or ingest credential carries, and the
/// attributes a replica's own are merged over.
///
/// `Err` — nothing recorded at either end — naming the stage and the condition
/// ([`ReplicatedParticipantRefusal`]): a stage with no OPEN session of that id to record
/// in, or one whose session is at [`MAX_PARTICIPANTS_PER_SESSION`] participants. Both ends
/// are checked before either is written, so one stage cannot end up holding a participant
/// the other does not, and the session checked is the session written.
pub fn record_replicated_participant(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    replica: &ParticipantReplicaRecord,
    identity: &StageParticipant,
) -> Result<(), ReplicatedParticipantRefusal> {
    // The two ends of one replication: the session each record goes into, the attributes
    // that record carries, and whether the participant there IS the replica. The
    // publisher carries the attributes its own identity was issued with; the replica
    // carries the ones the replication merged for it.
    let ends = [
        (
            &replica.source_stage_arn,
            &replica.source_session_id,
            &identity.attributes,
            false,
        ),
        (
            &replica.destination_stage_arn,
            &replica.destination_session_id,
            &replica.attributes,
            true,
        ),
    ];
    // Neither end is written until both can hold the record.
    for (stage_arn, session_id, _, _) in &ends {
        // A session that has ENDED is no session to record in: the write below joins the
        // stage's open session ([`join_stage_session`]), and a stage has an open session
        // exactly while it has a participant in one, so refusing here is what keeps the
        // session checked and the session written the same session.
        let open = state
            .stage_session(stage_arn, session_id)
            .filter(|session| session.end_time.is_none());
        let Some(session) = open else {
            return Err(ReplicatedParticipantRefusal::NoSuchSession {
                stage_arn: (*stage_arn).clone(),
            });
        };
        // A session already holding this participant records no further record for it, so
        // its size is not what stands in the way.
        let holds = session
            .participants
            .iter()
            .any(|participant| participant.participant_id == replica.participant_id);
        if !holds && session.participants.len() >= MAX_PARTICIPANTS_PER_SESSION {
            return Err(ReplicatedParticipantRefusal::SessionFull {
                stage_arn: (*stage_arn).clone(),
            });
        }
    }
    for (stage_arn, session_id, attributes, is_replica) in ends {
        // Only a participant the session holds as present keeps its record untouched: one
        // whose record has left the session is brought back in by the join below.
        let present = session_participant(state, stage_arn, session_id, &replica.participant_id)
            .is_some_and(|present| present.state == PARTICIPANT_STATE_CONNECTED);
        if present {
            // The participant is in this stage already, and a replication starting is not
            // it joining again, so its record keeps what the join recorded. It HAS
            // published, though: only a publishing participant is replicated, and
            // `published` is "whether the participant ever published to the stage session",
            // which stays set once set.
            if let Some(present) =
                session_participant_mut(state, stage_arn, session_id, &replica.participant_id)
            {
                present.published = true;
            }
            // The media this replication carries is beginning where the session had not
            // seen that participant publishing: [`EVENT_PUBLISH_STARTED`] is what begins
            // one participant's media and the two publish names alternate
            // ([`StageSessionRecord::is_publishing`]), so the occurrence is recorded for
            // exactly that transition and what `ListParticipantEvents` reports cannot
            // disagree with what `ListParticipants` does.
            if !state
                .stage_session(stage_arn, session_id)
                .is_some_and(|session| session.is_publishing(&replica.participant_id))
            {
                let mut event =
                    ParticipantEvent::new(EVENT_PUBLISH_STARTED, &replica.participant_id);
                event.replica = is_replica;
                record_participant_event(state, stage_arn, session_id, event, SystemTime::now());
            }
            continue;
        }
        let joined = JoinedParticipant {
            participant_id: replica.participant_id.clone(),
            user_id: identity.user_id.clone(),
            attributes: attributes.clone(),
            published: true,
            // How a publisher reached a stage — whether anything records it, which ingest
            // configuration it broadcast with, whether that used redundant ingest — is
            // what the stage's DATA plane observes, which a replication starting is not,
            // so each stays at what a join with nothing observed records.
            recording_state: None,
            redundant_ingest: false,
            ingest_configuration_arn: None,
            client: ParticipantClient::default(),
            protocol: None,
            replica: is_replica,
        };
        if join_stage_session(state, stage_arn, joined).is_none() {
            // The session exists and has room — both checked above — so the records it
            // holds are the only thing a join of a participant new to it can run out of.
            return Err(ReplicatedParticipantRefusal::SessionFull {
                stage_arn: stage_arn.clone(),
            });
        }
    }
    Ok(())
}

/// What the participants of the session `session_id` of `stage_arn` report about
/// replication right now, keyed by participant id, read off the replications this store
/// holds.
///
/// The replication members of a participant are a fact about a replication rather than
/// about the join that recorded the participant, so they are read from the replications
/// themselves on every report: a participant is a [`REPLICATION_TYPE_REPLICA`] while a
/// replication runs INTO this session — then also naming the stage and session it is
/// replicated from, which the model reports "if `replicationType` is `REPLICA`" — a
/// [`REPLICATION_TYPE_SOURCE`] while one runs FROM this session, and
/// [`REPLICATION_TYPE_NONE`] when no replication names it, which is the default the model
/// documents and which a participant absent from this map takes
/// ([`ParticipantReplication::default`]).
///
/// ONE walk over the replications, for the whole session: a reporting operation reads them
/// once per request and then looks each participant of its page up here, so the work a
/// page costs is the session's participants plus this store's replications rather than the
/// product of the two.
pub fn session_participant_replications(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
) -> HashMap<String, ParticipantReplication> {
    let mut replications: HashMap<String, ParticipantReplication> = HashMap::new();
    for replica in state.participant_replicas.values().flatten() {
        let candidate = if replica.destination_stage_arn == stage_arn
            && replica.destination_session_id == session_id
        {
            ParticipantReplication {
                replication_type: REPLICATION_TYPE_REPLICA.to_string(),
                replication_state: Some(replica.replication_state.clone()),
                source_stage_arn: Some(replica.source_stage_arn.clone()),
                source_session_id: Some(replica.source_session_id.clone()),
            }
        } else if replica.source_stage_arn == stage_arn && replica.source_session_id == session_id {
            ParticipantReplication {
                replication_type: REPLICATION_TYPE_SOURCE.to_string(),
                replication_state: Some(replica.replication_state.clone()),
                // Both reported only "if `replicationType` is `REPLICA`", as the two
                // members' docs state: a participant replicated INTO another stage is not
                // replicated FROM one.
                source_stage_arn: None,
                source_session_id: None,
            }
        } else {
            continue;
        };
        let held = replications.get(&replica.participant_id);
        if held.is_none_or(|held| supersedes(&candidate, held)) {
            replications.insert(replica.participant_id.clone(), candidate);
        }
    }
    replications
}

/// Whether the participant `participant_id` of the session `session_id` of `stage_arn` is
/// a [`REPLICATION_TYPE_REPLICA`] there — a participant a replication runs INTO this
/// session.
///
/// Read off [`session_participant_replications`], the crate's one answer to what a
/// participant of a session reports about replication, so "is this participant a replica"
/// and the `replicationType` that same participant reports cannot disagree.
pub fn session_participant_is_replica(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
) -> bool {
    session_participant_replications(state, stage_arn, session_id)
        .get(participant_id)
        .is_some_and(|replication| replication.replication_type == REPLICATION_TYPE_REPLICA)
}

/// Which of two replications a participant of one session reports, when more than one
/// names it.
///
/// Two orderings, so the order this store happens to hold its replications in cannot
/// decide what a read answers: being a replica IN the session is the stronger statement
/// about that participant's record than being replicated FROM it, and between two of the
/// same kind the RUNNING replication is the one reported — a publisher replicated into
/// several stages is replicating while ANY of those replications is.
pub(super) fn supersedes(candidate: &ParticipantReplication, held: &ParticipantReplication) -> bool {
    let rank = |replication: &ParticipantReplication| {
        (
            replication.replication_type == REPLICATION_TYPE_REPLICA,
            replication.replication_state.as_deref() == Some(REPLICATION_STATE_ACTIVE),
        )
    };
    rank(candidate) > rank(held)
}

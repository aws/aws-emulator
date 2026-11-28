// SPDX-License-Identifier: CC0-1.0

//! The long-term memory EXTRACTION plane: the extraction jobs a memory container holds.
//!
//! An extraction job is a unit of long-term extraction WORK over the messages a memory
//! container accepted — not an extraction result. `ListMemoryExtractionJobs` reports the
//! units that are eligible to be started, and `StartMemoryExtractionJob` picks up the
//! messages one of them left unprocessed, so a unit is listed for as long as no
//! extraction attempt has picked its messages up ([`StartedExtractionJob`]).
//!
//! A start is therefore recorded as the WATERMARK of the session's records it covered:
//! every message the container had accepted into that session when the job ran is picked
//! up, and a message stored afterwards is left unprocessed and composes the session's next
//! unit of work — which is what lets a later start "pick up and extract those as well".
//!
//! One unit is one SESSION of one actor: a session's conversational messages are what
//! long-term extraction reads together, and the modelled job names the actor and session
//! its messages come from beside the `(eventId, messageIndex)` pair of each one
//! ([`ExtractionJobUnit::to_metadata`]). Both ways a container accepts messages for
//! long-term extraction compose a session's unit: the events `CreateEvent` stored in it
//! and the content `IngestData` submitted into it, in the order the container accepted
//! them.
//!
//! What a job does NOT carry is an outcome. A unit is reported with the identity and the
//! messages its session supports, and with no `status` and no `failureReason`: those report
//! an extraction ATTEMPT, and a unit is listed exactly while no attempt has picked its
//! messages up. What an attempt PRODUCES is the long-term memory records the container's
//! strategies derive from those messages, which is the other half of this plane
//! ([`super::memory_extraction`]): both the extraction the clock owes after a message is
//! accepted and the one a start asks for go through it, and both record their pick-up as the
//! watermark here.
//!
//! The work this walk does is bounded by the store's own ceilings
//! ([`super::MAX_STORED_EVENTS`] events and [`super::MAX_STORED_INGESTIONS`] ingestions
//! across every session), so a listing cannot grow with request volume.

use super::{normalize_memory_id, ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState};
use crate::ops::types::{
    ExtractionJobMessages, ExtractionJobMetadata, IngestPayloadType, MessageMetadata, PayloadType,
};

/// `ExtractionMode`'s only value: an event created with it "is excluded from long-term
/// memory extraction" (source: smithy_query query="shape" shape="ExtractionMode", values
/// SKIP), so it composes no message of a container's extraction jobs.
pub const EXTRACTION_MODE_SKIP: &str = "SKIP";

/// The `jobId` of the unit composed of one actor's session, in the ONE spelling the two
/// extraction-job operations read it by.
///
/// `ExtractionJob$jobId` and `ExtractionJobMetadata$jobID` are plain strings with no
/// modelled form, and the unit a job stands for is identified by the actor and session
/// whose messages compose it — so the id is that pair, base64-encoded (the codec this
/// crate's pagination tokens already use) to keep it opaque to a caller. A `sessionId`
/// admits neither `:` nor any other separator character
/// (`^[a-zA-Z0-9][a-zA-Z0-9-_]*$`), so the pair is unambiguous in that order.
#[must_use]
pub fn extraction_job_id(actor_id: &str, session_id: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{session_id}:{actor_id}"))
}

/// The most extraction-job records one memory container keeps.
///
/// A record is keyed by the `jobId` of one (actor, session) pair, and a container holds at
/// most [`super::MAX_ACTORS_PER_MEMORY`] times [`super::MAX_SESSIONS_PER_ACTOR`] of those,
/// so this ceiling is that product: a caller looping on starts rewrites the record of a
/// session it already started rather than appending one. Should a container ever pass it
/// (a session evicted after its job was started), the least recently started record gives
/// way, which only makes that session's messages eligible again.
pub const MAX_STARTED_EXTRACTION_JOBS_PER_MEMORY: usize =
    super::MAX_ACTORS_PER_MEMORY * super::MAX_SESSIONS_PER_ACTOR;

/// One extraction job a `StartMemoryExtractionJob` picked the messages of up.
///
/// The record is the WATERMARK of that pick-up rather than a result: it says which of the
/// session's records the attempt covered, so the messages stored after it are the ones the
/// next unit of work is composed of.
#[derive(Debug, Clone)]
pub struct StartedExtractionJob {
    /// The job's `jobId`, as [`extraction_job_id`] spells it: the id the start was asked
    /// for and the id it answers with.
    pub job_id: String,
    pub actor_id: String,
    pub session_id: String,
    /// The highest accepted sequence this attempt picked up — every message of the
    /// session the container had accepted by then.
    pub extracted_through_seq: u64,
    /// The sequence the start itself was handed: the order the container started its
    /// jobs in, and the eviction order of this record.
    pub seq: u64,
}

/// One unit of long-term extraction work a memory container holds: the messages of one
/// session that no extraction has processed, with the identity a listing reports them
/// under.
#[derive(Debug, Clone)]
pub struct ExtractionJobUnit {
    /// The unit's `jobId`, as [`extraction_job_id`] spells it.
    pub job_id: String,
    pub actor_id: String,
    pub session_id: String,
    /// The messages that compose the unit, in the order the container accepted them.
    pub messages: Vec<MessageMetadata>,
    /// The sequence of the earliest event the unit's messages come from: the order the
    /// container accepted this unit of work, which is the order a listing reports units
    /// in.
    pub accepted_seq: u64,
    /// The sequence of the LATEST record the unit's messages come from: the watermark a
    /// start of this unit records ([`StartedExtractionJob::extracted_through_seq`]).
    pub latest_seq: u64,
}

impl ExtractionJobUnit {
    /// This unit as the modelled `ExtractionJobMetadata`.
    ///
    /// `status`, `failureReason` and `strategyId` are left out: the first two report an
    /// extraction attempt's outcome and nothing has attempted one, and a memory strategy
    /// is owned by the CONTROL plane, a different service, so this plane can name none
    /// for a unit it derived from stored messages.
    #[must_use]
    pub fn to_metadata(&self) -> ExtractionJobMetadata {
        ExtractionJobMetadata {
            job_id: self.job_id.clone(),
            messages: ExtractionJobMessages::messagesList(self.messages.clone()),
            status: None,
            failure_reason: None,
            strategy_id: None,
            session_id: Some(self.session_id.clone()),
            actor_id: Some(self.actor_id.clone()),
        }
    }
}

impl super::StoredEvent {
    /// Whether this event's messages are read by long-term extraction: an event created
    /// with `extractionMode` `SKIP` is excluded from it, and one created without the
    /// member "is processed for extraction as usual".
    #[must_use]
    pub fn read_by_extraction(&self) -> bool {
        self.extraction_mode.as_deref() != Some(EXTRACTION_MODE_SKIP)
    }

    /// The messages of this event, as the metadata an extraction job names each one by.
    ///
    /// `PayloadType` carries a conversational turn, a JSON document or a blob, and a
    /// MESSAGE is the conversational turn — `MessageMetadata$messageIndex` is "the
    /// position of this message within that event's ordered list of messages", so the
    /// index counts this event's conversational items in payload order and an event
    /// carrying none composes no message.
    #[must_use]
    pub fn extraction_messages(&self) -> Vec<MessageMetadata> {
        self.payload
            .iter()
            .filter(|item| matches!(item, PayloadType::Conversational(_)))
            .enumerate()
            .map(|(index, _)| MessageMetadata {
                event_id: self.event_id.clone(),
                message_index: i32::try_from(index).unwrap_or(i32::MAX),
            })
            .collect()
    }
}

impl super::IngestedContent {
    /// The messages of this ingested content, as the metadata an extraction job names
    /// each one by.
    ///
    /// `IngestPayloadType` carries a conversational turn or a JSON document, and a
    /// message is the conversational turn, indexed exactly as an event's messages are
    /// ([`super::StoredEvent::extraction_messages`]). The content is named by the id this
    /// service minted for the record it stored it in: `MessageMetadata$eventId` is the
    /// only identity a job can name a message by, and ingested content is stored as its
    /// own record rather than as one of the session's short-term events.
    #[must_use]
    pub fn extraction_messages(&self) -> Vec<MessageMetadata> {
        self.payload
            .iter()
            .filter(|item| matches!(item, IngestPayloadType::Conversational(_)))
            .enumerate()
            .map(|(index, _)| MessageMetadata {
                event_id: self.content_id.clone(),
                message_index: i32::try_from(index).unwrap_or(i32::MAX),
            })
            .collect()
    }
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The units of long-term extraction work the memory container `memory_id` names
    /// holds, oldest unit first.
    ///
    /// A session composes one unit, from the messages of the events extraction reads
    /// ([`super::StoredEvent::read_by_extraction`]) and of the content ingested into it,
    /// ordered by the write that stored each one; a session whose records compose no
    /// message at all is no unit of work and is left out, and so are the records an
    /// earlier start already picked up ([`StartedExtractionJob`]). The order of the units
    /// is the one the container accepted them in, with the `jobId` breaking a tie, so the
    /// pages of one listing cannot disagree about where a unit sits.
    ///
    /// A container this plane has nothing stored for holds no unit — which is not a
    /// statement about whether the CONTAINER exists, resolved by
    /// [`super::memory_item_container_resolved`] before this is read.
    #[must_use]
    pub fn extraction_job_units(&self, memory_id: &str) -> Vec<ExtractionJobUnit> {
        let Some(memory) = self.memories.get(&normalize_memory_id(memory_id)) else {
            return Vec::new();
        };
        let mut units: Vec<ExtractionJobUnit> = Vec::new();
        for (actor_id, actor) in &memory.actors {
            for (session_id, session) in &actor.sessions {
                let job_id = extraction_job_id(actor_id, session_id);
                // What an earlier start of this unit already picked up: only the records
                // the container accepted after it are still work.
                let extracted_through = memory
                    .extractions
                    .get(&job_id)
                    .map_or(0, |started| started.extracted_through_seq);
                // The records of this session that carry messages for extraction, in the
                // order the container accepted them: the engine-wide sequence totally
                // orders the two kinds of write against each other.
                let mut accepted: Vec<(u64, Vec<MessageMetadata>)> = session
                    .events
                    .iter()
                    .filter(|event| event.read_by_extraction())
                    .map(|event| (event.seq, event.extraction_messages()))
                    .chain(
                        session
                            .ingestions
                            .iter()
                            .map(|ingested| (ingested.seq, ingested.extraction_messages())),
                    )
                    .filter(|(seq, messages)| *seq > extracted_through && !messages.is_empty())
                    .collect();
                accepted.sort_by_key(|(seq, _)| *seq);
                let Some(accepted_seq) = accepted.first().map(|(seq, _)| *seq) else {
                    continue;
                };
                let latest_seq = accepted.last().map_or(accepted_seq, |(seq, _)| *seq);
                units.push(ExtractionJobUnit {
                    job_id,
                    actor_id: actor_id.clone(),
                    session_id: session_id.clone(),
                    messages: accepted
                        .into_iter()
                        .flat_map(|(_, messages)| messages)
                        .collect(),
                    accepted_seq,
                    latest_seq,
                });
            }
        }
        units.sort_by(|left, right| {
            left.accepted_seq
                .cmp(&right.accepted_seq)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        units
    }

    /// The unit of extraction work the memory container `memory_id` names holds under
    /// `job_id`, or `None` when it holds no such unit.
    #[must_use]
    pub fn extraction_job_unit(&self, memory_id: &str, job_id: &str) -> Option<ExtractionJobUnit> {
        self.extraction_job_units(memory_id)
            .into_iter()
            .find(|unit| unit.job_id == job_id)
    }

    /// The extraction job `job_id` a start of this container already picked the messages
    /// of up, or `None` when no start did.
    #[must_use]
    pub fn started_extraction_job(
        &self,
        memory_id: &str,
        job_id: &str,
    ) -> Option<&StartedExtractionJob> {
        self.memories
            .get(&normalize_memory_id(memory_id))?
            .extractions
            .get(job_id)
    }

    /// Pick up the messages the extraction job `job_id` left unprocessed, recording the
    /// watermark of what this attempt covered and storing the memory records `strategies`
    /// derive from those messages ([`super::memory_extraction`]).
    ///
    /// `None` when the container holds no unit of work under that id — the caller has
    /// already resolved the CONTAINER itself ([`super::memory_item_container_resolved`]), so
    /// this answers only about the job. A job that WAS started and has nothing new to
    /// pick up is answered with the record the earlier start wrote, which is what makes
    /// repeating the one `@idempotent` request safe: starting it again neither mints a
    /// second job nor refuses the id the first start was given.
    pub fn start_extraction_job(
        &mut self,
        memory_id: &str,
        job_id: &str,
        strategies: &[super::ExtractionStrategy],
        now: ::std::time::SystemTime,
    ) -> Option<StartedExtractionJob> {
        // A start is a request for the extraction itself, so it reads the unit's messages
        // whether or not the clock had made their own extraction due yet, and the records
        // it derives are stored by the one path that derives them.
        match self.extract_memory_job(memory_id, job_id, strategies, now) {
            Some(started) => Some(started),
            None => self.started_extraction_job(memory_id, job_id).cloned(),
        }
    }

    /// Record that the messages of the unit `job_id` up to `through_seq` have been picked
    /// up, so only the records the container accepts afterwards are still work.
    ///
    /// The ONE write of that watermark, whether the extraction was asked for by a start or
    /// was the one the clock owed, so the two cannot disagree about what has been
    /// extracted. `None` when this plane holds no index for the container at all.
    pub(super) fn record_extraction_watermark(
        &mut self,
        memory_key: &str,
        job_id: &str,
        actor_id: &str,
        session_id: &str,
        through_seq: u64,
    ) -> Option<StartedExtractionJob> {
        // The caller resolved the unit out of this container's own records, so the
        // container is held. The sequence is taken only once that is established, so a
        // lookup that somehow missed cannot consume one without writing anything.
        if !self.memories.contains_key(memory_key) {
            return None;
        }
        let seq = self.next_seq();
        let memory = self.memories.get_mut(memory_key)?;
        let started = StartedExtractionJob {
            job_id: job_id.to_string(),
            actor_id: actor_id.to_string(),
            session_id: session_id.to_string(),
            extracted_through_seq: through_seq,
            seq,
        };
        memory.last_seq = memory.last_seq.max(seq);
        memory
            .extractions
            .insert(job_id.to_string(), started.clone());
        while memory.extractions.len() > MAX_STARTED_EXTRACTION_JOBS_PER_MEMORY {
            let Some(oldest) =
                super::least_recently_written(&memory.extractions, |started| started.seq)
            else {
                break;
            };
            memory.extractions.remove(&oldest);
        }
        Some(started)
    }
}

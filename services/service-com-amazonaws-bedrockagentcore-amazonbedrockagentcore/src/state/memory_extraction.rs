// SPDX-License-Identifier: CC0-1.0

//! What long-term memory extraction PRODUCES: the derived memory records the messages a
//! container accepted yield, in the namespaces that container's strategies resolve to.
//!
//! [`extraction_jobs`](super::extraction_jobs) holds the WORK side of this plane — which
//! messages are still waiting to be extracted. This file is its outcome: a memory container
//! whose strategies resolve a namespace for the actor and session a message belongs to
//! answers a later read of that namespace with a record derived from it.
//!
//! ## What drives it
//!
//! Extraction is ASYNCHRONOUS: `CreateEvent` answers 201 with the event stored
//! short-term, and the derived record appears afterwards, which is what makes a client poll
//! `ListMemoryRecords` for it. So the span is the clock's: each accepted record carries the
//! instant its extraction comes due ([`LONG_TERM_EXTRACTION_SPAN`] after the call that
//! accepted it, stored and never slept on), and every operation that reaches a container's
//! memory records materialises the extraction that instant has already made due
//! ([`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::settle_long_term_extraction`])
//! — exactly as such an operation materialises a record expiry the clock has run out on.
//! Nothing is produced early: a caller reading the namespace straight after the write still
//! sees it empty.
//!
//! ## What the strategies are
//!
//! A memory STRATEGY is the CONTROL plane's record (`CreateMemory` / `UpdateMemory` mint
//! each `strategyId` and hold its namespace templates), so this plane reads the strategies
//! of a container off the component that owns them ([`memory_extraction_strategies`]) and
//! keeps no copy: a strategy a caller added, removed or is still having provisioned is
//! whatever that plane reports it as. A container with no strategy derives nothing, which is
//! the whole point of configuring one.
//!
//! A namespace template resolves through the three variables the service defines itself and
//! through the values the accepting request supplied for the ones the memory declares
//! ([`resolved_namespace`]); a template with a variable nothing resolves addresses no
//! namespace, so it yields no record rather than one under a half-substituted name.
//!
//! ## What a derived record carries
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. What AWS
//! stores in a derived record is a model's reading of a conversation, so the content served
//! here SAYS that no model ran (the one wording of that notice, the Bedrock Runtime plane's
//! own [`EMULATED_COMPLETION_FLOOR`]) and then carries the conversation the extraction read,
//! as a transcript of the stored turns. It is never prose synthesised to look like a model's
//! reading, and a client reading a record can always tell that no inference happened.
//! Everything AROUND the content is real and computed from what was accepted: the strategy
//! that asked for the extraction, the namespace its template resolved to, the instant of the
//! source content and the expiry that strategy configures.
//!
//! A payload item that is not a conversational turn (a JSON document, a blob) composes no
//! message, so a record carrying only those derives nothing — the same reading
//! [`super::StoredEvent::extraction_messages`] already makes.
//!
//! ## What bounds it
//!
//! One settlement derives at most one record per strategy namespace template per session
//! unit, and each unit is picked up exactly once (the watermark
//! [`super::StartedExtractionJob`] a start of the same unit writes), so repeated reads
//! derive nothing further. The records themselves are held by the store's own ceilings
//! ([`super::MAX_MEMORY_RECORDS_PER_MEMORY`], [`super::MAX_STORED_MEMORY_RECORDS`]).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;
use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::EMULATED_COMPLETION_FLOOR;

use super::{
    extraction_job_id, mint_memory_record_id, normalize_memory_id, record_expires_at,
    ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState, StoredMemoryRecord,
};
use crate::ops::types::{Content, IngestPayloadType, MemoryContent, PayloadType};

/// How long after the call that accepted a message its long-term extraction comes due.
///
/// AWS publishes no duration for the extraction pipeline, so this span is the EMULATOR's
/// own: long enough that a caller reading the namespace immediately after `CreateEvent`
/// still sees the record absent — the state its own convergence poll is written against —
/// and short enough that the poll converges.
pub const LONG_TERM_EXTRACTION_SPAN: Duration = Duration::from_secs(3);

/// The longest content a derived record carries: `MemoryContent$text` is `@length(1..16000)`
/// (source: smithy_query query="shape" shape="MemoryContent"), so a conversation longer than
/// that is reported up to the limit rather than as a value the wire cannot render.
const MAX_DERIVED_CONTENT_CHARS: usize = 16_000;

/// The three namespace variables the service defines itself, substituted from the message's
/// own identity rather than from anything the caller supplied: `NamespaceVariableName`'s
/// pattern `^(?!memoryStrategyId$|actorId$|sessionId$)[a-z][a-z0-9]*$` (source: smithy_query
/// query="shape" shape="NamespaceVariablesMap") forbids a caller to define exactly these
/// three, because the service resolves them.
const NAMESPACE_VARIABLE_STRATEGY: &str = "memoryStrategyId";
/// See [`NAMESPACE_VARIABLE_STRATEGY`].
const NAMESPACE_VARIABLE_ACTOR: &str = "actorId";
/// See [`NAMESPACE_VARIABLE_STRATEGY`].
const NAMESPACE_VARIABLE_SESSION: &str = "sessionId";

/// One memory strategy, as the extraction of a container's messages reads it: the identity
/// a derived record is reported under, the namespaces it writes them to, and the expiry it
/// gives them.
#[derive(Debug, Clone)]
pub struct ExtractionStrategy {
    /// The `strategyId` the CONTROL plane minted, which is the `memoryStrategyId` every
    /// record derived under this strategy carries.
    pub strategy_id: String,
    /// The strategy's namespace templates, each resolved per message
    /// ([`resolved_namespace`]).
    pub namespace_templates: Vec<String>,
    /// The record expiry the strategy configures, in days, when it configures one.
    pub record_expiry_duration_days: Option<i32>,
}

/// The strategies the memory container `memory_id` names has extraction running under, read
/// off the component that OWNS them.
///
/// A READ of the control plane's store: nothing of either plane's state is changed. It is
/// taken and released BEFORE this service's own store, the lock order every reader of
/// another plane's store in this crate keeps.
///
/// An empty list is a container that derives nothing, for every reason a container has not
/// to: it holds no strategy, every strategy of it is still being provisioned or is being
/// removed, its deletion has already been accepted, the account does not hold it at all, or
/// the shared engine does not carry that plane's store.
#[must_use]
pub fn memory_extraction_strategies<Eng: Send + Sync + 'static>(
    engine: &Eng,
    memory_id: &str,
) -> Vec<ExtractionStrategy> {
    let Some(control) = control_plane::state::service_state(engine) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let store = control.lock();
    let Some(memory) = store.memories.get(&normalize_memory_id(memory_id)) else {
        return Vec::new();
    };
    if memory.is_deleting() {
        return Vec::new();
    }
    memory
        .memory(now)
        .strategies
        .into_iter()
        .flatten()
        // A strategy extraction runs under is one the control plane reports ACTIVE: one it
        // reports CREATING is not provisioned yet, and one it reports DELETING is on its
        // way out, so neither writes a record.
        .filter(|strategy| {
            strategy.status.as_deref()
                == Some(control_plane::state::MEMORY_STRATEGY_STATUS_ACTIVE)
        })
        .map(|strategy| ExtractionStrategy {
            strategy_id: strategy.strategy_id,
            // `namespaceTemplates` is the member a strategy states its namespaces in, with
            // the legacy `namespaces` ("Use namespaceTemplates instead") reported beside
            // it; the control plane stores one list under both names, so either one is the
            // strategy's namespaces.
            namespace_templates: if strategy.namespace_templates.is_empty() {
                strategy.namespaces
            } else {
                strategy.namespace_templates
            },
            record_expiry_duration_days: strategy.record_expiry_duration_days,
        })
        .collect()
}

/// The namespace `template` addresses for a message of `actor_id` / `session_id` extracted
/// under `strategy_id`, or `None` when the template references a variable nothing resolves.
///
/// A template's `{variable}` resolves either to one of the three variables the service
/// defines itself or to a value the accepting request supplied for a variable the memory
/// declares; a reference to anything else addresses no namespace, so no record is written
/// under a name still carrying the reference.
#[must_use]
pub fn resolved_namespace(
    template: &str,
    strategy_id: &str,
    actor_id: &str,
    session_id: &str,
    variables: &HashMap<String, String>,
) -> Option<String> {
    let mut resolved = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        resolved.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // An unclosed `{` references no variable, so the rest of the template is literal
        // text — the same reading the control plane makes of one.
        let Some(end) = after.find('}') else {
            resolved.push('{');
            rest = after;
            continue;
        };
        let value = match &after[..end] {
            NAMESPACE_VARIABLE_STRATEGY => strategy_id,
            NAMESPACE_VARIABLE_ACTOR => actor_id,
            NAMESPACE_VARIABLE_SESSION => session_id,
            declared => variables.get(declared)?.as_str(),
        };
        resolved.push_str(value);
        rest = &after[end + 1..];
    }
    resolved.push_str(rest);
    Some(resolved)
}

/// The conversation `payload` carries, as the text an extraction of it reads: one line per
/// conversational turn, naming the role that spoke it, in payload order.
///
/// A payload item that is not a conversational turn is no message, so it contributes no
/// line; a payload carrying none at all reads as the empty string, which composes no unit
/// of extraction work.
fn conversation_text<'a>(turns: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut text = String::new();
    for (role, said) in turns {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(role);
        text.push_str(": ");
        text.push_str(said);
    }
    truncated(text)
}

/// `text` cut to what `MemoryContent$text` can carry, on a character boundary.
fn truncated(text: String) -> String {
    if text.chars().count() > MAX_DERIVED_CONTENT_CHARS {
        return text.chars().take(MAX_DERIVED_CONTENT_CHARS).collect();
    }
    text
}

/// The content one derived record carries: the notice that no model ran, then the
/// `messages` turns of `conversation` the extraction read.
///
/// AWS stores a model's reading of the conversation here, which this emulator cannot run, so
/// the record states that and carries the transcript it was derived from rather than prose
/// composed to look like such a reading.
fn derived_content(conversation: &str, messages: usize) -> String {
    truncated(format!(
        "{EMULATED_COMPLETION_FLOOR} This long-term memory record carries the {messages} \
         message(s) its extraction read rather than a model's reading of them.\n{conversation}"
    ))
}

impl super::StoredEvent {
    /// This event's conversational turns as the text long-term extraction reads
    /// ([`conversation_text`]).
    #[must_use]
    pub fn extraction_text(&self) -> String {
        conversation_text(self.payload.iter().filter_map(|item| match item {
            PayloadType::Conversational(turn) => {
                let Content::Text(said) = &turn.content;
                Some((turn.role.as_str(), said.as_str()))
            }
            PayloadType::Blob(_) | PayloadType::Json(_) => None,
        }))
    }
}

impl super::IngestedContent {
    /// This ingested content's conversational turns as the text long-term extraction
    /// reads, exactly as an event's are.
    #[must_use]
    pub fn extraction_text(&self) -> String {
        conversation_text(self.payload.iter().filter_map(|item| match item {
            IngestPayloadType::Conversational(turn) => {
                let Content::Text(said) = &turn.content;
                Some((turn.role.as_str(), said.as_str()))
            }
            IngestPayloadType::Json(_) => None,
        }))
    }
}

/// One record of a session that extraction reads: the messages it carries as text, with the
/// instant, the order and the namespace values extraction reads them under.
#[derive(Debug, Clone)]
struct ExtractedSource {
    /// The instant the content occurred, as the accepting request stated it: what a derived
    /// record reports as its own `timestamp`, "the time of the source content that the
    /// record was derived from".
    timestamp: ::aws_smithy_types::DateTime,
    /// The container-wide sequence of the record, which orders the messages of a session
    /// against each other and is the watermark a pick-up records.
    seq: u64,
    /// The namespace variable values the accepting request supplied.
    variables: HashMap<String, String>,
    /// How many conversational turns the record carries, as
    /// [`super::StoredEvent::extraction_messages`] counts the messages of one.
    messages: usize,
    /// The conversation the messages carry.
    text: String,
}

/// One unit of extraction work that is ready to be extracted: the messages of one session
/// no extraction has picked up and whose own due instant has passed.
#[derive(Debug, Clone)]
struct DueExtraction {
    job_id: String,
    actor_id: String,
    session_id: String,
    /// The highest sequence this extraction covers: the watermark it records.
    through_seq: u64,
    sources: Vec<ExtractedSource>,
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// Materialise the long-term extraction the clock has already made due on the memory
    /// container `memory_id` names, under `strategies`.
    ///
    /// Called by every operation that reaches a container's memory records, before it reads
    /// or writes one: the records a due extraction derived exist whether or not anybody
    /// calls, exactly as a record whose expiry has run out is gone whether or not anybody
    /// calls ([`Self::expire_memory_records`]). A container with no strategy, or with no
    /// message whose extraction is due, is left exactly as it was.
    pub fn settle_long_term_extraction(
        &mut self,
        memory_id: &str,
        strategies: &[ExtractionStrategy],
        now: SystemTime,
    ) {
        if strategies.is_empty() {
            return;
        }
        let memory_key = normalize_memory_id(memory_id);
        for due in self.due_extractions(&memory_key, Some(now)) {
            self.extract(&memory_key, &due, strategies, now);
        }
    }

    /// Extract the messages the unit `job_id` of this container holds right now, whether or
    /// not their own due instant has passed: what `StartMemoryExtractionJob` asks for, which
    /// is an extraction of the messages "earlier extraction attempts have left
    /// unprocessed".
    ///
    /// Answers the watermark the pick-up recorded, or `None` when the container holds no
    /// unit of work under that id.
    pub fn extract_memory_job(
        &mut self,
        memory_id: &str,
        job_id: &str,
        strategies: &[ExtractionStrategy],
        now: SystemTime,
    ) -> Option<super::StartedExtractionJob> {
        let memory_key = normalize_memory_id(memory_id);
        let due = self
            .due_extractions(&memory_key, None)
            .into_iter()
            .find(|due| due.job_id == job_id)?;
        self.extract(&memory_key, &due, strategies, now)
    }

    /// The units of extraction work this container holds whose own due instant has passed
    /// `due_by`: the ones the clock has made due, or every one of them (`None`) when the
    /// caller asked for the extraction itself.
    fn due_extractions(
        &self,
        memory_key: &str,
        due_by: Option<SystemTime>,
    ) -> Vec<DueExtraction> {
        let Some(memory) = self.memories.get(memory_key) else {
            return Vec::new();
        };
        let mut units: Vec<DueExtraction> = Vec::new();
        for (actor_id, actor) in &memory.actors {
            for (session_id, session) in &actor.sessions {
                let job_id = extraction_job_id(actor_id, session_id);
                // What an earlier extraction of this unit already picked up: only the
                // records the container accepted after it are still work.
                let extracted_through = memory
                    .extractions
                    .get(&job_id)
                    .map_or(0, |started| started.extracted_through_seq);
                let mut sources: Vec<ExtractedSource> = Vec::new();
                for event in &session.events {
                    if !event.read_by_extraction()
                        || event.seq <= extracted_through
                        || due_by.is_some_and(|due_by| event.extraction_due_at > due_by)
                    {
                        continue;
                    }
                    let text = event.extraction_text();
                    if text.is_empty() {
                        continue;
                    }
                    sources.push(ExtractedSource {
                        timestamp: event.event_timestamp,
                        seq: event.seq,
                        variables: namespace_variables(event.extraction_config.as_ref()),
                        messages: event.extraction_messages().len(),
                        text,
                    });
                }
                for ingested in &session.ingestions {
                    if ingested.seq <= extracted_through
                        || due_by.is_some_and(|due_by| ingested.extraction_due_at > due_by)
                    {
                        continue;
                    }
                    let text = ingested.extraction_text();
                    if text.is_empty() {
                        continue;
                    }
                    sources.push(ExtractedSource {
                        timestamp: ingested.content_timestamp,
                        seq: ingested.seq,
                        variables: namespace_variables(ingested.extraction_config.as_ref()),
                        messages: ingested.extraction_messages().len(),
                        text,
                    });
                }
                sources.sort_by_key(|source| source.seq);
                let Some(through_seq) = sources.last().map(|source| source.seq) else {
                    continue;
                };
                units.push(DueExtraction {
                    job_id,
                    actor_id: actor_id.clone(),
                    session_id: session_id.clone(),
                    through_seq,
                    sources,
                });
            }
        }
        units.sort_by(|left, right| {
            left.through_seq
                .cmp(&right.through_seq)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        units
    }

    /// Extract one unit: write the watermark that says its messages were picked up, then
    /// store the records the strategies derived from them.
    ///
    /// The watermark is written whether or not any strategy resolved a namespace for the
    /// unit: the messages were read either way, so a later read does not extract them
    /// again.
    fn extract(
        &mut self,
        memory_key: &str,
        due: &DueExtraction,
        strategies: &[ExtractionStrategy],
        now: SystemTime,
    ) -> Option<super::StartedExtractionJob> {
        // The latest record the unit's messages come from: what the derived record reports
        // as the instant of the source content. A unit is composed only from records that
        // carry messages, so a unit with none is no unit at all.
        let latest = due.sources.last()?;
        let watermark = self.record_extraction_watermark(
            memory_key,
            &due.job_id,
            &due.actor_id,
            &due.session_id,
            due.through_seq,
        )?;
        let created_at = ::aws_smithy_types::DateTime::from(now);
        // The conversation the unit's messages carry, in the order the container accepted
        // them, and the namespace values the requests that accepted them supplied — the
        // later value of a variable is the one extraction substitutes, as the latest
        // configuration of the session.
        let conversation = due
            .sources
            .iter()
            .map(|source| source.text.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        let text = derived_content(
            &conversation,
            due.sources
                .iter()
                .map(|source| source.messages)
                .sum::<usize>(),
        );
        let mut variables: HashMap<String, String> = HashMap::new();
        for source in &due.sources {
            variables.extend(
                source
                    .variables
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        let latest_timestamp = latest.timestamp;
        let mut derived: Vec<StoredMemoryRecord> = Vec::new();
        for strategy in strategies {
            for template in &strategy.namespace_templates {
                let Some(namespace) = resolved_namespace(
                    template,
                    &strategy.strategy_id,
                    &due.actor_id,
                    &due.session_id,
                    &variables,
                ) else {
                    continue;
                };
                let seq = self.next_seq();
                derived.push(StoredMemoryRecord {
                    memory_record_id: mint_memory_record_id(created_at, seq),
                    memory_id: memory_key.to_string(),
                    // No batch entry asked for this record, so it tracks none.
                    request_identifier: None,
                    // `MemoryRecordSummary$namespaces` is `@length(max: 1)`, so a record
                    // is stored under the ONE namespace the template resolved to.
                    namespaces: vec![namespace],
                    content: MemoryContent::Text(text.clone()),
                    memory_strategy_id: strategy.strategy_id.clone(),
                    timestamp: latest_timestamp,
                    created_at,
                    expires_at: strategy
                        .record_expiry_duration_days
                        .map(|days| record_expires_at(created_at, days)),
                    metadata: None,
                    seq,
                });
            }
        }
        if !derived.is_empty() {
            // The container is registered and every bound of the store is held by the one
            // write path a memory record goes through, so extraction cannot grow it
            // without limit either. No `clientToken` created these records.
            self.insert_memory_records(memory_key, derived, None);
        }
        Some(watermark)
    }
}

/// The namespace variable values one accepting request supplied, as extraction substitutes
/// them; none for a request that carried no `extractionConfig`.
fn namespace_variables(
    config: Option<&crate::ops::types::ExtractionConfig>,
) -> HashMap<String, String> {
    config
        .and_then(|config| config.namespace_variables.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template resolves through the variables the service defines itself and through the
    /// values the request supplied, and addresses no namespace at all when it references one
    /// nothing resolves.
    #[test]
    fn a_namespace_template_resolves_only_through_variables_that_have_a_value() {
        let mut supplied = HashMap::new();
        supplied.insert("tenant".to_string(), "acme".to_string());

        // (case, the template a strategy states, the namespace it addresses)
        let rows: [(&str, &str, Option<&str>); 7] = [
            (
                "the session-scoped default shape",
                "/strategies/{memoryStrategyId}/actors/{actorId}/sessions/{sessionId}",
                Some("/strategies/strat-1a2b3c4d5e/actors/alpha/sessions/s1"),
            ),
            (
                "an actor-scoped template with a trailing separator",
                "/simparity/{actorId}/{sessionId}/",
                Some("/simparity/alpha/s1/"),
            ),
            (
                "a template with no variable at all",
                "/facts",
                Some("/facts"),
            ),
            (
                "a template through a variable the request supplied",
                "/t/{tenant}/{actorId}",
                Some("/t/acme/alpha"),
            ),
            (
                "a template through a variable nothing supplied",
                "/t/{region}/{actorId}",
                None,
            ),
            (
                "a template whose brace is never closed",
                "/t/{actorId",
                Some("/t/{actorId"),
            ),
            (
                "a template naming one variable twice",
                "/{actorId}/{actorId}",
                Some("/alpha/alpha"),
            ),
        ];
        for (case, template, expected) in rows {
            let actual =
                resolved_namespace(template, "strat-1a2b3c4d5e", "alpha", "s1", &supplied);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?} ({template}): expected {expected:?}, got {actual:?}"
            );
        }
    }
}

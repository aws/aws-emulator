// SPDX-License-Identifier: CC0-1.0

//! The SiteWise Assistant slice of [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`]:
//! the conversations `InvokeAssistant` starts and continues, the RETRIEVAL one turn runs
//! over the datasets this account holds, and the answer it is composed into.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. A turn is
//! answered in two steps, and neither of them is prose synthesised to look like inference:
//!
//! * the RETRIEVAL is a real free-text search over the stored datasets — the one shared
//!   search this workspace has ([`::service_common::document_text_search`]): the message's
//!   terms are matched against each dataset's id, name and description, a dataset carrying
//!   none of them is not a result, and the datasets that do carry some rank by the share of
//!   the message's terms each one carries;
//! * the ANSWER opens with the notice that no model generated it
//!   ([`EMULATED_ASSISTANT_NOTICE`]), states which turn of which conversation it is and how
//!   much of the account it searched, and then QUOTES the matched text of each cited dataset
//!   exactly as that dataset's record holds it. The caller's own `@sensitive` message is
//!   never quoted back: what the answer carries is text the store really holds.
//!
//! The CITATIONS are the same datasets, in the same order: one per quoted excerpt, naming
//! the dataset's ARN and the source its text was read through — the Kendra knowledge base
//! of a dataset whose source names one ("If the source is Kendra, it's the ARN of the
//! Kendra index"), and the dataset's own ARN otherwise.

use super::*;

/// The notice an answer opens with: that no model generated it.
///
/// One wording, in one place, so every turn says it the same way.
pub const EMULATED_ASSISTANT_NOTICE: &str =
    "No language model runs in this emulator, so the SiteWise Assistant answers from what \
     this account holds rather than by generating prose.";

/// How many conversations this store keeps.
///
/// A conversation is minted by `InvokeAssistant` and this service models no call that
/// deletes one, so the store is bounded by EVICTING the least recently used conversation —
/// the same eviction the logging-option scopes get
/// ([`super::ComAmazonawsIotsitewiseAwsiotsitewiseState::logging_options`]) — rather than by
/// refusing a turn. The emulator's own bound, not a published AWS quota.
pub const MAX_ASSISTANT_CONVERSATIONS: usize = 200;

/// How many of the terms a message names one turn searches for.
///
/// The retrieval compares every term against every stored dataset's text, so the work one
/// turn does is the PRODUCT of the terms the message names, the datasets the account holds
/// and the length of their stored text: each factor is bounded on its own — the message by
/// the `@length` the model puts on it, the datasets by [`MAX_DATASETS`] — and this is what
/// bounds the product. A message naming more distinct terms than this is searched for the
/// first of them, in the order it states them. The emulator's own bound.
pub const MAX_ASSISTANT_QUERY_TERMS: usize = 32;

/// How many datasets one answer cites.
///
/// The retrieval ranks every dataset the account holds, and the answer quotes the most
/// relevant ones: this is where it stops, so a turn against an account holding thousands of
/// datasets composes a bounded answer. The emulator's own bound.
pub const MAX_ASSISTANT_CITATIONS: usize = 5;

/// The excerpt one citation quotes: how much of the matched text it carries, how far ahead
/// of the first match it opens, and the most matches it is cut around.
///
/// The emulator's own, because the model documents no size for `Content.text`.
const EXCERPT_BOUNDS: ::service_common::document_text_search::WindowBounds =
    ::service_common::document_text_search::WindowBounds {
        max_chars: 512,
        lead_chars: 32,
        max_spans: 8,
    };

/// One conversation of the SiteWise Assistant: the id it is addressed by, when it began,
/// when its last turn landed and how many turns it has taken.
///
/// The id is the caller's when the request named one ("if you prefer to have your own ID,
/// you must specify it here in UUID format") and minted here when it named none ("IoT
/// SiteWise automatically generates a unique ID for you").
///
/// The caller's `@sensitive` message is NOT kept: nothing this emulator answers is composed
/// from it, so storing it would hold a sensitive value no read can ever need.
#[derive(Debug, Clone)]
pub struct AssistantConversationRecord {
    pub id: String,
    pub started_at: SystemTime,
    pub last_turn_at: SystemTime,
    /// How many turns this conversation has taken, the turn being answered included.
    pub turns: u32,
}

/// One cited dataset of an answer: which dataset it is, the source its text was read
/// through, and the excerpt the answer quotes from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantCitation {
    pub dataset_id: String,
    pub dataset_name: String,
    pub dataset_arn: String,
    /// The ARN of the source the cited text was read through: the dataset's Kendra
    /// knowledge base when its source names one, and the dataset's own ARN otherwise.
    pub source_arn: String,
    /// The cited text, exactly as the dataset's record holds it.
    pub text: String,
}

/// One answered turn: the message served, the datasets it cites and the trace of how it was
/// arrived at.
///
/// The traces are composed whether or not the request asked for them; `enableTrace` decides
/// only whether they are SENT, which keeps the answer the same either way.
#[derive(Debug, Clone)]
pub struct AssistantAnswer {
    pub message: String,
    pub citations: Vec<AssistantCitation>,
    pub traces: Vec<String>,
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// Record one turn against the conversation `conversation_id` names — starting it when
    /// the request named none, or when the id names a conversation this store has evicted —
    /// and answer the conversation as it stands AFTER that turn.
    ///
    /// This is the WRITE `InvokeAssistant` performs: the operation "starts or continues a
    /// conversation", so the turn is what it records. Nothing else in this service writes a
    /// conversation.
    pub fn record_assistant_turn(
        &mut self,
        conversation_id: Option<&str>,
        now: SystemTime,
    ) -> AssistantConversationRecord {
        let id = match conversation_id {
            Some(named) => named.to_string(),
            None => loop {
                let candidate = new_id();
                if !self.assistant_conversations.contains_key(&candidate) {
                    break candidate;
                }
            },
        };
        if !self.assistant_conversations.contains_key(&id) {
            self.evict_assistant_conversations();
        }
        let conversation = self
            .assistant_conversations
            .entry(id.clone())
            .or_insert_with(|| AssistantConversationRecord {
                id,
                started_at: now,
                last_turn_at: now,
                turns: 0,
            });
        conversation.turns = conversation.turns.saturating_add(1);
        conversation.last_turn_at = now;
        conversation.clone()
    }

    /// The conversation `conversation_id` names, or `None` when this store holds none.
    pub fn assistant_conversation(
        &self,
        conversation_id: &str,
    ) -> Option<&AssistantConversationRecord> {
        self.assistant_conversations.get(conversation_id)
    }

    /// Make room for one more conversation: while the store is at
    /// [`MAX_ASSISTANT_CONVERSATIONS`], the least recently used conversation goes, the
    /// earlier-started one first and then the lower id, so one store evicts one way whatever
    /// order its map iterates in.
    fn evict_assistant_conversations(&mut self) {
        while self.assistant_conversations.len() >= MAX_ASSISTANT_CONVERSATIONS {
            let Some(evicted) = self
                .assistant_conversations
                .values()
                .min_by(|left, right| {
                    left.last_turn_at
                        .cmp(&right.last_turn_at)
                        .then_with(|| left.started_at.cmp(&right.started_at))
                        .then_with(|| left.id.cmp(&right.id))
                })
                .map(|conversation| conversation.id.clone())
            else {
                return;
            };
            self.assistant_conversations.remove(&evicted);
        }
    }

    /// The answer one turn of `conversation` serves for `message`.
    ///
    /// READ-ONLY over the account: the retrieval reads the datasets this store holds and
    /// writes nothing. The turn itself is recorded by [`Self::record_assistant_turn`],
    /// whose record this is passed.
    pub fn assistant_answer(
        &self,
        message: &str,
        conversation: &AssistantConversationRecord,
    ) -> AssistantAnswer {
        let mut terms = ::service_common::document_text_search::query_terms(message);
        // The work this turn does is bounded before any of it happens; see
        // [`MAX_ASSISTANT_QUERY_TERMS`].
        terms.truncate(MAX_ASSISTANT_QUERY_TERMS);
        let searched = self.datasets.len();
        let matched = self.assistant_retrieval(&terms);
        let citations: Vec<AssistantCitation> = matched
            .iter()
            .take(MAX_ASSISTANT_CITATIONS)
            .map(|(_, citation)| citation.clone())
            .collect();

        let mut answered = format!(
            "{EMULATED_ASSISTANT_NOTICE} This is turn {} of conversation {}. {}",
            conversation.turns,
            conversation.id,
            retrieval_clause(terms.len(), searched, matched.len(), citations.len()),
        );
        for citation in &citations {
            answered.push_str(&format!(
                " Dataset \"{}\" ({}) carries matching text: {}",
                citation.dataset_name, citation.dataset_id, citation.text,
            ));
        }

        let mut traces = vec![
            format!(
                "Read the {searched} datasets this account holds and matched the {} terms \
                 searched for against each dataset's id, name and description.",
                terms.len(),
            ),
            format!(
                "{} of them carry one of those terms, and {} are cited, most relevant \
                 first.",
                matched.len(),
                citations.len(),
            ),
        ];
        for citation in &citations {
            traces.push(format!(
                "Read dataset {} ({}) through its source {}.",
                citation.dataset_name, citation.dataset_id, citation.source_arn,
            ));
        }

        AssistantAnswer {
            message: answered,
            citations,
            traces,
        }
    }

    /// Every dataset this account holds that carries a term of `terms`, most relevant first
    /// — the share of the terms a dataset carries, then its name, then its id, so one store
    /// ranks one way whatever order its map iterates in.
    ///
    /// A dataset's searchable text is its id, its name and its description, which is
    /// everything of a dataset a caller can state in words; the excerpt reported is of the
    /// text the match was found in.
    fn assistant_retrieval(&self, terms: &[String]) -> Vec<(f64, AssistantCitation)> {
        let mut matched: Vec<(f64, AssistantCitation)> = self
            .datasets
            .values()
            .filter_map(|record| {
                let hit = ::service_common::document_text_search::match_document(
                    ::service_common::document_text_search::DocumentFields {
                        name: &record.id,
                        title: &record.name,
                        body: record.description.as_deref(),
                    },
                    terms,
                    EXCERPT_BOUNDS,
                )?;
                Some((
                    hit.score,
                    AssistantCitation {
                        dataset_id: record.id.clone(),
                        dataset_name: record.name.clone(),
                        dataset_arn: record.arn.clone(),
                        source_arn: assistant_source_arn(record),
                        text: hit.excerpt.text,
                    },
                ))
            })
            .collect();
        matched.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left.dataset_name.cmp(&right.dataset_name))
                .then_with(|| left.dataset_id.cmp(&right.dataset_id))
        });
        matched
    }
}

/// The ARN a citation reports as the source the cited text was read through.
///
/// `Source.arn` is "the ARN of the dataset. If the source is Kendra, it's the ARN of the
/// Kendra index", so a dataset whose source names a Kendra knowledge base is cited through
/// that knowledge base and every other dataset through its own ARN.
fn assistant_source_arn(record: &DatasetRecord) -> String {
    record
        .source
        .source_detail
        .as_ref()
        .and_then(|detail| detail.kendra.as_ref())
        .map(|kendra| kendra.knowledge_base_arn.clone())
        .unwrap_or_else(|| record.arn.clone())
}

/// What the answer says about the retrieval: which of the four cases this turn fell in, in
/// the numbers the account and the message made it fall in.
fn retrieval_clause(terms: usize, searched: usize, matched: usize, cited: usize) -> String {
    if terms == 0 {
        return "This message names no term to search the account's datasets for, so this \
                answer cites no dataset."
            .to_string();
    }
    if searched == 0 {
        return "This account holds no dataset, so there is nothing to read and this answer \
                cites no dataset."
            .to_string();
    }
    if matched == 0 {
        return format!(
            "None of the {searched} datasets this account holds carries one of the {terms} \
             terms searched for, so this answer cites no dataset."
        );
    }
    format!(
        "{matched} of the {searched} datasets this account holds carry one of the {terms} \
         terms searched for, and the {cited} most relevant are cited here."
    )
}

// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-polly-parrot-v1 service.
//!
//! Three stores live here, and every handler reaches them through [`service_state`]:
//!
//! * [`ComAmazonawsPollyParrotV1State::lexicons`] — the pronunciation lexicons
//!   `PutLexicon` writes and `GetLexicon`, `ListLexicons`, `DeleteLexicon` and
//!   `SynthesizeSpeech` read. Keyed by [`LexiconKey`] — the Region the lexicon was
//!   put in, then its name — in a `BTreeMap`, so one Region's lexicons sit
//!   contiguously in name order and `ListLexicons` reads them with a range scan
//!   without sorting on every call. Reach them through the accessors on the state
//!   rather than through the map, so no operation can read past its own Region.
//! * [`ComAmazonawsPollyParrotV1State::voices`] — the catalog of the voices Amazon
//!   Polly PROVIDES, which `DescribeVoices` serves. Seeded once from [`VOICE_CATALOG`]
//!   when the state is first touched and never written at runtime; no operation
//!   mutates a voice the service provides. A request may name a voice the catalog does
//!   not list, so a `VoiceId` is resolved through
//!   [`ComAmazonawsPollyParrotV1State::voice`] rather than over that field.
//! * [`ComAmazonawsPollyParrotV1State::custom_voices`] — the custom voices
//!   `CreateVoice` makes and `GetVoice`, `DescribeVoices` and `DeleteVoice` read.
//!   Keyed by [`CustomVoiceKey`] — the Region the voice was created in, then its id —
//!   for the same reason the lexicons are, and read through
//!   [`ComAmazonawsPollyParrotV1State::settled_custom_voices`], which materialises the
//!   status transition the clock already owes each of them ([`custom_voices`]).
//! * [`ComAmazonawsPollyParrotV1State::synthesis_tasks`] — the asynchronous speech
//!   synthesis tasks `StartSpeechSynthesisTask` creates and
//!   `GetSpeechSynthesisTask` and `ListSpeechSynthesisTasks` report. Keyed by
//!   `TaskId` in a `BTreeMap`, so the list read is ordered and paginable the same way.
//!   A task is kept for the documented [`TASK_RETENTION`] window and no longer, so
//!   every task read reaches the store through
//!   [`ComAmazonawsPollyParrotV1State::retained_synthesis_tasks`], which materialises
//!   what the clock already owes it. What a task's COMPLETION owes outside this service —
//!   the output object in the caller's bucket and the notification about the terminal
//!   status — travels with the task ([`TaskOutputDelivery`]) and is delivered by
//!   [`task_completion`].
//!
//! The state also carries the secret the paginated reads mint their `NextToken`s under
//! ([`PageTokenKey`]), which is what lets a read tell a continuation token this service
//! issued from one a caller built.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime};

/// What a completed synthesis task does OUTSIDE this service: the output object it writes
/// into the caller's bucket, and the notification it publishes about the terminal status it
/// reached.
pub mod task_completion;

/// The custom voices `CreateVoice` makes, and the one projection of a voice onto the
/// wire.
pub mod custom_voices;

/// WHICH resource a caller-supplied ARN addresses, and what a tag set has to satisfy.
pub mod resource_tags;

pub use custom_voices::{
    described_custom_voice, mint_custom_voice_id, provided_voice, reported_custom_voice,
    settle_voice_status, CustomVoiceKey, PendingVoiceStatus, StoredCustomVoice, StoredEncryption,
    StoredRecording, CUSTOM_VOICE_ENGINES, VOICE_ACTIVATION_DELAY, VOICE_ORIGIN_AWS,
    VOICE_ORIGIN_CUSTOMER, VOICE_STATUS_ACTIVE, VOICE_STATUS_FAILED, VOICE_STATUS_PENDING,
};
pub use resource_tags::{
    addressed_resource, reported_tags, tag_keys_are_settable, tags_after_adding, AddressedResource,
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX, RESOURCE_TYPE_LEXICON, RESOURCE_TYPE_VOICE,
};

/// WHERE a task's output is announced: the address this emulator serves that object at.
mod output_location;

pub use output_location::announced_output_uri;

/// The key one stored lexicon sits under: the Region it was put in, then its name.
///
/// A lexicon is a REGIONAL resource, so its name alone does not identify it: the
/// model's `LexiconNotFoundException` documents the error as caused by "specifying a
/// lexicon that is in a different region", and `PutLexicon` as overwriting "a lexicon
/// with the same name" that "already exists in the region".
///
/// Region first and name second, so the derived `Ord` lays one Region's lexicons out
/// contiguously in name order: [`ComAmazonawsPollyParrotV1State::lexicons_in_region`]
/// is then a range scan rather than a filter over the whole store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LexiconKey {
    /// The Region the lexicon lives in, as [`request_region`] read it from the
    /// storing call's credential scope.
    pub region: String,
    /// The lexicon's name, unique within `region`.
    pub name: String,
}

impl LexiconKey {
    /// The key for `name` in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// A pronunciation lexicon exactly as `PutLexicon` stored it, plus the attributes
/// derived from its PLS document at store time.
///
/// `content` is kept verbatim so `GetLexicon` can return the bytes the caller put,
/// character for character — the derived attributes are a *summary* of it, never a
/// replacement for it.
#[derive(Debug, Clone)]
pub struct StoredLexicon {
    /// The lexicon's name (the `@httpLabel` of every lexicon operation).
    pub name: String,
    /// The PLS document, byte-for-byte as `PutLexicon` received it.
    pub content: String,
    /// The PLS `alphabet` attribute, when the document carries one.
    pub alphabet: Option<String>,
    /// The PLS `xml:lang` attribute. Every stored lexicon has one: a PLS document
    /// element that declares no language is not a lexicon this service stores, so
    /// there is no stored lexicon whose reported attributes omit `LanguageCode`.
    pub language_code: String,
    /// When this lexicon was last written by `PutLexicon`.
    pub last_modified: SystemTime,
    /// `arn:aws:polly:<region>:<account>:lexicon/<name>`.
    pub lexicon_arn: String,
    /// Number of `<lexeme>` elements in `content`.
    pub lexemes_count: i32,
    /// Size of the lexicon in characters, which is what the model's
    /// `LexiconAttributes$Size` documents ("Total size of the lexicon, in
    /// characters").
    pub size: i32,
    /// The tags on this lexicon, in key order: the ones `PutLexicon` stored it with and
    /// the ones `TagResource` has added since. They live on the lexicon rather than in
    /// a store beside it, so one read answers both what the lexicon is and how it is
    /// tagged.
    pub tags: BTreeMap<String, String>,
}

/// One entry of the seeded voice catalog `DescribeVoices` serves.
///
/// Every field is `&'static` because the catalog is a compile-time constant: the
/// service has no operation that creates or edits a voice.
#[derive(Debug, Clone, Copy)]
pub struct StoredVoice {
    /// The `VoiceId` enum value, e.g. `"Joanna"` — the ASCII form of [`name`].
    ///
    /// [`name`]: StoredVoice::name
    pub id: &'static str,
    /// The voice's own name, spelled the way the voice's language spells it: a voice
    /// whose name carries diacritics keeps them here, and `id` is "the ASCII version
    /// of the voice name" (`DescribeVoices`' own documentation). For a voice whose
    /// name is already ASCII the two are the same string.
    pub name: &'static str,
    /// `"Female"` or `"Male"` — the `Gender` enum.
    pub gender: &'static str,
    /// The voice's default language, a `LanguageCode` enum value.
    pub language_code: &'static str,
    /// The language's name in English (`Voice$LanguageName`).
    pub language_name: &'static str,
    /// Extra languages a bilingual voice speaks (`Voice$AdditionalLanguageCodes`).
    pub additional_language_codes: &'static [&'static str],
    /// The `Engine` values this voice supports (`Voice$SupportedEngines`).
    pub supported_engines: &'static [&'static str],
}

impl StoredVoice {
    /// Whether this voice speaks `language`, counting its additional languages only
    /// when `include_additional` is set — the rule
    /// `DescribeVoices$IncludeAdditionalLanguageCodes` documents.
    pub fn speaks(&self, language: &str, include_additional: bool) -> bool {
        self.language_code == language
            || (include_additional && self.additional_language_codes.contains(&language))
    }

    /// Whether this voice is available on `engine`.
    pub fn supports_engine(&self, engine: &str) -> bool {
        self.supported_engines.contains(&engine)
    }
}

/// A speech synthesis task, as the asynchronous synthesis operations keep it.
///
/// Every member of the wire `SynthesisTask` shape is stored, so the read operations
/// report the task the start call actually recorded rather than a shape rebuilt from
/// a later request. `task_status` is the task's status NOW, and
/// [`StoredSynthesisTask::pending_status`] is the one the clock owes it.
#[derive(Debug, Clone)]
pub struct StoredSynthesisTask {
    /// The service-generated `TaskId`, which is also this entry's key.
    pub task_id: String,
    /// The engine the task synthesises with.
    pub engine: String,
    /// The task's `OutputFormat`.
    pub output_format: String,
    /// Where the finished output is written (`SynthesisTask$OutputUri`).
    pub output_uri: String,
    /// "Timestamp for the time the synthesis task was started".
    pub creation_time: SystemTime,
    /// "Number of billable characters synthesized".
    pub request_characters: i32,
    /// The SNS topic the start call asked for status notifications on, if any.
    pub sns_topic_arn: Option<String>,
    /// The lexicons the task applies.
    pub lexicon_names: Vec<String>,
    /// The requested sample rate, as the request spelled it.
    pub sample_rate: Option<String>,
    /// The requested speech mark types.
    pub speech_mark_types: Vec<String>,
    /// `text` or `ssml`.
    pub text_type: String,
    /// The voice the task synthesises with.
    pub voice_id: String,
    /// The language code the start call named, for a bilingual voice.
    pub language_code: Option<String>,
    /// The task's status now — a value of the model's `TaskStatus` enum.
    pub task_status: String,
    /// "Reason for the current status of a specific speech synthesis task".
    pub task_status_reason: Option<String>,
    /// The status the CLOCK owes this task, and when it falls due. A task Amazon
    /// Polly returns in a non-terminal status reaches its terminal one on its own,
    /// without any further call, so the due instant is stored and
    /// [`settle_task_status`] materialises it when a read arrives after it.
    pub pending_status: Option<PendingTaskStatus>,
    /// What this task's completion owes OUTSIDE this service — the output object it
    /// writes into the bucket the start call named, and the notification it publishes
    /// to the topic that call named — or `None` for a task with nothing left to
    /// deliver. See [`task_completion`], which is what delivers it.
    pub output_delivery: Option<TaskOutputDelivery>,
}

/// The side effects one task's COMPLETION has outside this service, and how far they
/// have got.
///
/// Amazon Polly's asynchronous API exists to put the rendered output in the caller's
/// bucket, so a task is not completed by the clock alone: the output object has to be
/// written (and the optional notification published) before the task is reported
/// terminal. This record carries what producing that object needs — the text, the
/// output location and the voice settings the start call resolved — so the delivery is
/// made from the task's own record rather than from a later request.
#[derive(Debug, Clone)]
pub struct TaskOutputDelivery {
    /// The Region the start call was signed for: the Region of the bucket the output is
    /// written to and of the topic the notification is published to.
    pub region: String,
    /// `OutputS3BucketName` — the bucket the output object is written into.
    pub bucket: String,
    /// The key the output object is written at, which is the key
    /// [`StoredSynthesisTask::output_uri`] reports.
    pub key: String,
    /// The `Text` the task synthesises.
    pub text: String,
    /// How far the delivery has got.
    pub state: DeliveryState,
}

/// How far a task's completion side effects have got.
///
/// The three states are what keep the delivery EXACTLY ONCE while two callers poll the
/// same task: the claim that moves [`DeliveryState::Owed`] to
/// [`DeliveryState::InFlight`] happens under the state lock, and only the claimant goes
/// on to write the object and publish the notification. A claim carries the instant it was
/// made, so a claimant that never reports back — the read it was made on was dropped —
/// cannot wedge the task: the claim is taken again once it cannot still be running. That
/// instant is a MONOTONIC one, so how long a claim has been outstanding never depends on
/// the wall clock the task's own timestamps are measured on — a clock that jumps backwards
/// cannot make a claim look stale and have the delivery made twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// Nothing has been delivered yet.
    Owed,
    /// One caller claimed the delivery at this instant and is making it.
    InFlight(Instant),
    /// The delivery was made (or it failed, and the task carries that failure).
    Done,
}

/// A status transition the clock owes a task.
#[derive(Debug, Clone)]
pub struct PendingTaskStatus {
    /// The instant from which this task is in `status`.
    pub due_at: SystemTime,
    /// The `TaskStatus` the task moves to.
    pub status: String,
    /// The `TaskStatusReason` that moves with it (a failure's reason).
    pub reason: Option<String>,
}

/// How long after the start call a synthesis task reaches its terminal status.
///
/// The emulator chooses this span: Amazon Polly's own synthesis time depends on the
/// text and the voice, and nothing here synthesises speech. It is deliberately NOT
/// zero, so a caller that polls immediately after starting a task still observes the
/// non-terminal status and the transition, and short enough that a test need not
/// wait on it.
pub const TASK_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How long a synthesis task is kept after it was started.
///
/// NOT a span the emulator chooses: the model documents it on
/// `StartSpeechSynthesisTask` — "The SpeechSynthesisTask object is available for 72
/// hours after starting the asynchronous synthesis task" — so the stored task is gone
/// once that long has passed since its `CreationTime`, and the read operations then
/// answer as they do for a `TaskId` that names no task.
pub const TASK_RETENTION: Duration = Duration::from_secs(72 * 60 * 60);

/// Whether `task` is still within its retention window at `now`.
///
/// The window is measured from the task's own `CreationTime` — "available for 72 hours
/// AFTER starting" — so the task is reportable strictly before `creation_time +
/// TASK_RETENTION` and gone from that instant on. A creation time so far in the future
/// that the sum is not representable is treated as retained: a task cannot expire
/// before it was created.
pub fn task_is_retained(task: &StoredSynthesisTask, now: SystemTime) -> bool {
    task.creation_time
        .checked_add(TASK_RETENTION)
        .is_none_or(|expires_at| now < expires_at)
}

/// Materialise the status transition the clock has ALREADY made due on `task`.
///
/// A read calls this before reporting a task: it changes nothing a caller could not
/// have observed by waiting, because the transition was owed at `now` whether or not
/// anybody called. It never moves a status that is not yet due.
pub fn settle_task_status(task: &mut StoredSynthesisTask, now: SystemTime) {
    // A task whose completion still owes its output object is NOT completed by the clock
    // alone: the object the task's own `OutputUri` names has to exist before any read
    // reports the task terminal, so the transition waits for the delivery
    // ([`task_completion`]) and that delivery is what applies it.
    if task
        .output_delivery
        .as_ref()
        .is_some_and(|delivery| delivery.state != DeliveryState::Done)
    {
        return;
    }
    if task.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
        let pending = task
            .pending_status
            .take()
            .expect("the pending status was just observed to be due");
        task.task_status = pending.status;
        task.task_status_reason = pending.reason;
    }
}

/// An optional list member of a reported resource: `Some` only when the resource has
/// values for it.
///
/// A resource that has none omits the member rather than carrying an empty array,
/// because the absent member and `[]` are different documents: the first says the
/// resource has no such values, the second says the service reports a list it did not
/// have to send. Every optional list member of a reported shape goes through here, so
/// no two of them can answer that differently.
pub fn reported_list<T: ToString>(values: &[T]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.iter().map(ToString::to_string).collect())
}

/// The one order every voice reports its supported engines in.
///
/// `Voice$SupportedEngines` is the SET of engines a voice is offered on — "Specifies
/// which engines (`standard`, `neural`, `long-form` or `generative`) are supported by a
/// given voice" — and the service reports that set in a single order for every voice,
/// newest engine first, which is the reverse of the order the model's `Engine` enum
/// declares its values in. Kept here as ONE list, so no catalog row can report its
/// engines in a different order from another row's.
pub const REPORTED_ENGINE_ORDER: [&str; 4] = ["generative", "long-form", "neural", "standard"];

/// `Voice$SupportedEngines` for one voice: its engines in [`REPORTED_ENGINE_ORDER`],
/// whatever order the catalog row happens to name them in, and absent entirely for a
/// voice with none (the rule [`reported_list`] states).
///
/// An engine the order above does not name — one a later model adds — is reported
/// after the ones it does, keeping the row's own relative order, so a new `Engine`
/// value is still reported rather than dropped.
pub fn reported_engines(engines: &[&str]) -> Option<Vec<String>> {
    let mut reported: Vec<&str> = engines.to_vec();
    reported.sort_by_key(|engine| {
        REPORTED_ENGINE_ORDER
            .iter()
            .position(|known| known == engine)
            .unwrap_or(REPORTED_ENGINE_ORDER.len())
    });
    reported_list(&reported)
}

/// The `SynthesisTask` wire shape for a stored task.
///
/// Shared by `GetSpeechSynthesisTask`, `ListSpeechSynthesisTasks` and
/// `StartSpeechSynthesisTask` so no two of them can report one task differently.
pub fn synthesis_task(stored: &StoredSynthesisTask) -> crate::ops::types::SynthesisTask {
    crate::ops::types::SynthesisTask {
        engine: Some(stored.engine.clone()),
        task_id: Some(stored.task_id.clone()),
        task_status: Some(stored.task_status.clone()),
        task_status_reason: stored.task_status_reason.clone(),
        output_uri: Some(stored.output_uri.clone()),
        creation_time: Some(::aws_smithy_types::DateTime::from(stored.creation_time)),
        request_characters: stored.request_characters,
        sns_topic_arn: stored.sns_topic_arn.clone(),
        lexicon_names: reported_list(&stored.lexicon_names),
        output_format: Some(stored.output_format.clone()),
        sample_rate: stored.sample_rate.clone(),
        speech_mark_types: reported_list(&stored.speech_mark_types),
        text_type: Some(stored.text_type.clone()),
        voice_id: Some(stored.voice_id.clone()),
        language_code: stored.language_code.clone(),
    }
}

/// Per-service state for com-amazonaws-polly-parrot-v1. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsPollyParrotV1State {
    /// Stored lexicons by Region and name. Ordered, so `ListLexicons` pages one
    /// Region deterministically.
    pub lexicons: BTreeMap<LexiconKey, StoredLexicon>,
    /// The seeded catalog of the voices Amazon Polly provides, in [`VOICE_CATALOG`]
    /// order.
    pub voices: Vec<StoredVoice>,
    /// The custom voices `CreateVoice` made, by Region and id. Ordered for the same
    /// reason as the lexicons: `DescribeVoices` reports one Region's deterministically.
    pub custom_voices: BTreeMap<CustomVoiceKey, StoredCustomVoice>,
    /// Speech synthesis tasks by `TaskId`. Ordered for the same reason as the
    /// lexicons: `ListSpeechSynthesisTasks` pages deterministically over it.
    pub synthesis_tasks: BTreeMap<String, StoredSynthesisTask>,
    /// The secret this store's `NextToken`s are authenticated under. PRIVATE, and
    /// reached only through [`ComAmazonawsPollyParrotV1State::page_token_key`]: a key a
    /// caller could read or replace would no longer tell a token this service minted
    /// from one it did not.
    page_token_key: PageTokenKey,
    /// How many task completions are being delivered right now
    /// ([`task_completion::deliver_when_due`]). PRIVATE, and taken and given back through
    /// the two methods below: it is what bounds the work a burst of start calls can leave
    /// outstanding.
    deliveries_in_flight: usize,
}

impl ComAmazonawsPollyParrotV1State {
    /// The custom voice `voice_id` of `region`, with whatever the clock already owes it
    /// materialised first — or `None` when that Region holds no such voice, which is
    /// what makes a read of another Region's voice the modelled "not found".
    ///
    /// `&mut self`, because reporting a voice settles the transition that fell due
    /// ([`settle_voice_status`]): a write that was owed at `now` whether or not anybody
    /// called, and the only write a read of this store makes.
    pub fn settled_custom_voice(
        &mut self,
        region: &str,
        voice_id: &str,
        now: SystemTime,
    ) -> Option<&StoredCustomVoice> {
        let voice = self.custom_voices.get_mut(&CustomVoiceKey::new(region, voice_id))?;
        settle_voice_status(voice, now);
        Some(voice)
    }

    /// Every custom voice `region` holds, in id order, each with what the clock owes it
    /// materialised.
    ///
    /// A range scan bounded by the Region, exactly as the lexicon read is: the keys of
    /// one Region are contiguous, so this reads that Region's entries and stops at the
    /// first key belonging to another.
    pub fn settled_custom_voices_in_region<'a>(
        &'a mut self,
        region: &'a str,
        now: SystemTime,
    ) -> impl Iterator<Item = &'a StoredCustomVoice> + 'a {
        // A minted id matches `^vc-[0-9a-f]+$`, so the empty id sorts before every real
        // one and excluding it excludes nothing.
        let start = CustomVoiceKey::new(region, "");
        self.custom_voices
            .range_mut((Bound::Excluded(start), Bound::Unbounded))
            .take_while(move |(key, _)| key.region == region)
            .map(move |(_, stored)| {
                settle_voice_status(stored, now);
                &*stored
            })
    }

    /// How many custom voices `region` holds — what the custom-voice count quota counts.
    pub fn custom_voice_count_in_region(&self, region: &str) -> usize {
        let start = CustomVoiceKey::new(region, "");
        self.custom_voices
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(|(key, _)| key.region == region)
            .count()
    }

    /// The voice `region` already created under `client_token`, if any.
    ///
    /// What makes `CreateVoice` idempotent: "If you retry a request that succeeded with
    /// the same client token and the same parameters, Amazon Polly returns the voice
    /// that the first request created and doesn't create a second voice."
    pub fn custom_voice_of_client_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&StoredCustomVoice> {
        let start = CustomVoiceKey::new(region, "");
        self.custom_voices
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(|(key, _)| key.region == region)
            .map(|(_, stored)| stored)
            .find(|stored| stored.client_token.as_deref() == Some(client_token))
    }

    /// Store `voice` in `region` under its own id, leaving every other Region untouched.
    pub fn store_custom_voice(&mut self, region: &str, voice: StoredCustomVoice) {
        self.custom_voices
            .insert(CustomVoiceKey::new(region, &voice.voice_id), voice);
    }

    /// Remove the custom voice `voice_id` of `region`, answering `None` — and deleting
    /// nothing — when that Region holds no such voice.
    pub fn remove_custom_voice(
        &mut self,
        region: &str,
        voice_id: &str,
    ) -> Option<StoredCustomVoice> {
        self.custom_voices
            .remove(&CustomVoiceKey::new(region, voice_id))
    }

    /// Whether `region` already holds a custom voice under `voice_id`.
    ///
    /// A read that reports existence alone, so a create can check a freshly minted id
    /// without taking the voice — or the status the clock owes it — into account.
    pub fn holds_custom_voice(&self, region: &str, voice_id: &str) -> bool {
        self.custom_voices
            .contains_key(&CustomVoiceKey::new(region, voice_id))
    }

    /// The tags of the resource `resource` addresses in `region`, or `None` when that
    /// Region holds no such resource.
    ///
    /// The ONE place a resource type an ARN names is mapped onto the resource it
    /// addresses, so the three tagging operations cannot disagree about which resources
    /// this service tags or about where a resource's tags live — they live ON the
    /// resource, so a resource that does not exist has no tags to report and a deleted
    /// one takes its tags with it.
    pub fn tags_of(
        &self,
        resource: &AddressedResource,
        region: &str,
    ) -> Option<&BTreeMap<String, String>> {
        match resource.resource_type {
            RESOURCE_TYPE_LEXICON => self.lexicon(region, &resource.id).map(|stored| &stored.tags),
            RESOURCE_TYPE_VOICE => self
                .custom_voices
                .get(&CustomVoiceKey::new(region, &resource.id))
                .map(|stored| &stored.tags),
            _ => None,
        }
    }

    /// The same tags for a WRITE: what `TagResource` and `UntagResource` change.
    pub fn tags_of_mut(
        &mut self,
        resource: &AddressedResource,
        region: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource.resource_type {
            RESOURCE_TYPE_LEXICON => self
                .lexicon_mut(region, &resource.id)
                .map(|stored| &mut stored.tags),
            RESOURCE_TYPE_VOICE => self
                .custom_voice_mut(region, &resource.id)
                .map(|stored| &mut stored.tags),
            _ => None,
        }
    }

    /// The lexicon `name` of `region`, or `None` when that Region stores none under
    /// that name — a lexicon of ANOTHER Region never answers here, which is what
    /// makes a cross-Region read the modelled "not found".
    pub fn lexicon(&self, region: &str, name: &str) -> Option<&StoredLexicon> {
        self.lexicons.get(&LexiconKey::new(region, name))
    }

    /// The lexicon `name` of `region` for a WRITE — a tag change, which is a change to
    /// the lexicon itself rather than to a store beside it.
    pub fn lexicon_mut(&mut self, region: &str, name: &str) -> Option<&mut StoredLexicon> {
        self.lexicons.get_mut(&LexiconKey::new(region, name))
    }

    /// The custom voice `voice_id` of `region` for a WRITE, for the same reason.
    pub fn custom_voice_mut(
        &mut self,
        region: &str,
        voice_id: &str,
    ) -> Option<&mut StoredCustomVoice> {
        self.custom_voices
            .get_mut(&CustomVoiceKey::new(region, voice_id))
    }

    /// Every lexicon `region` holds, in name order.
    ///
    /// A range scan bounded by the Region: the keys of one Region are contiguous, so
    /// this reads exactly that Region's entries and stops at the first key belonging
    /// to another.
    pub fn lexicons_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a StoredLexicon> + 'a {
        // A lexicon name matches `^[0-9A-Za-z]{1,20}$`, so the empty name sorts
        // before every real one and excluding it excludes nothing.
        let start = LexiconKey::new(region, "");
        self.lexicons
            .range((Bound::Excluded(start), Bound::Unbounded))
            .take_while(move |(key, _)| key.region == region)
            .map(|(_, stored)| stored)
    }

    /// How many lexicons `region` holds — what the per-Region count quota counts.
    pub fn lexicon_count_in_region(&self, region: &str) -> usize {
        self.lexicons_in_region(region).count()
    }

    /// Store `stored` in `region` under its own name, replacing whatever that Region
    /// held under that name and leaving every other Region untouched.
    pub fn store_lexicon(&mut self, region: &str, stored: StoredLexicon) {
        self.lexicons
            .insert(LexiconKey::new(region, &stored.name), stored);
    }

    /// Remove the lexicon `name` of `region`, answering `None` — and deleting
    /// nothing — when that Region holds none under that name.
    pub fn remove_lexicon(&mut self, region: &str, name: &str) -> Option<StoredLexicon> {
        self.lexicons.remove(&LexiconKey::new(region, name))
    }

    /// The key the paginated reads of this store mint and recognise their `NextToken`s
    /// under. Copied out rather than borrowed, so a read can check the token it was
    /// handed and go on to borrow the task store in the same call.
    ///
    /// `pub(crate)`: only this crate's own paginated read needs the key, and a key
    /// anything else could copy out would no longer tell a token this service minted
    /// from one it did not.
    pub(crate) fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }

    /// The voice a request's `VoiceId` names, whether or not the catalog lists it.
    ///
    /// A request may name any value of the model's `VoiceId` enum, which is wider than
    /// the offered catalog ([`UNLISTED_VOICES`]), so the lookup reads the catalog first
    /// and then the voices the catalog does not list — a synthesis request therefore
    /// reads the gender and engines of the voice it actually named, and never falls
    /// through to no voice at all.
    pub fn voice(&self, id: &str) -> Option<&StoredVoice> {
        self.voices
            .iter()
            .find(|voice| voice.id == id)
            .or_else(|| UNLISTED_VOICES.iter().find(|voice| voice.id == id))
    }

    /// Materialise everything the CLOCK already owes the task store at `now`, and
    /// answer the tasks that are left.
    ///
    /// Two clock-driven changes, in the order the clock makes them: a task past its
    /// [`TASK_RETENTION`] window is dropped, because it is no longer a task the service
    /// holds, and every task still held settles the status transition that fell due
    /// ([`settle_task_status`]). Both were owed at `now` whether or not anybody called,
    /// so a read may make them; neither depends on which read arrives first.
    ///
    /// Every task read goes through here, so no two of them can disagree about which
    /// tasks exist or what status they are in. What it hands back is SHARED: the two
    /// clock-driven writes above are the only ones a report makes, and a caller that
    /// reads through here cannot make another.
    pub fn retained_synthesis_tasks(
        &mut self,
        now: SystemTime,
    ) -> &BTreeMap<String, StoredSynthesisTask> {
        self.synthesis_tasks
            .retain(|_, task| task_is_retained(task, now));
        for task in self.synthesis_tasks.values_mut() {
            settle_task_status(task, now);
        }
        &self.synthesis_tasks
    }

    /// Take one of the [`task_completion::MAX_DELIVERIES_IN_FLIGHT`] delivery slots, or
    /// answer `false` when they are all taken — which is what keeps the deliveries a burst
    /// of start calls arms bounded. A caller that takes one gives it back with
    /// [`ComAmazonawsPollyParrotV1State::finish_delivery`].
    pub(crate) fn begin_delivery(&mut self) -> bool {
        if self.deliveries_in_flight >= task_completion::MAX_DELIVERIES_IN_FLIGHT {
            return false;
        }
        self.deliveries_in_flight += 1;
        true
    }

    /// Give back a delivery slot, however the delivery ended.
    pub(crate) fn finish_delivery(&mut self) {
        self.deliveries_in_flight = self.deliveries_in_flight.saturating_sub(1);
    }
}

impl Default for ComAmazonawsPollyParrotV1State {
    fn default() -> Self {
        Self {
            lexicons: BTreeMap::new(),
            // No custom voice exists until a create call makes one.
            custom_voices: BTreeMap::new(),
            // No task exists until a start call creates one.
            synthesis_tasks: BTreeMap::new(),
            // Seeded on first touch: `DescribeVoices` reports a catalog, and a
            // catalog that only appears after some other call would report an
            // empty voice list to the first caller.
            voices: VOICE_CATALOG.to_vec(),
            // One key per store, from the system's randomness: see `PageTokenKey`.
            page_token_key: PageTokenKey::new(),
            // No delivery is in flight until a task is started.
            deliveries_in_flight: 0,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPollyParrotV1State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPollyParrotV1State>)
}

/// The region this request was signed for, read from its SigV4 credential scope: the
/// region the ARNs it mints are scoped to, and the region its resources live in.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, which carries no scope.
/// The scope is caller-controlled text that lands in an ARN and keys the lexicon
/// store, so anything that is not an AWS region token is refused here and the default
/// used instead: a minted ARN can never carry text a client cannot parse back, and a
/// store key stays a short token.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_CHARACTERS
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a credential scope's region may carry to be read as one.
///
/// Comfortably above every published AWS region name, and short enough that the
/// region cannot itself be the unbounded part of a store key.
pub const MAX_REGION_CHARACTERS: usize = 64;

/// The region an unsigned request's ARNs are scoped to — the region the emulator's
/// generated client configuration uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `LexiconAttributes` wire shape for a stored lexicon.
///
/// Shared by `GetLexicon` and `ListLexicons` so the two cannot report different
/// attributes for one stored lexicon.
pub fn lexicon_attributes(stored: &StoredLexicon) -> crate::ops::types::LexiconAttributes {
    crate::ops::types::LexiconAttributes {
        alphabet: stored.alphabet.clone(),
        language_code: Some(stored.language_code.clone()),
        last_modified: Some(::aws_smithy_types::DateTime::from(stored.last_modified)),
        lexicon_arn: Some(stored.lexicon_arn.clone()),
        lexemes_count: stored.lexemes_count,
        size: stored.size,
    }
}

// ---------------------------------------------------------------------------
// Task identifiers
// ---------------------------------------------------------------------------

/// A fresh `TaskId`, in the form Amazon Polly mints one: a random UUID written in the
/// canonical hyphenated form.
///
/// Every `TaskId` the service hands out comes from here. What a READ accepts is not
/// narrowed to this form: the model's `TaskId` pattern (`^[a-zA-Z0-9_-]{1,100}$`) is
/// what a request has to satisfy, and an identifier that satisfies it simply names
/// whatever task the store holds under it — or no task at all.
pub fn mint_task_id() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// The secret this service's page tokens are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED.
///
/// A continuation token carries the position itself (there is no page kept on the side to
/// look up), so only the authenticator over that position tells a token this store minted
/// from one a caller wrote. A token minted under another store's key — another emulator
/// process, a restarted one — names no position here either.
pub use ::service_common::page_tokens::PageTokenKey;

/// Encode `cursor` as an opaque `NextToken` this store can later recognise as its own:
/// the authenticated bytes of the cursor, hex-encoded.
///
/// Hex is this service's spelling for an opaque token — it is what makes the value one a
/// caller round-trips rather than constructs, and a corrupted one detectable — while the
/// authenticator inside makes a token the service did NOT mint detectable too, which is
/// what lets a paginated read answer `InvalidNextTokenException` instead of silently
/// serving a page from a position the caller chose.
pub fn encode_page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The cursor a `NextToken` carries, or `None` for any token THIS store did not mint —
/// one that is not hex, one too short to carry an authenticator, one whose authenticator
/// does not match the cursor it travels with, and one whose cursor is not UTF-8.
pub fn decode_page_token(key: &PageTokenKey, token: &str) -> Option<String> {
    key.cursor_of(&::hex::decode(token).ok()?)
}

/// The position a request's `NextToken` names, or `None` when it names none.
///
/// The model admits the empty token (`NextToken` is len 0..4096) and the framework
/// binds a bare `?NextToken=` to `Some("")`, so a read is handed an empty token by a
/// caller that asked to continue from nowhere. An empty token names no position, so
/// every read of this service takes it for no token at all and answers the first page
/// rather than refusing it; only a token that names something is a token a read has to
/// recognise or refuse. This is the one place that distinction is made, so no read can
/// disagree with another about it.
pub fn requested_page_token(token: Option<&str>) -> Option<&str> {
    token.filter(|token| !token.is_empty())
}

/// Whether `token` is a continuation token an UNPAGINATED read was handed.
///
/// A read the model declares no pagination for reports its whole result in one
/// response and so mints no continuation token at all; every `NextToken` that names a
/// position ([`requested_page_token`]) presented to one is therefore a token this
/// service could not have issued, and the read refuses it with
/// `InvalidNextTokenException` instead of answering a silently partial list.
pub fn is_unmintable_page_token(token: Option<&str>) -> bool {
    requested_page_token(token).is_some()
}

// ---------------------------------------------------------------------------
// The seeded voice catalog
// ---------------------------------------------------------------------------

/// The voices `DescribeVoices` reports.
///
/// Every `id` is a value of the model's `VoiceId` enum
/// (`smithy_query(query="shape", shape="VoiceId")`), every `language_code` a value of
/// its `LanguageCode` enum, every `gender` a value of `Gender` and every entry of
/// `supported_engines` a value of `Engine`; the voice names, the language names and the
/// voice→language→engine assignments come from the AWS "Available voices" table
/// linked from `SynthesizeSpeech$Engine`'s documentation.
///
/// A voice's `name` is the name that table spells, diacritics included, and its `id` is
/// "the ASCII version of the voice name" (`DescribeVoices`) — which is why the eight
/// voices whose own names carry diacritics are the only rows whose `name` and `id`
/// differ. `language_name` is the language's name in English as that table names it
/// (so `de-CH` is "Swiss Standard German", not "Swiss German"), and one language is
/// named the same way by every voice that speaks it.
///
/// `Aditi` carries `hi-IN` as an additional language code because
/// `Voice$AdditionalLanguageCodes`'s own documentation states it: "the default
/// language for Aditi is Indian English (en-IN) ... this parameter would show the
/// code `hi-IN`" — which is the rule for every bilingual voice: its `language_code` is
/// the language it was first used for and its additional codes are the other languages
/// it speaks, so `hi-IN` and `arb` are reported as additional languages of the Indian
/// English and Gulf Arabic voices rather than as those voices' default language.
///
/// The catalog is the voices the service OFFERS, which is what this operation reports
/// — "the list of voices that are available for use when requesting speech synthesis".
/// The model's `VoiceId` enum is WIDER than it: the enum is the set of ids a request
/// may name, and it carries ids of voices the offered table does not list.
/// [`UNLISTED_VOICES`] holds those, so a request naming one is still resolved against
/// the voice it names rather than against nothing, and no voice the catalog reports is
/// hidden from a filtered query.
pub const VOICE_CATALOG: &[StoredVoice] = &[
    // ---- English ----
    voice("Danielle", "Female", "en-US", "US English", &[], &["generative", "neural"]),
    voice("Gregory", "Male", "en-US", "US English", &[], &["neural"]),
    voice("Ivy", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Joanna", "Female", "en-US", "US English", &[], &["generative", "neural", "standard"]),
    voice("Joey", "Male", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Justin", "Male", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Kendra", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Kevin", "Male", "en-US", "US English", &[], &["neural"]),
    voice("Kimberly", "Female", "en-US", "US English", &[], &["neural", "standard"]),
    voice("Matthew", "Male", "en-US", "US English", &[], &["generative", "neural", "standard"]),
    voice("Ruth", "Female", "en-US", "US English", &[], &["generative", "neural"]),
    voice("Salli", "Female", "en-US", "US English", &[], &["generative", "neural", "standard"]),
    voice("Stephen", "Male", "en-US", "US English", &[], &["generative", "neural"]),
    voice("Tiffany", "Female", "en-US", "US English", &[], &["generative"]),
    voice("Amy", "Female", "en-GB", "British English", &[], &["generative", "neural", "standard"]),
    voice("Arthur", "Male", "en-GB", "British English", &[], &["neural"]),
    voice("Brian", "Male", "en-GB", "British English", &[], &["generative", "neural", "standard"]),
    voice("Emma", "Female", "en-GB", "British English", &[], &["neural", "standard"]),
    voice("Geraint", "Male", "en-GB-WLS", "Welsh English", &[], &["standard"]),
    voice("Nicole", "Female", "en-AU", "Australian English", &[], &["standard"]),
    voice("Olivia", "Female", "en-AU", "Australian English", &[], &["generative", "neural"]),
    voice("Russell", "Male", "en-AU", "Australian English", &[], &["standard"]),
    voice("Aditi", "Female", "en-IN", "Indian English", &["hi-IN"], &["standard"]),
    voice("Kajal", "Female", "en-IN", "Indian English", &["hi-IN"], &["generative", "neural"]),
    voice("Raveena", "Female", "en-IN", "Indian English", &[], &["standard"]),
    voice("Aria", "Female", "en-NZ", "New Zealand English", &[], &["generative", "neural"]),
    voice("Ayanda", "Female", "en-ZA", "South African English", &[], &["generative", "neural"]),
    voice("Niamh", "Female", "en-IE", "Irish English", &[], &["generative", "neural"]),
    voice("Jasmine", "Female", "en-SG", "Singaporean English", &[], &["generative", "neural"]),
    // ---- German ----
    voice("Daniel", "Male", "de-DE", "German", &[], &["generative", "neural"]),
    voice("Hans", "Male", "de-DE", "German", &[], &["standard"]),
    voice("Lennart", "Male", "de-DE", "German", &[], &["generative"]),
    voice("Marlene", "Female", "de-DE", "German", &[], &["standard"]),
    voice("Vicki", "Female", "de-DE", "German", &[], &["generative", "neural", "standard"]),
    voice("Hannah", "Female", "de-AT", "Austrian German", &[], &["generative", "neural"]),
    voice("Sabrina", "Female", "de-CH", "Swiss Standard German", &[], &["generative", "neural"]),
    // ---- French ----
    voice("Ambre", "Female", "fr-FR", "French", &[], &["generative"]),
    voice_spelled("Celine", "Céline", "Female", "fr-FR", "French", &[], &["generative", "standard"]),
    voice("Florian", "Male", "fr-FR", "French", &[], &["generative"]),
    voice_spelled("Lea", "Léa", "Female", "fr-FR", "French", &[], &["generative", "neural", "standard"]),
    voice("Mathieu", "Male", "fr-FR", "French", &[], &["standard"]),
    voice_spelled("Remi", "Rémi", "Male", "fr-FR", "French", &[], &["generative", "neural"]),
    voice("Chantal", "Female", "fr-CA", "Canadian French", &[], &["standard"]),
    voice("Gabrielle", "Female", "fr-CA", "Canadian French", &[], &["generative", "neural"]),
    voice("Liam", "Male", "fr-CA", "Canadian French", &[], &["generative", "neural"]),
    voice("Isabelle", "Female", "fr-BE", "Belgian French", &[], &["generative", "neural"]),
    // ---- Spanish ----
    voice("Conchita", "Female", "es-ES", "Castilian Spanish", &[], &["standard"]),
    voice("Enrique", "Male", "es-ES", "Castilian Spanish", &[], &["standard"]),
    voice("Lucia", "Female", "es-ES", "Castilian Spanish", &[], &["generative", "neural", "standard"]),
    voice("Sergio", "Male", "es-ES", "Castilian Spanish", &[], &["generative", "neural"]),
    voice_spelled("Andres", "Andrés", "Male", "es-MX", "Mexican Spanish", &[], &["generative", "neural"]),
    voice("Mia", "Female", "es-MX", "Mexican Spanish", &[], &["generative", "neural", "standard"]),
    voice("Lupe", "Female", "es-US", "US Spanish", &[], &["generative", "neural", "standard"]),
    voice("Miguel", "Male", "es-US", "US Spanish", &[], &["standard"]),
    voice("Pedro", "Male", "es-US", "US Spanish", &[], &["generative", "neural"]),
    voice_spelled("Penelope", "Penélope", "Female", "es-US", "US Spanish", &[], &["standard"]),
    // ---- Italian ----
    voice("Adriano", "Male", "it-IT", "Italian", &[], &["neural"]),
    voice("Beatrice", "Female", "it-IT", "Italian", &[], &["generative"]),
    voice("Bianca", "Female", "it-IT", "Italian", &[], &["generative", "neural", "standard"]),
    voice("Carla", "Female", "it-IT", "Italian", &[], &["standard"]),
    voice("Giorgio", "Male", "it-IT", "Italian", &[], &["standard"]),
    voice("Lorenzo", "Male", "it-IT", "Italian", &[], &["generative"]),
    // ---- Portuguese ----
    voice("Camila", "Female", "pt-BR", "Brazilian Portuguese", &[], &["generative", "neural", "standard"]),
    voice("Ricardo", "Male", "pt-BR", "Brazilian Portuguese", &[], &["standard"]),
    voice("Thiago", "Male", "pt-BR", "Brazilian Portuguese", &[], &["neural"]),
    voice_spelled("Vitoria", "Vitória", "Female", "pt-BR", "Brazilian Portuguese", &[], &["neural", "standard"]),
    voice("Cristiano", "Male", "pt-PT", "Portuguese", &[], &["standard"]),
    voice_spelled("Ines", "Inês", "Female", "pt-PT", "Portuguese", &[], &["neural", "standard"]),
    // ---- East Asian ----
    voice("Kazuha", "Female", "ja-JP", "Japanese", &[], &["neural"]),
    voice("Mizuki", "Female", "ja-JP", "Japanese", &[], &["standard"]),
    voice("Takumi", "Male", "ja-JP", "Japanese", &[], &["neural", "standard"]),
    voice("Tomoko", "Female", "ja-JP", "Japanese", &[], &["neural"]),
    voice("Jihye", "Female", "ko-KR", "Korean", &[], &["neural"]),
    voice("Seoyeon", "Female", "ko-KR", "Korean", &[], &["generative", "neural", "standard"]),
    voice("Zhiyu", "Female", "cmn-CN", "Chinese Mandarin", &[], &["neural", "standard"]),
    voice("Hiujin", "Female", "yue-CN", "Cantonese", &[], &["neural"]),
    // ---- Arabic ----
    voice("Zeina", "Female", "arb", "Arabic", &[], &["standard"]),
    voice("Hala", "Female", "ar-AE", "Gulf Arabic", &["arb"], &["neural"]),
    voice("Zayd", "Male", "ar-AE", "Gulf Arabic", &["arb"], &["neural"]),
    // ---- Nordic ----
    voice("Astrid", "Female", "sv-SE", "Swedish", &[], &["standard"]),
    voice("Elin", "Female", "sv-SE", "Swedish", &[], &["neural"]),
    voice("Mads", "Male", "da-DK", "Danish", &[], &["standard"]),
    voice("Naja", "Female", "da-DK", "Danish", &[], &["standard"]),
    voice("Sofie", "Female", "da-DK", "Danish", &[], &["neural"]),
    voice("Ida", "Female", "nb-NO", "Norwegian", &[], &["neural"]),
    voice("Liv", "Female", "nb-NO", "Norwegian", &[], &["standard"]),
    voice("Suvi", "Female", "fi-FI", "Finnish", &[], &["neural"]),
    voice_spelled("Dora", "Dóra", "Female", "is-IS", "Icelandic", &[], &["standard"]),
    voice("Karl", "Male", "is-IS", "Icelandic", &[], &["standard"]),
    // ---- Other European ----
    voice("Laura", "Female", "nl-NL", "Dutch", &[], &["generative", "neural"]),
    voice("Lotte", "Female", "nl-NL", "Dutch", &[], &["standard"]),
    voice("Ruben", "Male", "nl-NL", "Dutch", &[], &["standard"]),
    voice("Lisa", "Female", "nl-BE", "Belgian Dutch", &[], &["generative", "neural"]),
    voice("Ewa", "Female", "pl-PL", "Polish", &[], &["generative", "standard"]),
    voice("Jacek", "Male", "pl-PL", "Polish", &[], &["standard"]),
    voice("Jan", "Male", "pl-PL", "Polish", &[], &["standard"]),
    voice("Maja", "Female", "pl-PL", "Polish", &[], &["standard"]),
    voice("Ola", "Female", "pl-PL", "Polish", &[], &["generative", "neural"]),
    voice("Jitka", "Female", "cs-CZ", "Czech", &[], &["neural"]),
    voice("Carmen", "Female", "ro-RO", "Romanian", &[], &["standard"]),
    voice("Arlet", "Female", "ca-ES", "Catalan", &[], &["neural"]),
    voice("Gwyneth", "Female", "cy-GB", "Welsh", &[], &["standard"]),
    voice("Burcu", "Female", "tr-TR", "Turkish", &[], &["neural"]),
    voice("Filiz", "Female", "tr-TR", "Turkish", &[], &["standard"]),
    voice("Maxim", "Male", "ru-RU", "Russian", &[], &["standard"]),
    voice("Tatyana", "Female", "ru-RU", "Russian", &[], &["standard"]),
];

/// The voices the model's `VoiceId` enum declares that the offered catalog
/// ([`VOICE_CATALOG`]) does not list.
///
/// `VoiceId` is the set of ids a REQUEST may name, and it is wider than the catalog:
/// a voice id gets into the enum when the voice exists, and the catalog
/// `DescribeVoices` reports is the narrower set of voices offered for synthesis. These
/// rows are therefore NOT reported by `DescribeVoices` — a discovery call must not
/// offer a voice the catalog does not list — but they are still resolved for a request
/// that names one ([`ComAmazonawsPollyParrotV1State::voice`]), so synthesis reads such
/// a voice's own gender and engines rather than reading nothing.
///
/// Every row is a generative-only voice, which is what the newer `VoiceId` values are.
pub const UNLISTED_VOICES: &[StoredVoice] = &[
    voice("Kenneth", "Male", "en-US", "US English", &[], &["generative"]),
    voice("Tina", "Female", "en-US", "US English", &[], &["generative"]),
    voice("Curtis", "Male", "en-GB", "British English", &[], &["generative"]),
    voice("Nate", "Male", "en-AU", "Australian English", &[], &["generative"]),
    voice("Kiara", "Female", "en-IN", "Indian English", &[], &["generative"]),
    voice("Arjun", "Male", "hi-IN", "Hindi", &[], &["generative"]),
    voice("Carolina", "Female", "es-MX", "Mexican Spanish", &[], &["generative"]),
    voice("Carlos", "Male", "es-US", "US Spanish", &[], &["generative"]),
    voice("Leo", "Male", "pt-BR", "Brazilian Portuguese", &[], &["generative"]),
    voice("Hyunduk", "Male", "ko-KR", "Korean", &[], &["generative"]),
];

/// One catalog row whose voice name is already ASCII, so `Voice$Name` and `Voice$Id`
/// are the same string. `const fn` so [`VOICE_CATALOG`] stays a compile-time constant
/// and the table above reads as data rather than as struct literals.
const fn voice(
    id: &'static str,
    gender: &'static str,
    language_code: &'static str,
    language_name: &'static str,
    additional_language_codes: &'static [&'static str],
    supported_engines: &'static [&'static str],
) -> StoredVoice {
    voice_spelled(
        id,
        id,
        gender,
        language_code,
        language_name,
        additional_language_codes,
        supported_engines,
    )
}

/// One catalog row whose voice name carries diacritics, so it is spelled out beside
/// the `VoiceId` enum value — which is "the ASCII version of the voice name".
#[allow(clippy::too_many_arguments)]
const fn voice_spelled(
    id: &'static str,
    name: &'static str,
    gender: &'static str,
    language_code: &'static str,
    language_name: &'static str,
    additional_language_codes: &'static [&'static str],
    supported_engines: &'static [&'static str],
) -> StoredVoice {
    StoredVoice {
        id,
        name,
        gender,
        language_code,
        language_name,
        additional_language_codes,
        supported_engines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row of either voice table must name values the model declares, or
    /// `DescribeVoices` would serve — or synthesis would resolve — a voice no SDK can
    /// deserialize.
    #[test]
    fn every_seeded_voice_uses_modeled_enum_values() {
        use crate::ops::types::{ENGINE_VALUES, GENDER_VALUES, LANGUAGE_CODE_VALUES, VOICE_ID_VALUES};
        for voice in VOICE_CATALOG.iter().chain(UNLISTED_VOICES) {
            assert!(
                VOICE_ID_VALUES.contains(&voice.id),
                "voice {:?}: id is not a modeled VoiceId",
                voice.id
            );
            assert!(
                GENDER_VALUES.contains(&voice.gender),
                "voice {:?}: gender {:?} is not a modeled Gender",
                voice.id,
                voice.gender
            );
            assert!(
                LANGUAGE_CODE_VALUES.contains(&voice.language_code),
                "voice {:?}: language code {:?} is not a modeled LanguageCode",
                voice.id,
                voice.language_code
            );
            for code in voice.additional_language_codes {
                assert!(
                    LANGUAGE_CODE_VALUES.contains(code),
                    "voice {:?}: additional language code {code:?} is not a modeled LanguageCode",
                    voice.id
                );
            }
            assert!(
                !voice.supported_engines.is_empty(),
                "voice {:?}: no supported engines",
                voice.id
            );
            for engine in voice.supported_engines {
                assert!(
                    ENGINE_VALUES.contains(engine),
                    "voice {:?}: engine {engine:?} is not a modeled Engine",
                    voice.id
                );
            }
        }
    }

    /// A `VoiceId` a request may name resolves to exactly one voice: the two tables
    /// together cover the model's whole enum and share no id, so synthesis reads the
    /// voice the request named whether or not the catalog lists it, and
    /// `DescribeVoices` reports each listed voice once.
    #[test]
    fn every_modeled_voice_id_resolves_to_exactly_one_voice() {
        use crate::ops::types::VOICE_ID_VALUES;
        let state = ComAmazonawsPollyParrotV1State::default();
        for id in VOICE_ID_VALUES {
            let resolved = state.voice(id);
            assert!(
                resolved.is_some_and(|voice| voice.id == *id),
                "voice {id:?}: a modelled VoiceId must resolve to its own voice, got {resolved:?}"
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for voice in VOICE_CATALOG.iter().chain(UNLISTED_VOICES) {
            assert!(
                seen.insert(voice.id),
                "voice {:?}: declared twice across the voice tables",
                voice.id
            );
        }
        assert_eq!(
            seen.len(),
            VOICE_ID_VALUES.len(),
            "the voice tables must declare each modelled VoiceId once, got {} for {}",
            seen.len(),
            VOICE_ID_VALUES.len()
        );
        // And the catalog is the narrower of the two: a voice the catalog does not list
        // is not reported, so it must not be in the catalog as well.
        for voice in UNLISTED_VOICES {
            assert!(
                !VOICE_CATALOG.iter().any(|listed| listed.id == voice.id),
                "voice {:?}: a voice the catalog does not list cannot also be listed",
                voice.id
            );
        }
    }

    /// Whatever engines a voice is offered on, and whatever order its catalog row names
    /// them in, the reported list is that same set in the service's one engine order —
    /// checked over EVERY subset of the modelled `Engine` values and every permutation
    /// of each, so no row's spelling can change the order a client sees.
    #[test]
    fn supported_engines_are_reported_in_one_order_whatever_the_row_says() {
        let engines = REPORTED_ENGINE_ORDER;
        for mask in 0..(1u8 << engines.len()) {
            let subset: Vec<&str> = engines
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, engine)| *engine)
                .collect();
            let expected: Option<Vec<String>> = (!subset.is_empty())
                .then(|| subset.iter().map(|engine| (*engine).to_string()).collect());

            // Every rotation of the subset is one spelling of the same set.
            for rotation in 0..subset.len().max(1) {
                let mut row = subset.clone();
                row.rotate_left(rotation);
                assert_eq!(
                    reported_engines(&row),
                    expected,
                    "case {row:?}: wrong reported engines"
                );
            }
        }

        // An engine the order does not name is still reported, after the ones it does.
        assert_eq!(
            reported_engines(&["whispered", "standard", "generative"]),
            Some(vec![
                "generative".to_string(),
                "standard".to_string(),
                "whispered".to_string(),
            ]),
            "an engine outside the reported order must still be reported"
        );
    }

    /// A catalog row's `name` is the voice's own spelling and its `id` the ASCII form of
    /// it, so a client that displays the name and synthesizes with the id reaches the
    /// same voice.
    #[test]
    fn a_voice_id_is_the_ascii_form_of_its_name() {
        let mut spelled = 0usize;
        for voice in VOICE_CATALOG.iter().chain(UNLISTED_VOICES) {
            assert!(
                voice.id.is_ascii(),
                "voice {:?}: an id is the ASCII form of the name",
                voice.id
            );
            assert_eq!(
                voice.name.chars().count(),
                voice.id.chars().count(),
                "voice {:?}: name {:?} and id must be the same name",
                voice.id,
                voice.name
            );
            if !voice.name.is_ascii() {
                spelled += 1;
            } else {
                assert_eq!(
                    voice.name, voice.id,
                    "voice {:?}: an ASCII voice name is its own id",
                    voice.id
                );
            }
        }
        assert!(
            spelled > 0,
            "the catalog must keep the diacritics of the voices whose names carry them"
        );
    }

    /// One stored task, as a start call would have recorded it: `task_id` identifies the
    /// case and `created` is its `CreationTime`, and a test overrides the members its
    /// own case is about. ONE definition for this module, so a new stored member is
    /// added here and nowhere else.
    fn stored_task(task_id: &str, created: SystemTime) -> StoredSynthesisTask {
        StoredSynthesisTask {
            task_id: task_id.to_string(),
            engine: "standard".to_string(),
            output_format: "mp3".to_string(),
            output_uri: format!("s3://bucket/{task_id}.mp3"),
            creation_time: created,
            request_characters: 5,
            sns_topic_arn: None,
            lexicon_names: Vec::new(),
            sample_rate: None,
            speech_mark_types: Vec::new(),
            text_type: "text".to_string(),
            voice_id: "Joanna".to_string(),
            language_code: None,
            task_status: "inProgress".to_string(),
            task_status_reason: None,
            pending_status: None,
            // A seeded task stands for one whose delivery is already behind it, so the
            // clock alone settles what it is owed.
            output_delivery: None,
        }
    }

    /// A pending status is materialised exactly when the clock has made it due, and
    /// never before — polling early must not complete a task.
    #[test]
    fn a_task_status_moves_only_when_the_clock_makes_it_due() {
        let now = SystemTime::now();
        let task = |due: SystemTime| {
            let mut task = stored_task("task", now);
            task.pending_status = Some(PendingTaskStatus {
                due_at: due,
                status: "completed".to_string(),
                reason: None,
            });
            task
        };

        for (case, due, expected) in [
            ("due in the future", now + TASK_COMPLETION_DELAY, "inProgress"),
            ("due now", now, "completed"),
            ("due in the past", now - TASK_COMPLETION_DELAY, "completed"),
        ] {
            let mut subject = task(due);
            settle_task_status(&mut subject, now);
            assert_eq!(
                subject.task_status, expected,
                "case {case:?}: wrong status, pending {:?}",
                subject.pending_status
            );
        }

        // A settled task keeps the status it reached: there is nothing further owed.
        let mut settled = task(now);
        settle_task_status(&mut settled, now);
        settle_task_status(&mut settled, now + TASK_COMPLETION_DELAY);
        assert_eq!(
            (settled.task_status.as_str(), settled.pending_status.is_some()),
            ("completed", false),
            "a settled task must stay settled"
        );
    }

    /// A task whose completion still owes its output object does NOT reach its terminal
    /// status on the clock alone: the status moves with the delivery, so no read can report
    /// the task completed before the object its own `OutputUri` names is there.
    #[test]
    fn a_due_status_waits_for_the_output_delivery() {
        let now = SystemTime::now();
        let delivery = |state: DeliveryState| TaskOutputDelivery {
            region: "us-east-1".to_string(),
            bucket: "polly-output".to_string(),
            key: "task.mp3".to_string(),
            text: "hello".to_string(),
            state,
        };
        // Every state of a delivery, plus the task that owes none at all.
        for (case, owed, expected) in [
            ("nothing to deliver", None, "completed"),
            ("the delivery is owed", Some(DeliveryState::Owed), "inProgress"),
            (
                "the delivery is being made",
                Some(DeliveryState::InFlight(Instant::now())),
                "inProgress",
            ),
            ("the delivery was made", Some(DeliveryState::Done), "completed"),
        ] {
            let mut subject = stored_task("task", now);
            subject.pending_status = Some(PendingTaskStatus {
                // Due well before now: the clock owes this transition.
                due_at: now - TASK_COMPLETION_DELAY,
                status: "completed".to_string(),
                reason: None,
            });
            subject.output_delivery = owed.map(delivery);
            settle_task_status(&mut subject, now);
            assert_eq!(
                subject.task_status, expected,
                "case {case:?}: wrong status, pending {:?}",
                subject.pending_status
            );
        }
    }

    /// A task is held for the documented retention window, measured from its own
    /// creation time, and is gone from the store once that window has elapsed — while a
    /// task still inside it keeps the status transition the clock owes it.
    #[test]
    fn the_task_store_holds_a_task_for_its_retention_window() {
        let now = SystemTime::now();
        let mut state = ComAmazonawsPollyParrotV1State::default();

        // Ages spanning the class: inside the window, on the published bound, and past
        // it — and one creation time in the future, which cannot have expired.
        let cases: [(&str, SystemTime, bool); 7] = [
            ("just created", now, true),
            ("an hour old", now - Duration::from_secs(3_600), true),
            (
                "a second inside the window",
                now - (TASK_RETENTION - Duration::from_secs(1)),
                true,
            ),
            ("exactly the window old", now - TASK_RETENTION, false),
            (
                "a millisecond past the window",
                now - (TASK_RETENTION + Duration::from_millis(1)),
                false,
            ),
            (
                "many windows old",
                now - (TASK_RETENTION * 10),
                false,
            ),
            ("created in the future", now + Duration::from_secs(60), true),
        ];
        for (case, created, _) in cases {
            let mut task = stored_task(case, created);
            // A due transition on every task, so a retained one must still settle it —
            // except where the task was created in the future and is owed nothing yet.
            if created <= now {
                task.pending_status = Some(PendingTaskStatus {
                    due_at: now - TASK_COMPLETION_DELAY,
                    status: "completed".to_string(),
                    reason: None,
                });
            }
            state.synthesis_tasks.insert(task.task_id.clone(), task);
        }

        let held = state.retained_synthesis_tasks(now);
        for (case, _, retained) in cases {
            let actual = held.contains_key(case);
            assert_eq!(
                actual, retained,
                "case {case:?}: held is {actual}, expected {retained}"
            );
            if retained {
                let task = &held[case];
                let expected_status = if task.creation_time > now {
                    "inProgress"
                } else {
                    "completed"
                };
                assert_eq!(
                    task.task_status, expected_status,
                    "case {case:?}: a retained task must still settle what it was owed"
                );
            }
        }

        // Nothing is owed a second time: the same reading drops nothing further.
        let count = held.len();
        assert_eq!(
            state.retained_synthesis_tasks(now).len(),
            count,
            "a second reading at the same instant must hold the same tasks"
        );
        // And the clock going on past every window empties the store.
        assert!(
            state
                .retained_synthesis_tasks(now + TASK_RETENTION * 2)
                .is_empty(),
            "every task must be gone once its window has elapsed"
        );
    }

    /// Every identifier the service mints satisfies the model's own `TaskId` pattern —
    /// otherwise no request could ever carry one back — and no identifier is minted
    /// twice, which is what keeps one task's record distinct from another's.
    #[test]
    fn a_minted_task_id_is_one_a_request_can_carry() {
        let mut minted = std::collections::BTreeSet::new();
        for round in 0..64 {
            let id = mint_task_id();
            // A minted id must satisfy the model's own `TaskId` pattern
            // (`^[a-zA-Z0-9_-]{1,100}$`), or no request could ever carry it.
            assert!(
                (1..=100).contains(&id.len())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "round {round}: the minted id {id:?} is outside the modeled TaskId pattern"
            );
            assert!(
                minted.insert(id.clone()),
                "round {round}: the minted id {id:?} was minted twice"
            );
        }
    }

    /// A lexicon exists only in the Region it was stored in: a read, a count and a
    /// removal in another Region see nothing of it.
    #[test]
    fn the_lexicon_store_is_partitioned_by_region() {
        let lexicon = |name: &str, region: &str| StoredLexicon {
            name: name.to_string(),
            content: "<lexicon/>".to_string(),
            alphabet: Some("ipa".to_string()),
            language_code: "en-US".to_string(),
            last_modified: SystemTime::now(),
            lexicon_arn: ::engine::emulator_arn("polly", region, &format!("lexicon/{name}")),
            lexemes_count: 1,
            size: 10,
            tags: BTreeMap::new(),
        };

        let mut state = ComAmazonawsPollyParrotV1State::default();
        // The same name in two regions, plus a second name in one of them.
        for (region, name) in [("us-east-1", "Shared"), ("eu-west-1", "Shared"), ("eu-west-1", "Only")] {
            state.store_lexicon(region, lexicon(name, region));
        }

        for (region, expected) in [
            ("us-east-1", vec!["Shared"]),
            ("eu-west-1", vec!["Only", "Shared"]),
            ("ap-southeast-2", Vec::new()),
        ] {
            let names: Vec<&str> = state
                .lexicons_in_region(region)
                .map(|stored| stored.name.as_str())
                .collect();
            assert_eq!(
                names, expected,
                "region {region:?}: wrong lexicons, store {:?}",
                state.lexicons.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                state.lexicon_count_in_region(region),
                expected.len(),
                "region {region:?}: the count must be that region's own"
            );
        }
        assert!(
            state.lexicon("us-east-1", "Only").is_none(),
            "a name stored only in eu-west-1 must not be readable in us-east-1"
        );

        // Removing in one region leaves the other region's same-named lexicon.
        assert!(
            state.remove_lexicon("ap-southeast-2", "Shared").is_none(),
            "removing a lexicon of another region must remove nothing"
        );
        assert!(
            state.remove_lexicon("us-east-1", "Shared").is_some(),
            "removing a lexicon of its own region must remove it"
        );
        assert!(
            state.lexicon("eu-west-1", "Shared").is_some(),
            "another region's lexicon of the same name must survive the delete"
        );
    }

    /// The secret behind the tokens is not in a rendering of the key, nor of the state
    /// that holds it: a minted token is the caller's, the key never leaves the process.
    #[test]
    fn a_debug_rendering_carries_no_page_token_key_material() {
        let state = ComAmazonawsPollyParrotV1State::default();
        let key = state.page_token_key();
        let rendered = format!("{key:?} {state:?}");
        assert!(
            rendered.contains("PageTokenKey(..)"),
            "the key must render redacted, got {rendered:?}"
        );
        // A minted token is the caller's to hold; the secret behind it is not, so no
        // rendering may carry the bytes a second token could be minted with.
        let token = encode_page_token(&key, "a cursor");
        assert!(
            !rendered.contains(&token),
            "case {token:?}: a rendering must not carry key material"
        );
    }

    /// A page token round-trips under the key that minted it, and anything that key did
    /// not mint is rejected rather than read as "start from the beginning" — including
    /// a well-formed token of another key, and a minted token with a byte changed.
    #[test]
    fn page_tokens_round_trip_and_reject_tokens_this_store_did_not_mint() {
        let key = PageTokenKey::new();
        let other = PageTokenKey::new();
        for cursor in ["", "A", "MyLexicon", "zzzz", "1761000000000\u{1f}task\u{1f}"] {
            let token = encode_page_token(&key, cursor);
            assert_eq!(
                decode_page_token(&key, &token).as_deref(),
                Some(cursor),
                "cursor {cursor:?}: token {token:?} did not round-trip"
            );
            assert_eq!(
                decode_page_token(&other, &token),
                None,
                "cursor {cursor:?}: token {token:?} must not decode under another key"
            );

            // The same cursor with an authenticator the caller made up: the shape is
            // exactly a minted token's, so only the authenticator can refuse it.
            let forged = ::hex::encode(
                [
                    &[0_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES][..],
                    cursor.as_bytes(),
                ]
                .concat(),
            );
            assert_eq!(
                decode_page_token(&key, &forged),
                None,
                "cursor {cursor:?}: forged token {forged:?} must not decode"
            );

            // Every single-byte change to a minted token, in the authenticator and in
            // the cursor.
            let minted = ::hex::decode(&token).expect("a minted token is hex");
            for index in 0..minted.len() {
                let mut tampered = minted.clone();
                tampered[index] ^= 0x01;
                let tampered = ::hex::encode(tampered);
                assert_eq!(
                    decode_page_token(&key, &tampered),
                    None,
                    "cursor {cursor:?}: byte {index} flipped ({tampered:?}) must not decode"
                );
            }
        }

        // Not hex at all, odd-length hex, hex too short to carry an authenticator, and
        // hex of bytes that are not UTF-8.
        for garbage in [
            "not-hex",
            "abc",
            "ff",
            "fffe",
            &::hex::encode("1\u{1f}x\u{1f}"),
            &::hex::encode([0xff_u8; ::service_common::page_tokens::AUTHENTICATOR_BYTES + 1]),
        ] {
            assert!(
                decode_page_token(&key, garbage).is_none(),
                "garbage token {garbage:?}: expected no cursor, got {:?}",
                decode_page_token(&key, garbage)
            );
        }
    }
}

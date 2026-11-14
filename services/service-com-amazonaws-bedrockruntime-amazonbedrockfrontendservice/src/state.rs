// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bedrockruntime-amazonbedrockfrontendservice service.
//!
//! Besides the state struct itself this module holds the pieces every Bedrock
//! Runtime handler shares, so no two operations grow their own copy:
//!
//! * the seeded **model inventory** (`models`) that `modelId` resolution and
//!   its refusals are driven from — a 400 `ValidationException` for an
//!   identifier the service cannot interpret, a 404
//!   `ResourceNotFoundException` for one naming a model or resource that is not
//!   available here;
//! * the **tokenizer** (`count_text_tokens`, `truncate_to_tokens`) that
//!   `CountTokens`, `Converse` and `InvokeModel` all count with, so the counts
//!   two operations report for the same text agree by construction;
//! * the **prompt-text extraction** rule used to count tokens in an
//!   `InvokeModel` body;
//! * the **async-invoke** records, their idempotency index and the clock-owed
//!   transition that moves a submitted invocation out of `InProgress`;
//! * the **prompt-cache** entries a Converse-API `cachePoint` marker writes
//!   (`prompt_cache`), which is what lets a later call with the same marked
//!   prefix report it as read from the cache instead of as input tokens;
//! * the **account-enforced guardrail** ([`enforced_guardrail`]) the Bedrock
//!   control plane installs, which every invocation surface evaluates its
//!   request against whether or not the request asked for a guardrail;
//! * the **metric publication** ([`invocation_metrics`]) that turns a served
//!   inference's token accounting, the request it is and the duration measured
//!   for it into `AWS/Bedrock` metric data, so the counts and the latency a
//!   response reports inline are the ones CloudWatch surfaces for it.
//!
//! No language model runs inside this emulator, so nothing here invents model
//! output and presents it as inference: a synchronous completion carries, in the
//! model family's own response envelope, text that says plainly that no model
//! ran ([`generation`]), together with the token accounting computed from the
//! real request; an asynchronous invocation writes the MANIFEST of the job it
//! ran to the caller's own output location and no media bytes it did not
//! generate ([`async_invoke_output`]); and the arguments a forced tool call
//! carries are the instantiation of the schema that tool was DECLARED with
//! ([`tool_input_arguments`]), not values a model inferred from the conversation.
//! Everything around that — resolution, validation, status, timestamps, ARNs,
//! paging and the invocation log — is real and computed from persisted state.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::ops::types::{
    AsyncInvokeOutputDataConfig, CacheDetail, CachePointBlock, CitationGeneratedContent,
    CitationSourceContent, ContentBlock, DocumentBlock, DocumentContentBlock, DocumentSource,
    GuardrailConverseContentBlock, InferenceConfiguration, Message, ReasoningContentBlock,
    SearchResultBlock, SystemContentBlock, Tag, TokenUsage, Tool, ToolChoice, ToolConfiguration,
    ToolInputSchema, ToolReference, ToolResultContentBlock, ToolUseBlock,
};

/// The `AWS/Bedrock` metric data a served inference publishes, beside the
/// `usage` it reports inline.
pub mod invocation_metrics;

/// The inline guardrail checks `InvokeGuardrailChecks` evaluates, and the
/// text-unit accounting it reports for them.
pub mod guardrail_checks;

/// The account-enforced guardrail the control plane installs, and how an
/// invocation is evaluated against it.
pub mod enforced_guardrail;

/// The guardrail revisions this plane can apply, as the control plane published
/// them.
pub mod guardrail_registry;

/// Applying a guardrail revision to submitted content, and what the evaluation
/// reports.
pub mod guardrail_evaluation;

/// The generation stage every inference surface answers from: the assistant text
/// a turn serves, which says plainly that no language model ran.
pub mod generation;

/// Which tool request a Converse-API turn makes for a request that offers tools, which is
/// what starts the tool-use round trip.
pub mod tool_use_turn;

/// How a streamed turn's content is cut into the increments its event stream delivers.
pub mod stream_framing;

/// The base models this plane resolves a `modelId` against, and how each one is served.
/// Here rather than in this file so one file holds one concern; the inventory the state
/// struct seeds itself with is this module's.
pub mod model_catalog;

/// The model's context window, and the refusal an input past it earns.
pub mod context_window;

/// What a finished asynchronous invocation writes to the caller's Amazon S3 location, and
/// the settle that delivers it.
pub mod async_invoke_output;

/// The OpenAI-compatible chat-completions `InvokeModel` envelope, which is the request and
/// response shape of the providers Amazon Bedrock serves on that schema.
pub mod openai_chat;

// Re-exported so every handler and every caller of this module reaches the
// generation stage where it reached it before: the stage MOVED into its own
// module, it did not change, and nothing about the text a turn serves differs.
pub use generation::{
    emulated_completion, emulated_completion_with, EmulatedCompletion, EMULATED_COMPLETION_FLOOR,
};

/// The most async-invoke records this emulator retains.
///
/// AWS publishes no retention for them, so this is the EMULATOR's bound, and it
/// exists so a caller cannot grow the store without limit: the oldest record is
/// evicted (with its idempotency-index entry) once the store passes it. It is
/// far above any test or client working set, so eviction never hides a record a
/// caller is still polling.
pub const MAX_ASYNC_INVOKE_RECORDS: usize = 1000;

/// The most inference-log entries this emulator retains: the log is a ring
/// buffer of the most recent calls, so one entry per request cannot grow without
/// limit either.
pub const MAX_INVOCATION_LOG_ENTRIES: usize = 1000;

/// The page size `ListAsyncInvokes` uses when the caller names no `maxResults`,
/// and the ceiling it clamps one to: the modelled `maxResults` maximum, so one
/// request cannot render an unbounded number of records into one response.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How deep a caller-supplied document or body is walked before the walk stops.
///
/// Keeps the recursion bound LOCAL to this crate rather than inherited from a
/// deserializer's own limit.
pub const MAX_DOCUMENT_DEPTH: usize = 64;

/// The most cached prompt prefixes this emulator retains.
///
/// The EMULATOR's bound, so a caller marking a fresh prefix on every call
/// cannot grow the store without limit: the entry closest to falling out of the
/// cache is evicted once the store passes it. Each entry holds a digest and a
/// token count, never the prompt text itself.
pub const MAX_PROMPT_CACHE_ENTRIES: usize = 256;

/// The most cache checkpoints one request can place in the cache.
///
/// The EMULATOR's bound, so the number of `cachePoint` markers a request
/// carries cannot decide how many entries one call inserts into (or evicts
/// from) the shared cache: the LONGEST prefixes a request marks are the ones
/// honoured, which leaves the prefix the last marker delimits — the one that
/// decides the input tokens charged — always among them.
pub const MAX_CACHE_CHECKPOINTS: usize = 4;

/// The time to live a cache checkpoint is written under when its `cachePoint`
/// names no `ttl`: the five minutes Amazon Bedrock publishes for a prompt cache
/// checkpoint ("Prompt caching for faster model inference" in the Amazon
/// Bedrock User Guide), restarted on every cache hit.
pub const DEFAULT_CACHE_TTL: &str = "5m";

/// Per-service state for com-amazonaws-bedrockruntime-amazonbedrockfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    /// The recognised foundation models, keyed by model id. Seeded by
    /// [`Default`], so `modelId` resolution and its refusals are state-driven.
    pub models: BTreeMap<String, ModelEntry>,
    /// Every async invocation this emulator still retains, keyed by
    /// `invocationArn`. Bounded by [`MAX_ASYNC_INVOKE_RECORDS`].
    pub async_invokes: BTreeMap<String, AsyncInvokeRecord>,
    /// `clientRequestToken` -> `invocationArn`, so `StartAsyncInvoke` is
    /// idempotent and can detect a reused token carrying a different payload.
    /// An entry is dropped with the record it points at, so this index cannot
    /// outgrow `async_invokes`.
    pub idempotency_index: HashMap<String, String>,
    /// The inference log: one entry per served inference call, as a ring buffer
    /// of the most recent [`MAX_INVOCATION_LOG_ENTRIES`].
    pub invocations: VecDeque<InvocationLogEntry>,
    /// The inline guardrail evaluations `InvokeGuardrailChecks` served, as a ring
    /// buffer of the most recent [`MAX_INVOCATION_LOG_ENTRIES`]: one entry per
    /// call, carrying the text units it consumed per configured check.
    pub guardrail_check_evaluations: VecDeque<guardrail_checks::EvaluationLogEntry>,
    /// The prompt prefixes a `cachePoint` marker has had cached, keyed by
    /// [`prompt_cache_key`]. Bounded by [`MAX_PROMPT_CACHE_ENTRIES`], and an
    /// entry leaves it once its time to live has run out.
    pub prompt_cache: BTreeMap<String, PromptCacheEntry>,
    /// The guardrail this account enforces on inference, as the Bedrock control
    /// plane's `PutEnforcedGuardrailConfiguration` installed it
    /// ([`enforced_guardrail`]). `None` while the account enforces none, which
    /// is what leaves an invocation unguarded unless its own request asks for a
    /// guardrail.
    pub enforced_guardrail: Option<enforced_guardrail::EnforcedGuardrail>,
    /// The guardrails this plane can APPLY, keyed by `guardrailId`, as the Bedrock
    /// control plane published each one's revisions
    /// ([`guardrail_registry`]). A guardrail this account does not hold is
    /// absent, which is what makes a request naming one answer that it was not
    /// found.
    pub guardrails: BTreeMap<String, guardrail_registry::PublishedGuardrail>,
    /// Monotonic counter mixed into every id this service mints (an async
    /// invocation's, a tool request's).
    pub next_id: u64,
}

impl Default for ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    fn default() -> Self {
        let mut models = BTreeMap::new();
        for entry in model_catalog::seed_models() {
            models.insert(entry.model_id.clone(), entry);
        }
        Self {
            models,
            async_invokes: BTreeMap::new(),
            idempotency_index: HashMap::new(),
            invocations: VecDeque::new(),
            guardrail_check_evaluations: VecDeque::new(),
            prompt_cache: BTreeMap::new(),
            enforced_guardrail: None,
            guardrails: BTreeMap::new(),
            next_id: 0,
        }
    }
}

impl ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState {
    /// Resolve a `modelId` as given on the wire to a registry entry, or `None`
    /// when this emulator does not recognise the model.
    ///
    /// Accepts the spellings the model's `modelId` pattern allows for a base
    /// model: the bare model id, a `foundation-model/<id>` ARN, an
    /// `inference-profile/<id>` ARN, and a cross-region inference profile id,
    /// which prefixes the model id with a geography (`us.`, `eu.`, `apac.`,
    /// `us-gov.`) — see "Supported Regions and models for cross-region
    /// inference" in the Amazon Bedrock User Guide.
    ///
    /// This is the raw registry lookup: it says nothing about whether the model
    /// can serve inference. An operation that is about to invoke a model calls
    /// [`Self::resolve_model_for_inference`] instead, which carries the refusal
    /// the service owes for an identifier it cannot serve.
    pub fn resolve_model(&self, model_id: &str) -> Option<&ModelEntry> {
        match interpret_model_id(model_id) {
            ModelIdentifier::BaseModel { key, .. } => self.models.get(key.as_str()),
            ModelIdentifier::AccountResource => None,
        }
    }

    /// Resolve a `modelId` for a call that NAMES a model without invoking it.
    ///
    /// The identifier is interpreted BEFORE the inventory is consulted, exactly as
    /// [`Self::resolve_model_for_inference`] describes, and the two failures are
    /// classified the same way — so a caller that only has to know WHICH model an
    /// identifier names states one refusal per class with the rest of the service.
    /// What it does not decide is the model's LIFECYCLE: "a model id stays a
    /// recognised identifier after its model reaches end of life", and whether a
    /// model still serves inference is a fact of an inference call, so a caller
    /// that runs none takes the entry the catalogue holds whatever its lifecycle.
    pub fn recognise_model(&self, model_id: &str) -> Result<&ModelEntry, ModelRejection> {
        let (key, named_by_arn) = match interpret_model_id(model_id) {
            ModelIdentifier::BaseModel { key, named_by_arn } => (key, named_by_arn),
            // A well-formed ARN of an account-scoped resource: a custom model, a
            // provisioned throughput, an imported model, a custom-model
            // deployment, a prompt, a prompt router or a SageMaker endpoint.
            // This emulator holds no such resource, so the ARN names one that
            // does not exist.
            ModelIdentifier::AccountResource => {
                return Err(ModelRejection::NotFound(format!(
                    "Could not resolve the resource named by the provided model identifier: \
                     {model_id}"
                )))
            }
        };
        match self.models.get(key.as_str()) {
            Some(entry) => Ok(entry),
            None if named_by_arn => Err(ModelRejection::NotFound(format!(
                "Could not resolve the model named by the provided model identifier: {model_id}"
            ))),
            None => Err(ModelRejection::Invalid(
                INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
            )),
        }
    }

    /// Resolve a `modelId` for a call that is about to run inference against it.
    ///
    /// The identifier is interpreted BEFORE the inventory is consulted, because
    /// the two failures are different faults with different statuses:
    ///
    /// * an identifier this service cannot interpret as naming a model it offers
    ///   — a bare id (or cross-region profile id) that is in no inventory — is a
    ///   client input fault: [`ModelRejection::Invalid`], answered with
    ///   `ValidationException` (400) and the service's fixed
    ///   [`INVALID_MODEL_IDENTIFIER_MESSAGE`];
    /// * an identifier that DOES name a resource — an ARN, or a base model the
    ///   inventory carries as retired — whose resource is not available here is
    ///   [`ModelRejection::NotFound`], answered with `ResourceNotFoundException`
    ///   (404).
    ///
    /// Keeping the two apart is what lets an SDK classify the failure: 400 is a
    /// non-retryable client fault, while 404 is routinely retried as
    /// eventual consistency.
    ///
    /// Which model an identifier names is [`Self::recognise_model`]'s; what this
    /// adds is the one refusal inference owes beyond it — a model past its end of
    /// life serves none.
    pub fn resolve_model_for_inference(&self, model_id: &str) -> Result<&ModelEntry, ModelRejection> {
        let entry = self.recognise_model(model_id)?;
        if entry.lifecycle == ModelLifecycle::EndOfLife {
            return Err(ModelRejection::NotFound(END_OF_LIFE_MESSAGE.to_string()));
        }
        Ok(entry)
    }

    /// The next async-invoke id: 12 lowercase alphanumeric characters, as the
    /// `InvocationArn` pattern requires.
    ///
    /// Derived from the state counter plus the request's own values, so two
    /// invocations never collide and the id is not a guessable sequence.
    pub fn mint_invocation_id(&mut self, model_id: &str, submit_millis: i64) -> String {
        use sha2::{Digest, Sha256};
        self.next_id += 1;
        let seed = format!("{}:{}:{}", self.next_id, model_id, submit_millis);
        let digest = Sha256::digest(seed.as_bytes());
        hex::encode(digest)[..12].to_string()
    }

    /// The id a tool request is issued under: the `tooluse_` prefix every
    /// Bedrock tool request carries, plus 22 hex characters, which satisfies the
    /// modelled `ToolUseId` (`@length` 1..64, `@pattern`
    /// `^[a-zA-Z0-9_.:-]+$`).
    ///
    /// Derived from the state counter plus the request's own values, as
    /// [`Self::mint_invocation_id`] is, so two requests never collide and the id
    /// is not a guessable sequence. The id is MINTED here, which is what lets a
    /// later turn be checked against the ids this service actually issued.
    pub fn mint_tool_use_id(&mut self, tool_name: &str, at_millis: i64) -> String {
        use sha2::{Digest, Sha256};
        self.next_id += 1;
        let seed = format!("{}:{}:{}", self.next_id, tool_name, at_millis);
        let digest = Sha256::digest(seed.as_bytes());
        format!("tooluse_{}", &hex::encode(digest)[..22])
    }

    /// The tool request this turn makes, or `None` when the turn generates text
    /// instead.
    ///
    /// `toolChoice` decides how much of this is the model's own
    /// ([`tool_use_turn`]): a MANDATORY choice (`any`, or `tool` naming one of the
    /// tools offered) obliges the turn to request a tool, and `auto` — the
    /// default, and what a request carrying no `toolChoice` gets — leaves the
    /// decision to the model, which is taken here from the state of the
    /// CONVERSATION: a turn whose latest message answers no tool request asks for
    /// the first tool the request offers, and the turn that is handed the
    /// `toolResult` for it answers in text.
    ///
    /// The request carries the arguments the chosen tool's own `inputSchema`
    /// describes ([`tool_input_arguments`]), so a client reading `toolUse.input`
    /// finds the argument object it declared the tool with.
    ///
    /// `Err` carries the refusal sentence for a choice the request's own tools
    /// cannot satisfy; the caller answers it with the operation's modelled
    /// `ValidationException` (400).
    pub fn tool_use_for_turn(
        &mut self,
        tool_config: Option<&ToolConfiguration>,
        messages: Option<&Vec<Message>>,
        at_millis: i64,
    ) -> Result<Option<ToolUseBlock>, String> {
        let Some(config) = tool_config else {
            return Ok(None);
        };
        let requestable = requestable_tool_names(config);
        let name = match &config.tool_choice {
            // `auto`, and a request that states no choice at all: the model
            // decides. The tools this request offers have not been called for the
            // latest prompt unless that prompt is itself the result of one, so the
            // turn requests the first tool offered — and answers in text once the
            // result is in hand. A tool list that offers no requestable tool
            // leaves nothing to request, which is text too.
            Some(ToolChoice::Auto(_)) | None => {
                if tool_use_turn::latest_turn_answers_a_tool_request(messages) {
                    return Ok(None);
                }
                match requestable.first() {
                    Some(name) => (*name).to_string(),
                    None => return Ok(None),
                }
            }
            // "The model must request at least one tool": the first tool the
            // request offers is the one requested, so the choice is answered
            // from the request rather than from a guess.
            Some(ToolChoice::Any(_)) => match requestable.first() {
                Some(name) => (*name).to_string(),
                None => {
                    return Err("The toolConfig must offer at least one tool for a toolChoice \
                         that requires the model to request one. Try again with a toolConfig \
                         whose tools include a tool the model can request."
                        .to_string())
                }
            },
            // "The Model must request the specified tool", which has to be one
            // of the tools the request passed.
            Some(ToolChoice::Tool(choice)) => {
                if !requestable.contains(&choice.name.as_str()) {
                    return Err(format!(
                        "The tool {} named by toolChoice is not one of the tools in \
                         toolConfig. Try again with a toolChoice that names a tool you \
                         passed in toolConfig.",
                        choice.name
                    ));
                }
                choice.name.clone()
            }
        };
        let tool_use_id = self.mint_tool_use_id(&name, at_millis);
        // The arguments the tool is requested with are the instantiation of the
        // schema that tool was DECLARED with ([`tool_input_arguments`]): a tool
        // is called with the argument object its own `inputSchema` describes.
        let input = tool_input_arguments(config, &name);
        Ok(Some(ToolUseBlock {
            tool_use_id,
            name,
            input,
            r#type: None,
        }))
    }

    /// Every invocation whose transition out of `InProgress` the clock has
    /// already made due at `now_millis`, as what delivering its output needs.
    ///
    /// Reading them is what lets the delivery run with no guard of this store
    /// held ([`async_invoke_output::settle_due_invocations`]): the transition was
    /// owed at this instant whether or not anybody called, and the output the
    /// invocation writes has to be in the caller's bucket before any read reports
    /// it finished.
    pub fn due_async_invokes(&self, now_millis: i64) -> Vec<async_invoke_output::DueInvocation> {
        self.async_invokes
            .values()
            .filter(|record| {
                record.status == AsyncInvokeStatus::InProgress && now_millis >= record.settles_at
            })
            .filter_map(async_invoke_output::DueInvocation::of)
            .collect()
    }

    /// Apply the terminal transition the delivery of one due invocation settled
    /// on: `Completed` when its output was written, and the modelled `Failed`
    /// with `failure` when it could not be.
    ///
    /// The instant stamped on the record is the one the transition was DUE at,
    /// not the instant the read that materialised it ran, so what a caller reads
    /// does not depend on when it asked. An invocation that has already left
    /// `InProgress` is terminal and is left exactly as it is.
    pub fn settle_async_invoke(&mut self, invocation_arn: &str, failure: Option<String>) {
        let Some(record) = self.async_invokes.get_mut(invocation_arn) else {
            return;
        };
        if record.status != AsyncInvokeStatus::InProgress {
            return;
        }
        record.status = match failure {
            None => AsyncInvokeStatus::Completed,
            Some(_) => AsyncInvokeStatus::Failed,
        };
        record.failure_message = failure;
        record.end_time = Some(record.settles_at);
        record.last_modified_time = Some(record.settles_at);
    }

    /// Persist a submitted invocation, index it under its idempotency token, and
    /// evict the oldest records once the store passes
    /// [`MAX_ASYNC_INVOKE_RECORDS`].
    pub fn insert_async_invoke(&mut self, record: AsyncInvokeRecord) {
        let invocation_arn = record.invocation_arn.clone();
        if let Some(token) = record.client_request_token.clone() {
            self.idempotency_index.insert(token, invocation_arn.clone());
        }
        self.async_invokes.insert(invocation_arn, record);
        while self.async_invokes.len() > MAX_ASYNC_INVOKE_RECORDS {
            let oldest = self
                .async_invokes
                .values()
                .min_by(|left, right| {
                    (left.submit_time, left.invocation_arn.as_str())
                        .cmp(&(right.submit_time, right.invocation_arn.as_str()))
                })
                .map(|record| record.invocation_arn.clone());
            let Some(oldest) = oldest else { break };
            let evicted = self.async_invokes.remove(&oldest);
            // Drop the index entry only while it still points at the evicted
            // record: a later invocation may have taken the same token over.
            if let Some(token) = evicted.and_then(|record| record.client_request_token) {
                if self
                    .idempotency_index
                    .get(&token)
                    .is_some_and(|arn| *arn == oldest)
                {
                    self.idempotency_index.remove(&token);
                }
            }
        }
    }

    /// Record one served inference call, keeping the log at most
    /// [`MAX_INVOCATION_LOG_ENTRIES`] entries long.
    pub fn log_invocation(&mut self, entry: InvocationLogEntry) {
        self.invocations.push_back(entry);
        while self.invocations.len() > MAX_INVOCATION_LOG_ENTRIES {
            self.invocations.pop_front();
        }
    }

    /// Record one served inline guardrail evaluation, keeping that log at most
    /// [`MAX_INVOCATION_LOG_ENTRIES`] entries long for the same reason.
    pub fn log_guardrail_checks(&mut self, entry: guardrail_checks::EvaluationLogEntry) {
        self.guardrail_check_evaluations.push_back(entry);
        while self.guardrail_check_evaluations.len() > MAX_INVOCATION_LOG_ENTRIES {
            self.guardrail_check_evaluations.pop_front();
        }
    }

    /// Run the prompt-cache stage of one Converse-API turn: match the prefixes
    /// this request's `cachePoint` markers delimit against the cache, write the
    /// ones that are not in it yet, and report the token accounting that
    /// follows.
    ///
    /// `None` when the request marks no cacheable prefix at all, which is what
    /// makes a turn report no cache counters: there was no cache to read from or
    /// write to.
    ///
    /// `plan` is [`prompt_cache_plan`]'s, computed by the caller BEFORE it takes
    /// the state guard, so digesting a prompt never runs under the lock this
    /// service's other requests wait on; it carries at most
    /// [`MAX_CACHE_CHECKPOINTS`] prefixes, in increasing length, so the longest
    /// one already cached is the one this request reads, everything from there
    /// to the last marker is what it writes, and one request can neither insert
    /// nor evict more than that many entries.
    pub fn apply_prompt_cache(
        &mut self,
        model_id: &str,
        plan: &PromptCachePlan,
        now_millis: i64,
    ) -> Option<PromptCacheOutcome> {
        if plan.checkpoints.is_empty() {
            return None;
        }
        // An entry whose time to live has run out is gone whether or not
        // anybody called, so the clock's expiries are materialised before this
        // request is matched against the cache.
        self.prompt_cache
            .retain(|_, entry| entry.expires_at > now_millis);

        let mut read_tokens = 0u32;
        for checkpoint in &plan.checkpoints {
            let key = prompt_cache_key(model_id, &checkpoint.digest);
            if let Some(entry) = self.prompt_cache.get_mut(&key) {
                // Reading a cached prefix restarts its time to live.
                entry.expires_at = now_millis.saturating_add(cache_ttl_millis(&entry.ttl));
                read_tokens = entry.tokens;
            }
        }

        // The prefix tokens this call puts in the cache: those between the
        // longest prefix it read and the last marker it carries, attributed to
        // the time to live each marker asked for.
        let mut writes: Vec<(String, u32)> = Vec::new();
        let mut cached_tokens = read_tokens;
        for checkpoint in &plan.checkpoints {
            if checkpoint.tokens <= cached_tokens {
                continue;
            }
            self.prompt_cache.insert(
                prompt_cache_key(model_id, &checkpoint.digest),
                PromptCacheEntry {
                    tokens: checkpoint.tokens,
                    ttl: checkpoint.ttl.clone(),
                    expires_at: now_millis.saturating_add(cache_ttl_millis(&checkpoint.ttl)),
                },
            );
            let written = checkpoint.tokens.saturating_sub(cached_tokens);
            match writes.iter_mut().find(|(ttl, _)| *ttl == checkpoint.ttl) {
                Some((_, tokens)) => *tokens = tokens.saturating_add(written),
                None => writes.push((checkpoint.ttl.clone(), written)),
            }
            cached_tokens = checkpoint.tokens;
            // Trimmed on every insert, so the store never holds more than its
            // bound even for the length of one call.
            self.evict_prompt_cache();
        }

        // `cacheDetails` is the breakdown of what this call WROTE, longest time
        // to live first, and is empty when it wrote nothing.
        writes.sort_by_key(|(ttl, _)| ::std::cmp::Reverse(cache_ttl_millis(ttl)));
        let write_tokens = writes
            .iter()
            .fold(0u32, |total, (_, tokens)| total.saturating_add(*tokens));
        Some(PromptCacheOutcome {
            read_tokens,
            write_tokens,
            cache_details: writes
                .into_iter()
                .map(|(ttl, tokens)| CacheDetail {
                    ttl,
                    input_tokens: token_count_i32(tokens),
                })
                .collect(),
            input_tokens: plan.tail_tokens,
        })
    }

    /// Keep the prompt cache at most [`MAX_PROMPT_CACHE_ENTRIES`] entries long,
    /// dropping the entry closest to falling out of it first.
    ///
    /// Called on every insert, so it has at most one entry to drop per call and
    /// at most [`MAX_CACHE_CHECKPOINTS`] per request: the scan for that entry is
    /// over a store a caller cannot grow past its bound.
    fn evict_prompt_cache(&mut self) {
        while self.prompt_cache.len() > MAX_PROMPT_CACHE_ENTRIES {
            let oldest = self
                .prompt_cache
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    (left.expires_at, left_key.as_str())
                        .cmp(&(right.expires_at, right_key.as_str()))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.prompt_cache.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState>)
}

// ---------------------------------------------------------------------------
// The model registry
// ---------------------------------------------------------------------------

/// The request/response body family a base model speaks on `InvokeModel`.
///
/// One variant per request/response envelope documented in "Inference request
/// parameters and response fields for foundation models" in the Amazon Bedrock
/// User Guide; the envelope each one emits is in
/// [`crate::state::invoke_model_response_body`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Anthropic Claude (Messages API).
    AnthropicMessages,
    /// Amazon Titan Text.
    TitanText,
    /// Amazon Nova (Converse-shaped body).
    Nova,
    /// Meta Llama.
    Llama,
    /// Mistral AI.
    Mistral,
    /// Cohere Command.
    CohereCommand,
    /// The OpenAI-compatible chat completions envelope, which is the published
    /// `InvokeModel` body format of every provider Amazon Bedrock onboards on that
    /// schema — the providers whose own API is the OpenAI chat-completions API. See
    /// [`openai_chat`].
    OpenAiChat,
}

/// What a base model takes and produces.
///
/// Modalities as published per model in "Supported foundation models in Amazon
/// Bedrock" in the Amazon Bedrock User Guide. Only [`ModelModality::Text`] has
/// an `InvokeModel` envelope in this emulator; an invocation of any other
/// modality is refused with the modelled error rather than answered with a
/// fabricated embedding, image or video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelModality {
    /// Text in, text out.
    Text,
    /// Text in, an embedding vector out.
    Embedding,
    /// An image out.
    Image,
    /// A video out.
    Video,
}

impl ModelModality {
    /// The modality as it reads in a refusal message.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Embedding => "embedding",
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

/// Where a base model is in the Bedrock model lifecycle.
///
/// "Model lifecycle" in the Amazon Bedrock User Guide: a model id stays a
/// recognised identifier after its model reaches end of life, but the model no
/// longer serves inference — which is a 404 on a resource that existed, not a
/// 400 on an identifier the service cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLifecycle {
    /// Available for inference.
    Active,
    /// Past its end-of-life date: the identifier resolves, the model is gone.
    EndOfLife,
}

/// One foundation model id this service recognises.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// The bare model id, e.g. `anthropic.claude-3-sonnet-20240229-v1:0`.
    pub model_id: String,
    /// Which `InvokeModel` body envelope the model speaks, or `None` when the model
    /// generates no text at all — every non-text modality, where this emulator computes
    /// no inference (see [`model_catalog`]). An invocation of one is refused, never
    /// answered with an invented payload.
    pub family: Option<ModelFamily>,
    /// What the model takes and produces.
    pub modality: ModelModality,
    /// Whether the model still serves inference at all.
    pub lifecycle: ModelLifecycle,
    /// Whether the model is served by the Converse API. Not every base model is
    /// — "Supported models and model features" in the Amazon Bedrock User Guide
    /// lists the Cohere Command (text) models as `InvokeModel`-only, for
    /// instance — and `Converse` must refuse one that is not.
    pub supports_converse: bool,
    /// Whether `CountTokens` serves this model. The count is the MODEL's own
    /// tokenizer's, so the operation is published per model family —
    /// `CountTokens`'s "Supported models" in the Amazon Bedrock User Guide lists
    /// the Anthropic Claude family — and a model outside that set is refused
    /// rather than answered with another tokenizer's count.
    pub supports_count_tokens: bool,
    /// Whether the model is invocable ASYNCHRONOUSLY, through
    /// `StartAsyncInvoke`. Asynchronous invocation is published per model, not
    /// per account — "Use asynchronous invocation for video generation" and
    /// "Supported foundation models in Amazon Bedrock" in the Amazon Bedrock
    /// User Guide list the video-generation models as the ones it serves,
    /// because the result of an asynchronous invocation is an artifact written
    /// to Amazon S3 rather than a response body — so a model outside that set
    /// must be refused at submit time rather than let into the async pipeline.
    pub supports_async_invoke: bool,
    /// The model's own ceiling on generated tokens, which caps
    /// `inferenceConfig.maxTokens`. Zero for a model that generates no text.
    pub max_output_tokens: u32,
    /// The model's own CONTEXT WINDOW: the most input tokens one request may send
    /// it (see [`model_catalog`]). It is a per-model property rather than a
    /// modelled constraint, and it is what an inference request's counted input is
    /// measured against ([`context_window`]) — an input past it is refused, never
    /// shortened. Zero for a model that takes no text prompt, which measures
    /// nothing.
    pub context_window: u32,
}

/// The service's fixed message for a `modelId` it cannot interpret as naming a
/// model it offers.
pub const INVALID_MODEL_IDENTIFIER_MESSAGE: &str = "The provided model identifier is invalid.";

/// The service's message for a model id whose model has reached end of life
/// ("Model lifecycle" in the Amazon Bedrock User Guide).
pub const END_OF_LIFE_MESSAGE: &str = "This model version has reached the end of its life. \
                                       Please refer to the AWS documentation for more details.";

/// Why a `modelId` cannot be served, as the modelled error the refusing
/// operation answers with. See
/// [`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::resolve_model_for_inference`].
#[derive(Debug, Clone)]
pub enum ModelRejection {
    /// Not an identifier this service can interpret as naming a model it
    /// offers: `ValidationException`, HTTP 400.
    Invalid(String),
    /// It names a model or a resource that is not available here:
    /// `ResourceNotFoundException`, HTTP 404.
    NotFound(String),
}

/// Geography prefixes a cross-region inference profile id puts in front of the
/// base model id. Source: "Supported Regions and models for cross-region
/// inference" in the Amazon Bedrock User Guide.
const INFERENCE_PROFILE_PREFIXES: [&str; 4] = ["us.", "eu.", "apac.", "us-gov."];

/// The ARN resource prefixes of the inference resources a `modelId` may name
/// that are NOT a base model, from the `InvokeModelIdentifier` pattern:
/// `custom-model`, `imported-model`, `provisioned-model`,
/// `custom-model-deployment`, `prompt`, `prompt-router`,
/// `default-prompt-router`, and a SageMaker `endpoint`. Each is created in an
/// account, so an ARN naming one this emulator has no record of names an absent
/// resource rather than an unreadable identifier.
const ACCOUNT_RESOURCE_SEGMENTS: [&str; 8] = [
    ":custom-model/",
    ":imported-model/",
    ":provisioned-model/",
    ":custom-model-deployment/",
    ":prompt/",
    ":prompt-router/",
    ":default-prompt-router/",
    ":endpoint/",
];

/// What a `modelId` as given on the wire names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelIdentifier {
    /// A base model, whose inventory key is `key`. `named_by_arn` records that
    /// the caller named it by ARN (a `foundation-model` or `inference-profile`
    /// ARN), which is a resource reference: when it resolves to nothing, the
    /// resource is absent rather than the identifier unreadable.
    BaseModel { key: String, named_by_arn: bool },
    /// An ARN of an account-scoped inference resource.
    AccountResource,
}

/// Interpret a `modelId` as given on the wire, without consulting the
/// inventory.
fn interpret_model_id(model_id: &str) -> ModelIdentifier {
    let (bare, named_by_arn) = if model_id.starts_with("arn:") {
        if ACCOUNT_RESOURCE_SEGMENTS
            .iter()
            .any(|segment| model_id.contains(segment))
        {
            return ModelIdentifier::AccountResource;
        }
        // A foundation-model ARN, or an inference-profile ARN whose resource id
        // is the (possibly geography-prefixed) model id.
        match ["foundation-model/", "inference-profile/"]
            .iter()
            .find_map(|segment| model_id.split_once(segment))
        {
            Some((_, resource)) => (resource, true),
            None => return ModelIdentifier::AccountResource,
        }
    } else {
        (model_id, false)
    };
    // A cross-region inference profile id is the base model id behind a
    // geography prefix, and resolves to the same model.
    let key = INFERENCE_PROFILE_PREFIXES
        .iter()
        .find_map(|prefix| bare.strip_prefix(prefix))
        .unwrap_or(bare);
    ModelIdentifier::BaseModel {
        key: key.to_string(),
        named_by_arn,
    }
}

/// The `foundation-model` ARN of a base model in `region`, as `GetAsyncInvoke`
/// reports it in `modelArn`.
pub fn foundation_model_arn(region: &str, model_id: &str) -> String {
    ::engine::emulator_arn("bedrock", region, &format!("foundation-model/{model_id}"))
}

/// The region an unsigned (anonymous) request is served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The AWS region this request was signed for, defaulting to [`DEFAULT_REGION`]
/// when the request carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// The inference log
// ---------------------------------------------------------------------------

/// Which API served an inference call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeKind {
    /// `InvokeModel`.
    Sync,
    /// `InvokeModelWithResponseStream`.
    SyncStream,
    /// `StartAsyncInvoke`.
    Async,
    /// `Converse`.
    Converse,
    /// `ConverseStream`.
    ConverseStream,
}

/// One served inference call.
#[derive(Debug, Clone)]
pub struct InvocationLogEntry {
    pub model_id: String,
    pub kind: InvokeKind,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Epoch milliseconds at which the call was served.
    pub at_millis: i64,
}

// ---------------------------------------------------------------------------
// Async invocations
// ---------------------------------------------------------------------------

/// The lifecycle status of an async invocation, as modelled by
/// `AsyncInvokeStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncInvokeStatus {
    InProgress,
    Completed,
    Failed,
}

impl AsyncInvokeStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "InProgress",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    /// Parse a modelled enum value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "InProgress" => Some(Self::InProgress),
            "Completed" => Some(Self::Completed),
            "Failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// How long after `StartAsyncInvoke` the invocation leaves `InProgress`.
///
/// AWS documents no duration for this, so the span is CHOSEN: half a second,
/// inside this project's 250–750 ms window for a clock-owed transition, so a
/// caller that reads the record straight after submitting it still observes
/// `InProgress` and the state errors that go with it. The due instant is stored
/// on the record ([`AsyncInvokeRecord::settles_at`]) and applied when a
/// reporting operation next runs; nothing sleeps.
pub const ASYNC_INVOKE_SETTLE_MILLIS: i64 = 500;

/// Why a submitted async invocation ends in `Failed`.
///
/// An asynchronous invocation's result is the artifact it writes to the caller's
/// own Amazon S3 location, so an invocation that could not write there carried
/// out none of the work it was accepted for: it ends in the modelled failure
/// state with the reason, rather than reporting a `Completed` whose output no
/// read of the object store would find
/// ([`async_invoke_output::settle_due_invocations`]).
pub const ASYNC_INVOKE_FAILURE_MESSAGE: &str =
    "The invocation's output could not be written to the Amazon S3 location the request \
     configured.";

/// The service's message for a `modelId` that names a model this service
/// recognises but which is not invocable asynchronously
/// ([`ModelEntry::supports_async_invoke`]).
pub const ASYNC_INVOKE_UNSUPPORTED_MESSAGE: &str =
    "The provided model doesn't support async inference.";

/// The service's message for an invocation ARN it cannot read as naming one of
/// its asynchronous invocations.
pub const INVALID_ASYNC_INVOKE_ARN_MESSAGE: &str = "The provided Async Invoke arn is not valid";

/// The service's message for a readable resource ARN that names a region other
/// than the one the request was made in: each region serves its own
/// invocations, so an ARN of another region names nothing this endpoint can
/// report on.
pub const FOREIGN_REGION_RESOURCE_ARN_MESSAGE: &str =
    "The provided ARN is invalid for the service region";

/// The service's message for an invocation ARN that is readable, is this
/// account's and this region's, and names no invocation this service holds.
///
/// The placeholder is part of the service's own fixed text, not a slot for the
/// requested ARN: the recorded refusal carries the template UNFILLED (the
/// behavioural check `bedrock-runtime.cross-account-async-invoke-not-visible`
/// asserts the message verbatim, braces included), and a client reads the ARN it
/// asked about from its own request.
pub const ASYNC_INVOKE_NOT_FOUND_MESSAGE: &str = "jobArn: {} is not found.";

/// The service's message for a resource ARN whose account field is not the
/// caller's own: the ARN is readable, so the refusal is an authorization one
/// (`AccessDeniedException`, HTTP 403) and not an input one.
pub const CROSS_ACCOUNT_RESOURCE_ARN_MESSAGE: &str =
    "The provided resource ARN is from a different account.";

/// The longest resource ARN a caller may name, in characters: the `@length`
/// maximum every ARN shape of this model carries
/// (`smithy_query(query="shape",shape="InvocationArn")`). A value past it is not
/// a readable reference.
pub const MAX_RESOURCE_ARN_LENGTH: usize = 2048;

/// Why a resource ARN cannot be resolved for the caller that named it.
///
/// The variants are the ORDER the resolution runs in, which is what decides
/// which refusal a caller sees when more than one holds: an ARN is read as this
/// service's resource, placed in a region, attributed to an account, read for
/// the invocation id it names, and only then looked up. Each variant carries the
/// status and code its refusal is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnRejection {
    /// Not readable as an ARN of this service's resource, or not naming an
    /// invocation id of the form this service mints: `ValidationException`,
    /// HTTP 400.
    Unreadable,
    /// Readable, but naming another region: `ValidationException`, HTTP 400.
    ForeignRegion,
    /// Readable and in region, but owned by another account:
    /// `AccessDeniedException`, HTTP 403.
    ForeignAccount,
}

impl ArnRejection {
    /// The service's own sentence for this rejection.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Unreadable => INVALID_ASYNC_INVOKE_ARN_MESSAGE,
            Self::ForeignRegion => FOREIGN_REGION_RESOURCE_ARN_MESSAGE,
            Self::ForeignAccount => CROSS_ACCOUNT_RESOURCE_ARN_MESSAGE,
        }
    }
}

/// Resolve `value` as an invocation ARN this endpoint may report on, for a
/// request served in `region`.
///
/// `Ok(())` says the ARN is one this endpoint owns the records for — whether or
/// not a record exists, which is the CALLER's next question and this service's
/// store's answer. The rungs are tried in the order [`ArnRejection`] documents,
/// so a reference that is wrong in more than one way is reported on the first
/// rule it breaks, and an ARN is never attributed to an account before it has
/// been read as this service's.
///
/// The invocation ID is read LAST, after the region and the owner: an ARN whose
/// id is not of the minted form is still placed and attributed first, which is
/// what lets a reference belonging to another account be refused as that rather
/// than as a bad id.
pub fn resolve_invocation_arn(value: &str, region: &str) -> Result<(), ArnRejection> {
    if value.chars().count() > MAX_RESOURCE_ARN_LENGTH {
        return Err(ArnRejection::Unreadable);
    }
    let arn = ::service_common::arn::parse(value).ok_or(ArnRejection::Unreadable)?;
    // The resource this operation reads is an async invocation of THIS service,
    // so an ARN of another service, or of another of this service's resources,
    // is not a reference it can read. `resource_id_of_type` is the shared reader
    // of a `<type>/<id>` (or `<type>:<id>`) resource field.
    let invocation_id = (arn.vendor == ASYNC_INVOKE_ARN_VENDOR)
        .then(|| ::service_common::arn::resource_id_of_type(value, ASYNC_INVOKE_ARN_RESOURCE_TYPE))
        .flatten()
        .ok_or(ArnRejection::Unreadable)?;
    if arn.region != region {
        return Err(ArnRejection::ForeignRegion);
    }
    if arn.account != ::engine::emulator_account_id() {
        return Err(ArnRejection::ForeignAccount);
    }
    // The id the modelled `InvocationArn` admits, which is also the only form
    // `mint_invocation_id` produces: an id outside it names no invocation this
    // service could hold, so the reference is unreadable rather than absent.
    if !is_minted_invocation_id(invocation_id) {
        return Err(ArnRejection::Unreadable);
    }
    Ok(())
}

/// Whether `id` is an invocation id of the form this service mints: the twelve
/// lowercase alphanumerics the modelled `InvocationArn` `@pattern` admits after
/// `async-invoke/` (`smithy_query(query="shape",shape="InvocationArn")`), which
/// is what [`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::mint_invocation_id`]
/// produces.
fn is_minted_invocation_id(id: &str) -> bool {
    id.len() == INVOCATION_ID_LENGTH
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// How many characters an invocation id carries.
const INVOCATION_ID_LENGTH: usize = 12;

/// The ARN vendor (service namespace) field every resource of this service
/// carries: the model's `arnNamespace`
/// (`smithy_query(query="service_info")`), which is the control plane's
/// namespace and not this data plane's endpoint prefix.
const ASYNC_INVOKE_ARN_VENDOR: &str = "bedrock";

/// The resource type an invocation ARN addresses
/// (`smithy_query(query="shape",shape="InvocationArn")`).
const ASYNC_INVOKE_ARN_RESOURCE_TYPE: &str = "async-invoke";

/// The service's message for a guardrail REFERENCE it cannot read as naming a
/// guardrail.
///
/// Carried by the `guardrailIdentifier` an `ApplyGuardrail` request names in its
/// URI and the one an `InvokeModel` / `InvokeModelWithResponseStream` request
/// names in its `X-Amzn-Bedrock-GuardrailIdentifier` header — the wording those
/// operations' own member documentation describes for a guardrail the request
/// enables but states wrongly, and what the recorded refusal of an unreadable
/// identifier carries (`bedrock-runtime-apply-guardrail-error-paths#task#1`).
pub const GUARDRAIL_REFERENCE_FORMAT_MESSAGE: &str =
    "Guardrail was enabled but input is in incorrect format.";

/// The service's message for a request it refuses without naming a member of it:
/// the refusal a value outside what the service can read gets where the service
/// states no reference wording of its own. Recorded for an unreadable guardrail
/// VERSION (`bedrock-runtime-apply-guardrail-error-paths#task#2`).
pub const INVALID_REQUEST_MESSAGE: &str = "The provided request is not valid";

/// The GUARDRAIL REFERENCE members a request names outside its body, in the
/// order the service reads them, each with the service's own wording for a value
/// it cannot read.
///
/// Bedrock refuses a guardrail reference it cannot read in its own words for that
/// reference rather than with the per-member constraint envelope a BODY member
/// outside its constraints gets. The two entries are the `guardrailIdentifier`
/// and `guardrailVersion` of
/// `/guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply` and of
/// `InvokeModel`'s and `InvokeModelWithResponseStream`'s guardrail headers
/// (`smithy_query(query="digest")`), which carry the same two member paths and
/// the same wording. No BODY member of this service is reached by either path:
/// `Converse` and `ConverseStream` carry their guardrail reference nested under
/// `guardrailConfig`, so a refusal there names
/// `guardrailConfig.guardrailIdentifier` and keeps the envelope.
///
/// The ORDER is what decides a request that is wrong in both: the resource is
/// read before the qualifier that selects one of its versions, so a request whose
/// IDENTIFIER cannot be read is refused on the guardrail, and only a request
/// naming a readable guardrail is refused on the version it asked for.
///
/// `modelId` is NOT here, and neither is `invocationArn`. The first is a BODY
/// member of `StartAsyncInvoke` as well as the label of five inference URIs, and
/// this seam cannot tell the two apart — it is handed the violation, not the
/// operation — so a `modelId` outside its constraints keeps the per-member
/// envelope on both surfaces. The second is resolved by its own operation
/// ([`resolve_invocation_arn`]) rather than at the boundary, because its REGION
/// and its OWNER are answered before its form is.
pub const RESOURCE_REFERENCES: [(&str, &str); 2] = [
    ("guardrailIdentifier", GUARDRAIL_REFERENCE_FORMAT_MESSAGE),
    ("guardrailVersion", INVALID_REQUEST_MESSAGE),
];

/// The order this service lists its CONTENT-FILTER CATEGORIES in wherever it
/// names more than one of them — the value set a refusal of the category member
/// quotes.
///
/// It is the service's own order and not a model artefact: the model's two enums
/// over the same five categories
/// (`smithy_query(query="shape",shape="GuardrailChecksContentFilterCategory,\
/// GuardrailContentFilterType")`) declare them in two different orders, and in
/// neither of these, so the order a message carries has to be stated once here
/// rather than taken from either. The recorded refusal that names the category
/// member states this order (`bedrock-runtime-guardrail-checks-inline#task#2`).
/// The MEMBERSHIP is the model's: the five values are exactly
/// `GUARDRAIL_CHECKS_CONTENT_FILTER_CATEGORY_VALUES`, which the crate's own test
/// pins.
pub const CONTENT_FILTER_CATEGORY_ORDER: [&str; 5] =
    ["INSULTS", "HATE", "MISCONDUCT", "SEXUAL", "VIOLENCE"];

/// The order this service lists the ROLES a message may carry in — the value set
/// a refusal of the role member quotes.
///
/// The model declares the same three in another order
/// (`smithy_query(query="shape",shape="GuardrailChecksRole")`), and the recorded
/// refusal that names the role member states this one
/// (`bedrock-runtime-invoke-guardrail-checks-error-paths#task#2`), so, as with
/// the categories above, the order is stated here and the MEMBERSHIP stays the
/// model's.
pub const MESSAGE_ROLE_ORDER: [&str; 3] = ["user", "system", "assistant"];

/// The check types a `GuardrailChecksConfig` admits, in the order this service
/// reads them.
///
/// The three members of the modelled structure
/// (`smithy_query(query="shape",shape="GuardrailChecksConfig")`), by their wire
/// names: a request configuring none of them is refused
/// ([`EMPTY_CHECK_CONFIG_MESSAGE`]), and the results and usage a served
/// evaluation reports carry exactly the ones it configured.
pub const GUARDRAIL_CHECK_TYPES: [&str; 3] =
    ["contentFilter", "promptAttack", "sensitiveInformation"];

/// The service's message for a `checks` configuring no check at all.
///
/// The modelled structure marks none of its three members required
/// (`smithy_query(query="shape",shape="GuardrailChecksConfig")`), so the boundary
/// validator admits an empty one; there is then nothing for the service to
/// evaluate, and it says so in the service's own words for that refusal.
pub const EMPTY_CHECK_CONFIG_MESSAGE: &str = "At least one check type must be configured";

/// Every MEMBER this service quotes a value set for in an order of its own, as
/// (the member path's leading segments, its trailing segments, the order).
///
/// A row is selected by BOTH the member path and the value set that arrived, and
/// it has to be both: this seam is handed the violation rather than the
/// operation, so the path alone would also match the same member of another
/// operation (`Converse` and `ConverseStream` bind a `messages` list whose role
/// enum is `ConversationRole`), and the set alone would match any enum anywhere
/// in this service holding the same strings. A member this service states no
/// order for keeps the order it arrived in — the model's — because inventing one
/// would change a message this service does not own. The paths are the
/// framework's own member paths, whose collection step is `<n>.member`.
const SERVICE_VALUE_ORDERS: [(&str, &str, &[&str]); 2] = [
    (
        "checks.contentFilter.categories",
        "member.category",
        &CONTENT_FILTER_CATEGORY_ORDER,
    ),
    ("messages", "member.role", &MESSAGE_ROLE_ORDER),
];

/// The order this service quotes the value set of the member at `path` in, when
/// `values` is that member's own set, or `None` for anything else.
#[must_use]
pub fn service_value_order(path: &str, values: &[String]) -> Option<Vec<String>> {
    SERVICE_VALUE_ORDERS
        .iter()
        .find(|(prefix, suffix, ordered)| {
            path.starts_with(prefix)
                && path.ends_with(suffix)
                && values.len() == ordered.len()
                && ordered
                    .iter()
                    .all(|value| values.iter().any(|carried| carried == value))
        })
        .map(|(_, _, ordered)| ordered.iter().map(|value| (*value).to_string()).collect())
}

/// The ids `StartAsyncInvoke` serves: the video generation models, which are the
/// models AWS publishes asynchronous invocation for ("Use asynchronous
/// invocation for video generation" in the Amazon Bedrock User Guide).
///
/// The inventory ([`model_catalog`]) marks exactly these
/// [`ModelEntry::supports_async_invoke`], and [`async_model_input_rules`]
/// carries one request schema per id; the crate's tests pin all three lists
/// against each other, so a model cannot be made async-capable without the
/// schema its `modelInput` is then checked against.
pub const ASYNC_INVOKE_MODEL_IDS: [&str; 3] = [
    "amazon.nova-reel-v1:0",
    "amazon.nova-reel-v1:1",
    "luma.ray-v2:0",
];

/// The envelope Bedrock states a `modelInput` schema violation in: the
/// provider's own JSON-Schema complaint, wrapped in the service's wording.
///
/// `detail` is built from the request — the failing member's JSON pointer and
/// the constraint it broke — so the whole message is a function of the request
/// and the model's published schema, never a fixed string.
pub fn malformed_model_input_message(detail: &str) -> String {
    format!("Malformed input request: {detail}, reformat your input and try again.")
}

/// One constraint of a model's published `modelInput` request schema.
///
/// The variants are the kinds of constraint the providers' request tables state:
/// a member that must be there, its type, a published value set, and a numeric
/// span with the step it advances in.
#[derive(Debug, Clone, Copy)]
enum InputConstraint {
    /// The member must be present and not null.
    Required,
    /// A string of `min_length` to `max_length` characters.
    Text {
        min_length: usize,
        max_length: usize,
    },
    /// A boolean.
    Flag,
    /// An array.
    Items,
    /// One of the published string values.
    OneOfText(&'static [&'static str]),
    /// One of the published integer values.
    OneOfNumber(&'static [i64]),
    /// An integer in `min..=max`, in whole steps of `step` (`1` for a span that
    /// publishes no step).
    Span { min: i64, max: i64, step: i64 },
}

/// One rule of a model's published request schema.
#[derive(Debug, Clone, Copy)]
struct InputRule {
    /// The member the rule constrains, as its path from the root of
    /// `modelInput` — which is what lets the refusal name it by the JSON
    /// pointer the provider's own complaint carries.
    path: &'static [&'static str],
    constraint: InputConstraint,
    /// `Some((member, value))` for a rule the request's own discriminator
    /// selects: a Nova Reel `taskType` chooses both the parameter block that
    /// carries the prompt and the duration the generation may run for.
    when: Option<(&'static str, &'static str)>,
}

/// A rule that applies to every request for the model.
const fn rule(path: &'static [&'static str], constraint: InputConstraint) -> InputRule {
    InputRule {
        path,
        constraint,
        when: None,
    }
}

/// A rule stating that the member at `path` must be there.
const fn required(path: &'static [&'static str]) -> InputRule {
    rule(path, InputConstraint::Required)
}

/// A rule that applies only to a request whose `discriminator` member carries
/// `value`.
const fn when(
    discriminator: &'static str,
    value: &'static str,
    path: &'static [&'static str],
    constraint: InputConstraint,
) -> InputRule {
    InputRule {
        path,
        constraint,
        when: Some((discriminator, value)),
    }
}

/// The prompt a video generation model takes: 1 to 4000 characters.
const PROMPT_TEXT: InputConstraint = InputConstraint::Text {
    min_length: 1,
    max_length: 4000,
};
/// `seed`: the signed 32-bit range less one.
const SEED: InputConstraint = InputConstraint::Span {
    min: 0,
    max: 2_147_483_646,
    step: 1,
};
/// `fps` and `dimension`: one published value each.
const FPS: InputConstraint = InputConstraint::OneOfNumber(&[24]);
const DIMENSION: InputConstraint = InputConstraint::OneOfText(&["1280x720"]);

/// Amazon Nova Reel 1.0: a single-shot 6-second generation.
static NOVA_REEL_V1_0_RULES: &[InputRule] = &[
    required(&["taskType"]),
    rule(&["taskType"], InputConstraint::OneOfText(&["TEXT_VIDEO"])),
    required(&["textToVideoParams"]),
    required(&["textToVideoParams", "text"]),
    rule(&["textToVideoParams", "text"], PROMPT_TEXT),
    rule(&["textToVideoParams", "images"], InputConstraint::Items),
    rule(
        &["videoGenerationConfig", "durationSeconds"],
        InputConstraint::OneOfNumber(&[6]),
    ),
    rule(&["videoGenerationConfig", "fps"], FPS),
    rule(&["videoGenerationConfig", "dimension"], DIMENSION),
    rule(&["videoGenerationConfig", "seed"], SEED),
];

/// Amazon Nova Reel 1.1, which adds the multi-shot task types: an automated
/// multi-shot generation runs 12 to 120 seconds in whole 6-second shots.
static NOVA_REEL_V1_1_RULES: &[InputRule] = &[
    required(&["taskType"]),
    rule(
        &["taskType"],
        InputConstraint::OneOfText(&["TEXT_VIDEO", "MULTI_SHOT_AUTOMATED", "MULTI_SHOT_MANUAL"]),
    ),
    when(
        "taskType",
        "TEXT_VIDEO",
        &["textToVideoParams"],
        InputConstraint::Required,
    ),
    required(&["textToVideoParams", "text"]),
    rule(&["textToVideoParams", "text"], PROMPT_TEXT),
    rule(&["textToVideoParams", "images"], InputConstraint::Items),
    when(
        "taskType",
        "MULTI_SHOT_AUTOMATED",
        &["multiShotAutomatedParams"],
        InputConstraint::Required,
    ),
    required(&["multiShotAutomatedParams", "text"]),
    rule(&["multiShotAutomatedParams", "text"], PROMPT_TEXT),
    when(
        "taskType",
        "TEXT_VIDEO",
        &["videoGenerationConfig", "durationSeconds"],
        InputConstraint::OneOfNumber(&[6]),
    ),
    when(
        "taskType",
        "MULTI_SHOT_AUTOMATED",
        &["videoGenerationConfig", "durationSeconds"],
        InputConstraint::Span {
            min: 12,
            max: 120,
            step: 6,
        },
    ),
    rule(
        &["videoGenerationConfig", "durationSeconds"],
        InputConstraint::Span {
            min: 6,
            max: 120,
            step: 6,
        },
    ),
    rule(&["videoGenerationConfig", "fps"], FPS),
    rule(&["videoGenerationConfig", "dimension"], DIMENSION),
    rule(&["videoGenerationConfig", "seed"], SEED),
];

/// Luma AI Ray v2, whose request is the prompt plus four published value sets.
static LUMA_RAY_V2_RULES: &[InputRule] = &[
    required(&["prompt"]),
    // The provider publishes no ceiling on the prompt's length.
    rule(
        &["prompt"],
        InputConstraint::Text {
            min_length: 1,
            max_length: usize::MAX,
        },
    ),
    rule(
        &["aspect_ratio"],
        InputConstraint::OneOfText(&["1:1", "3:4", "4:3", "9:16", "16:9", "9:21", "21:9"]),
    ),
    rule(&["loop"], InputConstraint::Flag),
    rule(&["duration"], InputConstraint::OneOfText(&["5s", "9s"])),
    rule(&["resolution"], InputConstraint::OneOfText(&["540p", "720p"])),
];

/// The published request schema of the model `model_id`'s `modelInput`, or
/// `None` when this service serves no asynchronous invocation for that model.
///
/// Sources, both in the Amazon Bedrock User Guide: "Amazon Nova Reel" under
/// "Video generation models" for the two Nova Reel ids (`taskType`, the
/// `textToVideoParams`/`multiShot*` parameter blocks, and the
/// `videoGenerationConfig` whose `durationSeconds`, `fps` and `dimension` each
/// publish a single accepted value for a 6-second generation, with `seed`
/// bounded by the signed 32-bit range less one), and "Luma AI Ray v2" for
/// `luma.ray-v2` (`prompt` required, with `aspect_ratio`, `loop`, `duration`
/// and `resolution` each published as a value set).
///
/// Unlisted members are NOT refused: these rules state what the provider
/// publishes, and a member this transcription does not carry is no evidence
/// that the provider rejects it.
fn async_model_input_rules(model_id: &str) -> Option<&'static [InputRule]> {
    match model_id {
        "amazon.nova-reel-v1:0" => Some(NOVA_REEL_V1_0_RULES),
        "amazon.nova-reel-v1:1" => Some(NOVA_REEL_V1_1_RULES),
        "luma.ray-v2:0" => Some(LUMA_RAY_V2_RULES),
        _ => None,
    }
}

/// The JSON pointer of the member at `path`, as the provider's complaint names
/// it: `#` for the root of `modelInput`, `#/a/b` for a nested member.
fn input_pointer(path: &[&str]) -> String {
    if path.is_empty() {
        return "#".to_string();
    }
    format!("#/{}", path.join("/"))
}

/// A submitted value as this wording shows it: a JSON string reads bare (`… 7s
/// is not a valid enum value`), every other value as JSON.
fn shown_input_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// How a `modelInput` broke the invoked model's published request schema, as the
/// detail of a [`malformed_model_input_message`], or `None` when it satisfies it
/// (and when the model publishes no schema here).
///
/// Several violations are reported as ONE: the first rule of the model's schema
/// the request breaks, in the order the schema states them, so the refusal a
/// request draws is fixed by the schema and never by map iteration order.
pub fn async_model_input_violation(
    model_id: &str,
    model_input: &serde_json::Value,
) -> Option<String> {
    let rules = async_model_input_rules(model_id)?;
    if !model_input.is_object() {
        return Some(format!(
            "#: {} is not of the expected type",
            shown_input_value(model_input)
        ));
    }
    rules
        .iter()
        .find_map(|rule| input_rule_violation(rule, model_input))
}

/// How `root` breaks `rule`, as the provider states it — the failing member's
/// pointer, then the constraint it broke and the value that broke it — or `None`
/// when the rule is satisfied or does not apply to this request.
///
/// A member the request does not carry is answered by its own `Required` rule
/// and by nothing else, so an absent optional member and an absent parent are
/// both silent here rather than reported twice.
fn input_rule_violation(rule: &InputRule, root: &serde_json::Value) -> Option<String> {
    if let Some((discriminator, value)) = rule.when {
        if root.get(discriminator).and_then(serde_json::Value::as_str) != Some(value) {
            return None;
        }
    }
    let (name, parent_path) = rule.path.split_last()?;
    let mut parent = root;
    for (depth, segment) in parent_path.iter().enumerate() {
        let next = parent.get(*segment).filter(|value| !value.is_null())?;
        if !next.is_object() {
            return Some(format!(
                "{}: {} is not of the expected type",
                input_pointer(&parent_path[..=depth]),
                shown_input_value(next)
            ));
        }
        parent = next;
    }
    let pointer = input_pointer(rule.path);
    let Some(member) = parent.get(*name).filter(|value| !value.is_null()) else {
        return matches!(rule.constraint, InputConstraint::Required).then(|| {
            format!(
                "{}: required key [{name}] not found",
                input_pointer(parent_path)
            )
        });
    };
    let shown = shown_input_value(member);
    let wrong_type = || Some(format!("{pointer}: {shown} is not of the expected type"));
    let not_in_set = || Some(format!("{pointer}: {shown} is not a valid enum value"));
    match rule.constraint {
        InputConstraint::Required => None,
        InputConstraint::Text {
            min_length,
            max_length,
        } => {
            let Some(text) = member.as_str() else {
                return wrong_type();
            };
            let length = text.chars().count();
            if length < min_length {
                return Some(format!(
                    "{pointer}: expected minLength: {min_length}, actual: {length}"
                ));
            }
            if length > max_length {
                return Some(format!(
                    "{pointer}: expected maxLength: {max_length}, actual: {length}"
                ));
            }
            None
        }
        InputConstraint::Flag if member.is_boolean() => None,
        InputConstraint::Flag => wrong_type(),
        InputConstraint::Items if member.is_array() => None,
        InputConstraint::Items => wrong_type(),
        InputConstraint::OneOfText(values) => {
            if member.as_str().is_some_and(|text| values.contains(&text)) {
                None
            } else {
                not_in_set()
            }
        }
        InputConstraint::OneOfNumber(values) => {
            if member.as_i64().is_some_and(|number| values.contains(&number)) {
                None
            } else {
                not_in_set()
            }
        }
        InputConstraint::Span { min, max, step } => {
            let Some(number) = member.as_i64() else {
                return wrong_type();
            };
            if number < min {
                return Some(format!("{pointer}: {shown} is not greater or equal to {min}"));
            }
            if number > max {
                return Some(format!("{pointer}: {shown} is not less or equal to {max}"));
            }
            if step > 1 && number % step != 0 {
                return Some(format!("{pointer}: {shown} is not a multiple of {step}"));
            }
            None
        }
    }
}

/// One async invocation, exactly as `StartAsyncInvoke` stored it.
#[derive(Debug, Clone)]
pub struct AsyncInvokeRecord {
    pub invocation_arn: String,
    pub model_id: String,
    pub model_arn: String,
    pub client_request_token: Option<String>,
    pub status: AsyncInvokeStatus,
    /// Epoch milliseconds of the submitting call.
    pub submit_time: i64,
    pub last_modified_time: Option<i64>,
    pub end_time: Option<i64>,
    pub failure_message: Option<String>,
    pub output_data_config: AsyncInvokeOutputDataConfig,
    pub model_input: ::aws_smithy_types::Document,
    pub tags: Vec<Tag>,
    /// Epoch milliseconds at which this record leaves `InProgress`.
    pub settles_at: i64,
    /// Digest of the submitted payload, compared when a `clientRequestToken` is
    /// reused. A fixed-size digest rather than the canonical payload itself, so
    /// the record keeps no second copy of the caller's data.
    pub request_fingerprint: String,
}

/// The fingerprint of a `StartAsyncInvoke` payload: a SHA-256 digest of its
/// canonical JSON rendering, used to tell an idempotent retry from a conflicting
/// reuse of the same token.
pub fn async_invoke_fingerprint(
    model_id: &str,
    model_input: &::aws_smithy_types::Document,
    output_data_config: &AsyncInvokeOutputDataConfig,
    tags: &[Tag],
) -> String {
    use sha2::{Digest, Sha256};
    let output = match output_data_config {
        AsyncInvokeOutputDataConfig::s3OutputDataConfig(config) => serde_json::json!({
            "s3Uri": config.s3_uri,
            "kmsKeyId": config.kms_key_id,
            "bucketOwner": config.bucket_owner,
        }),
    };
    let mut tag_pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    tag_pairs.sort_unstable();
    // `serde_json::Map` is ordered, so this rendering is canonical.
    let canonical = serde_json::json!({
        "modelId": model_id,
        "modelInput": document_to_json(model_input),
        "outputDataConfig": output,
        "tags": tag_pairs,
    })
    .to_string();
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// A page token for `ListAsyncInvokes`: the sort key of the last item of the
/// page just returned, hex-encoded so it is opaque to the caller and matches the
/// modelled `^\S*$` token pattern.
pub fn encode_page_token(submit_time: i64, invocation_arn: &str) -> String {
    hex::encode(format!("{submit_time}|{invocation_arn}"))
}

/// The sentence a paginated read refuses a `nextToken` it cannot place with.
///
/// The whole message is FIXED text for this refusal: it states that the token
/// handed over is not one the service can resume from and nothing else. In
/// particular it carries NO value from the request — the rejected token is not
/// quoted back — so the text a client matches or logs on is the same whatever
/// bytes were submitted.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token was passed";

/// The sort key a page token carries, or `None` when the token is not one this
/// emulator minted.
pub fn decode_page_token(token: &str) -> Option<(i64, String)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (submit_time, arn) = decoded.split_once('|')?;
    Some((submit_time.parse().ok()?, arn.to_string()))
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Now, in epoch SECONDS: the unit a response envelope that states the instant it was
/// created in reports it in (the OpenAI-compatible chat completions envelope's `created`,
/// [`openai_chat`]). Read off the same clock as [`now_millis`], so the two cannot disagree
/// about when a call was served.
pub fn now_seconds() -> i64 {
    now_millis() / 1_000
}

/// Now, in epoch milliseconds.
///
/// A clock the host reports as before the epoch, or a value too large for the
/// modelled timestamp, is logged rather than passed on silently: either would
/// order wrongly against the records already stored.
pub fn now_millis() -> i64 {
    match ::std::time::SystemTime::now().duration_since(::std::time::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or_else(|_| {
            ::tracing::warn!("the host clock is past the representable range; clamping");
            i64::MAX
        }),
        Err(error) => {
            ::tracing::warn!(%error, "the host clock is before the epoch; using the epoch");
            0
        }
    }
}

/// Epoch milliseconds as the modelled timestamp type.
pub fn datetime_of_millis(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A modelled timestamp as epoch milliseconds, so state comparisons are integer
/// comparisons.
pub fn millis_of_datetime(value: &::aws_smithy_types::DateTime) -> i64 {
    value.to_millis().unwrap_or_else(|error| {
        ::tracing::warn!(%error, "a request timestamp is outside the representable range");
        0
    })
}

// ---------------------------------------------------------------------------
// The tokenizer
// ---------------------------------------------------------------------------

/// The longest run of characters this emulator's tokenizer puts in one token.
///
/// The tokenizer is the emulator's own, and deliberately simple: a word is cut
/// into pieces of at most this many characters and every punctuation mark is a
/// token of its own. It is NOT any vendor's BPE vocabulary — no tokenizer
/// vocabulary ships with this emulator — but it is deterministic and it is the
/// SINGLE tokenizer every operation counts with, which is the property
/// `CountTokens` and `Converse` have to share.
pub const MAX_CHARS_PER_TOKEN: usize = 4;

/// The byte offset at which each token of `text` ends.
fn token_ends(text: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut chars_in_piece = 0usize;
    for (offset, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            if chars_in_piece == MAX_CHARS_PER_TOKEN {
                ends.push(offset);
                chars_in_piece = 0;
            }
            chars_in_piece += 1;
        } else {
            if chars_in_piece > 0 {
                ends.push(offset);
                chars_in_piece = 0;
            }
            if !ch.is_whitespace() {
                ends.push(offset + ch.len_utf8());
            }
        }
    }
    if chars_in_piece > 0 {
        ends.push(text.len());
    }
    ends
}

/// How many tokens `text` is.
pub fn count_text_tokens(text: &str) -> u32 {
    u32::try_from(token_ends(text).len()).unwrap_or(u32::MAX)
}

/// A token count as the modelled `Integer`.
pub fn token_count_i32(tokens: u32) -> i32 {
    i32::try_from(tokens).unwrap_or(i32::MAX)
}

/// `text` cut to at most `max_tokens` tokens, on a token boundary.
pub fn truncate_to_tokens(text: &str, max_tokens: u32) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let ends = token_ends(text);
    match ends.get(max_tokens as usize - 1) {
        Some(_) if ends.len() > max_tokens as usize => text[..ends[max_tokens as usize - 1]]
            .trim_end()
            .to_string(),
        _ => text.to_string(),
    }
}

/// Every piece of text a `Converse`-shaped input sends to the model, in the
/// order the service assembles the prompt in ([`converse_prompt_segments`]).
///
/// The count an inference call reports covers the WHOLE prompt the service hands
/// the model, not only the plain-text blocks of the conversation: `CountTokens`
/// documents its count as the tokens of "the input" for the request as a whole,
/// a `Converse` input is its `system` blocks, its `messages` and its
/// `toolConfig`, and every tool offered reaches the model verbatim — its name,
/// its description and its JSON input schema — so each of those is prompt text
/// too. The same holds inside a message: a document's text, a tool use's input,
/// a tool result's text or JSON, guarded content, reasoning text, a citation's
/// text and a search result's content are all text the model is given.
///
/// What carries no text contributes nothing: a media payload (`image`, `video`,
/// `audio`, or a document carried as bytes or an S3 location) has no published
/// token cost per byte, so this emulator counts no tokens it cannot derive
/// rather than inventing a rate, and a marker that is not prompt text at all
/// (`cachePoint`, a tool choice of `auto` or `any`) adds nothing.
pub fn converse_prompt_pieces(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> Vec<String> {
    converse_prompt_segments(system, messages, tool_config)
        .into_iter()
        .filter_map(|segment| match segment {
            PromptSegment::Text(text) => Some(text),
            PromptSegment::CachePoint(_) => None,
        })
        .collect()
}

/// One ordered piece of the prompt a `Converse`-shaped input assembles: a piece
/// of prompt text, or a `cachePoint` marker.
#[derive(Debug, Clone)]
pub enum PromptSegment {
    /// One piece of prompt text, as [`converse_prompt_pieces`] describes them.
    Text(String),
    /// A cache checkpoint, carrying the `CacheTTL` its marker asked for.
    CachePoint(Option<String>),
}

/// The whole prompt a `Converse`-shaped input is, piece by piece and marker by
/// marker, in the order the service assembles it: the tool configuration, then
/// the system prompt, then the conversation.
///
/// The ORDER is what a `cachePoint` divides: the marker ends a prefix, and
/// everything before it in the assembled prompt is the cacheable section it
/// delimits, whichever of the three places the marker sits in (`toolConfig`'s
/// tool list, `system`, or a message's content). [`converse_prompt_pieces`]
/// drops the markers and keeps the text, so one rule describes the prompt for
/// both the token count and the prompt cache.
pub fn converse_prompt_segments(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> Vec<PromptSegment> {
    // The text collected since the last marker, flushed into the segment list
    // when a marker arrives, so every piece keeps its place in prompt order.
    fn checkpoint(
        point: &CachePointBlock,
        pieces: &mut Vec<String>,
        segments: &mut Vec<PromptSegment>,
    ) {
        segments.extend(pieces.drain(..).map(PromptSegment::Text));
        segments.push(PromptSegment::CachePoint(point.ttl.clone()));
    }

    let mut segments: Vec<PromptSegment> = Vec::new();
    let mut pieces: Vec<String> = Vec::new();
    if let Some(config) = tool_config {
        for tool in &config.tools {
            match tool {
                Tool::toolSpec(spec) => {
                    pieces.push(spec.name.clone());
                    if let Some(description) = &spec.description {
                        pieces.push(description.clone());
                    }
                    match &spec.input_schema {
                        ToolInputSchema::Json(schema) => {
                            pieces.push(document_to_json(schema).to_string())
                        }
                    }
                }
                Tool::systemTool(tool) => pieces.push(tool.name.clone()),
                Tool::cachePoint(point) => checkpoint(point, &mut pieces, &mut segments),
            }
        }
        match &config.tool_choice {
            Some(ToolChoice::Tool(choice)) => pieces.push(choice.name.clone()),
            Some(ToolChoice::Auto(_) | ToolChoice::Any(_)) | None => {}
        }
    }
    for block in system.map(Vec::as_slice).unwrap_or_default() {
        match block {
            SystemContentBlock::Text(text) => pieces.push(text.clone()),
            SystemContentBlock::guardContent(guard) => push_guard_text(guard, &mut pieces),
            SystemContentBlock::cachePoint(point) => {
                checkpoint(point, &mut pieces, &mut segments)
            }
        }
    }
    for message in messages.map(Vec::as_slice).unwrap_or_default() {
        for block in &message.content {
            match block {
                ContentBlock::cachePoint(point) => {
                    checkpoint(point, &mut pieces, &mut segments)
                }
                block => push_content_text(block, &mut pieces),
            }
        }
    }
    segments.extend(pieces.into_iter().map(PromptSegment::Text));
    segments
}

/// One cacheable prefix a request's `cachePoint` marker delimits.
#[derive(Debug, Clone)]
pub struct CacheCheckpoint {
    /// The digest of the prompt text before the marker, in prompt order. The
    /// cache is keyed on it, so a prefix that diverges anywhere — its first
    /// token included — is a different prefix and cannot hit, and no prompt
    /// text has to be held to decide that.
    pub digest: String,
    /// The tokens that prefix is.
    pub tokens: u32,
    /// The modelled `CacheTTL` the entry is written under, as [`cache_ttl`]
    /// reads it off the marker.
    pub ttl: String,
}

/// What the `cachePoint` markers of one request ask the prompt cache to do.
#[derive(Debug, Clone, Default)]
pub struct PromptCachePlan {
    /// The cacheable prefixes the request delimits, in increasing length, at
    /// most [`MAX_CACHE_CHECKPOINTS`] of them. A marker delimiting no text at
    /// all contributes none.
    pub checkpoints: Vec<CacheCheckpoint>,
    /// The tokens of the prompt text AFTER the last marker — the input tokens a
    /// request that marks a cacheable prefix is charged for, the prefix itself
    /// being accounted for as a cache read or a cache write.
    pub tail_tokens: u32,
}

/// The cacheable prefixes one `Converse`-shaped input delimits, and the tokens
/// left after the last of them ([`PromptCachePlan`]).
pub fn prompt_cache_plan(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> PromptCachePlan {
    use sha2::{Digest, Sha256};

    let mut checkpoints: Vec<CacheCheckpoint> = Vec::new();
    // The prefix is digested as the prompt is walked, each piece separated from
    // the next, so a checkpoint's digest costs one clone of the hash state
    // rather than a copy of the prompt however many markers a request carries.
    let mut prefix = Sha256::new();
    let mut prefix_tokens = 0u32;
    let mut tail_tokens = 0u32;
    for segment in converse_prompt_segments(system, messages, tool_config) {
        match segment {
            PromptSegment::Text(text) => {
                let tokens = count_text_tokens(&text);
                prefix_tokens = prefix_tokens.saturating_add(tokens);
                tail_tokens = tail_tokens.saturating_add(tokens);
                prefix.update(text.as_bytes());
                prefix.update([0u8]);
            }
            PromptSegment::CachePoint(ttl) => {
                // A marker with no prompt text before it delimits nothing, so
                // there is no prefix to cache at it.
                if prefix_tokens > 0 {
                    checkpoints.push(CacheCheckpoint {
                        digest: hex::encode(prefix.clone().finalize()),
                        tokens: prefix_tokens,
                        ttl: cache_ttl(ttl.as_deref()),
                    });
                    // Only the longest prefixes a request marks are honoured,
                    // so neither this plan nor the work it drives grows with
                    // the number of markers the request carries.
                    if checkpoints.len() > MAX_CACHE_CHECKPOINTS {
                        checkpoints.remove(0);
                    }
                }
                tail_tokens = 0;
            }
        }
    }
    PromptCachePlan {
        checkpoints,
        tail_tokens,
    }
}

/// One prompt prefix this emulator holds in its prompt cache.
#[derive(Debug, Clone)]
pub struct PromptCacheEntry {
    /// The tokens of the cached prefix: what a request that matches it reports
    /// as read from the cache.
    pub tokens: u32,
    /// The modelled `CacheTTL` it was written under.
    pub ttl: String,
    /// When the entry leaves the cache unless it is read again before then.
    pub expires_at: i64,
}

/// The key a cached prefix is held under: the model it was prompted to, plus the
/// digest of the prefix text ([`CacheCheckpoint::digest`]).
///
/// A cached prefix belongs to the model it was prompted to, and keying on the
/// digest is what makes only a byte-identical prefix hit while keeping the
/// prompt text itself out of the store.
pub fn prompt_cache_key(model_id: &str, prefix_digest: &str) -> String {
    format!("{model_id}:{prefix_digest}")
}

/// The modelled `CacheTTL` a marker is honoured under: the one it named, or
/// [`DEFAULT_CACHE_TTL`] when it named none.
///
/// A value the enum does not offer is held — and reported — under the default,
/// so the time to live a response carries is always one the entry was actually
/// written for and one the model declares.
pub fn cache_ttl(ttl: Option<&str>) -> String {
    match ttl {
        Some("1h") => "1h".to_string(),
        _ => DEFAULT_CACHE_TTL.to_string(),
    }
}

/// A modelled `CacheTTL` as milliseconds.
///
/// The enum offers one hour and five minutes; [`cache_ttl`] is what makes every
/// stored and reported value one of the two.
pub fn cache_ttl_millis(ttl: &str) -> i64 {
    match ttl {
        "1h" => 60 * 60 * 1000,
        _ => 5 * 60 * 1000,
    }
}

/// The prompt-cache accounting of one served Converse-API turn.
#[derive(Debug, Clone, Default)]
pub struct PromptCacheOutcome {
    /// The prefix tokens this call read from the cache.
    pub read_tokens: u32,
    /// The prefix tokens this call wrote to the cache.
    pub write_tokens: u32,
    /// What it wrote, broken down by time to live, longest first.
    pub cache_details: Vec<CacheDetail>,
    /// The input tokens left to charge: the prompt text after the last marker.
    pub input_tokens: u32,
}

/// The whole prompt-cache stage of one Converse-API turn, as both Converse
/// operations run it: digest the prompt the request assembles, then match its
/// marked prefixes against this service's cache.
///
/// The digesting happens BEFORE the state guard is taken, so the only work
/// under the lock every other request of this service waits on is the bounded
/// match-and-write step ([`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::apply_prompt_cache`]).
/// `Converse` and `ConverseStream` both call this, so the two cannot drift.
pub fn serve_prompt_cache(
    state: &::engine::ServiceState<ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState>,
    model_id: &str,
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> Option<PromptCacheOutcome> {
    let plan = prompt_cache_plan(system, messages, tool_config);
    state
        .lock()
        .apply_prompt_cache(model_id, &plan, now_millis())
}

/// The text a guarded content block carries. A guarded image carries none.
fn push_guard_text(block: &GuardrailConverseContentBlock, pieces: &mut Vec<String>) {
    match block {
        GuardrailConverseContentBlock::Text(text) => pieces.push(text.text.clone()),
        GuardrailConverseContentBlock::Image(_) => {}
    }
}

/// The text a document block carries: its name and context, plus its source
/// when the source IS text. Bytes and an S3 location are not text.
fn push_document_text(block: &DocumentBlock, pieces: &mut Vec<String>) {
    pieces.push(block.name.clone());
    if let Some(context) = &block.context {
        pieces.push(context.clone());
    }
    match &block.source {
        DocumentSource::Text(text) => pieces.push(text.clone()),
        DocumentSource::Content(blocks) => {
            for block in blocks {
                match block {
                    DocumentContentBlock::Text(text) => pieces.push(text.clone()),
                }
            }
        }
        DocumentSource::Bytes(_) | DocumentSource::s3Location(_) => {}
    }
}

/// The text a search result carries: where it came from, its title and its
/// content.
fn push_search_result_text(block: &SearchResultBlock, pieces: &mut Vec<String>) {
    pieces.push(block.source.clone());
    pieces.push(block.title.clone());
    for content in &block.content {
        pieces.push(content.text.clone());
    }
}

/// The text a reference to a tool carries: whatever of it the caller named.
fn push_tool_reference_text(reference: &ToolReference, pieces: &mut Vec<String>) {
    for value in [
        reference.r#type.as_ref(),
        reference.name.as_ref(),
        reference.server_name.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        pieces.push(value.clone());
    }
}

/// The text one message content block sends the model.
fn push_content_text(block: &ContentBlock, pieces: &mut Vec<String>) {
    match block {
        ContentBlock::Text(text) => pieces.push(text.clone()),
        // A media payload carries no text; see `converse_prompt_pieces`.
        ContentBlock::Image(_) | ContentBlock::Video(_) | ContentBlock::Audio(_) => {}
        ContentBlock::Document(document) => push_document_text(document, pieces),
        ContentBlock::toolUse(tool_use) => {
            pieces.push(tool_use.name.clone());
            pieces.push(document_to_json(&tool_use.input).to_string());
        }
        ContentBlock::toolResult(result) => {
            for content in &result.content {
                match content {
                    ToolResultContentBlock::Text(text) => pieces.push(text.clone()),
                    ToolResultContentBlock::Json(json) => {
                        pieces.push(document_to_json(json).to_string())
                    }
                    ToolResultContentBlock::Document(document) => {
                        push_document_text(document, pieces)
                    }
                    ToolResultContentBlock::searchResult(result) => {
                        push_search_result_text(result, pieces)
                    }
                    ToolResultContentBlock::Image(_) | ToolResultContentBlock::Video(_) => {}
                }
            }
        }
        ContentBlock::guardContent(guard) => push_guard_text(guard, pieces),
        ContentBlock::cachePoint(_) => {}
        ContentBlock::reasoningContent(reasoning) => match reasoning {
            ReasoningContentBlock::reasoningText(text) => pieces.push(text.text.clone()),
            ReasoningContentBlock::redactedContent(_) => {}
        },
        ContentBlock::citationsContent(citations) => {
            for generated in citations.content.iter().flatten() {
                match generated {
                    CitationGeneratedContent::Text(text) => pieces.push(text.clone()),
                }
            }
            for citation in citations.citations.iter().flatten() {
                if let Some(title) = &citation.title {
                    pieces.push(title.clone());
                }
                for source in citation.source_content.iter().flatten() {
                    match source {
                        CitationSourceContent::Text(text) => pieces.push(text.clone()),
                    }
                }
            }
        }
        ContentBlock::searchResult(result) => push_search_result_text(result, pieces),
        ContentBlock::toolAddition(addition) => {
            push_tool_reference_text(&addition.tool, pieces)
        }
        ContentBlock::toolRemoval(removal) => push_tool_reference_text(&removal.tool, pieces),
    }
}

/// The tokens a `Converse`-shaped input is: the tokens of every piece of prompt
/// text it carries ([`converse_prompt_pieces`]). `CountTokens` and `Converse`
/// both call this, so the two agree for identical input.
pub fn converse_input_tokens(
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
) -> u32 {
    converse_prompt_pieces(system, messages, tool_config)
        .iter()
        .fold(0u32, |total, piece| {
            total.saturating_add(count_text_tokens(piece))
        })
}

/// The names of the tools a `toolConfig` offers the model, in request order.
///
/// A tool is requestable when it HAS a name: `toolSpec` and `systemTool` name
/// one, while `cachePoint` is a marker in the tool list and names none.
pub fn requestable_tool_names(config: &ToolConfiguration) -> Vec<&str> {
    config
        .tools
        .iter()
        .filter_map(|tool| match tool {
            Tool::toolSpec(spec) => Some(spec.name.as_str()),
            Tool::systemTool(tool) => Some(tool.name.as_str()),
            Tool::cachePoint(_) => None,
        })
        .collect()
}

/// The tool-result addressing rule both Converse APIs state: a `toolResult`
/// answers a tool request the conversation carries, addressed by the
/// `toolUseId` that request was issued under, so a `toolUseId` no `toolUse`
/// block of the conversation was issued with addresses nothing.
///
/// `Err` carries the refusal sentence, which names the member path of the
/// offending block, the id it supplied and the id (or ids) the tool requests it
/// could answer were issued under; the caller answers it with the operation's
/// modelled `ValidationException` (400). `Converse` and `ConverseStream` share
/// this, so the same conversation is accepted or refused identically by both.
pub fn validate_tool_results(messages: &[Message]) -> Result<(), String> {
    // The ids issued so far, in the order the conversation issued them, plus
    // those of the most recent tool request, which is the one a tool result is
    // normally answering.
    let mut issued: HashSet<&str> = HashSet::new();
    let mut latest: Vec<&str> = Vec::new();
    for (turn, message) in messages.iter().enumerate() {
        let mut issued_here: Vec<&str> = Vec::new();
        for (index, block) in message.content.iter().enumerate() {
            match block {
                ContentBlock::toolUse(tool_use) => {
                    issued.insert(tool_use.tool_use_id.as_str());
                    issued_here.push(tool_use.tool_use_id.as_str());
                }
                ContentBlock::toolResult(result) => {
                    if issued.contains(result.tool_use_id.as_str()) {
                        continue;
                    }
                    let addressed = if latest.is_empty() {
                        "the conversation carries no tool request for it to answer".to_string()
                    } else {
                        format!(
                            "the tool request it can answer was issued with toolUseId {}",
                            latest.join(", ")
                        )
                    };
                    return Err(format!(
                        "The toolResult block at messages.{turn}.content.{index} has \
                         toolUseId {}, but {addressed}. Try again with a toolResult whose \
                         toolUseId is the one of the toolUse block it answers.",
                        result.tool_use_id
                    ));
                }
                _ => {}
            }
        }
        if !issued_here.is_empty() {
            latest = issued_here;
        }
    }
    Ok(())
}

/// The tokens ONE content block is, by the same text rule
/// [`converse_prompt_pieces`] applies to a whole request — so a block costs the
/// same whether this emulator generates it or a later turn submits it back.
pub fn content_block_tokens(block: &ContentBlock) -> u32 {
    let mut pieces = Vec::new();
    push_content_text(block, &mut pieces);
    pieces.iter().fold(0u32, |total, piece| {
        total.saturating_add(count_text_tokens(piece))
    })
}

/// The opening-turn rule both Converse APIs state: a conversation is answered
/// only when its FIRST turn is a `user` turn.
///
/// A turn is the model's answer to what a user asked, so the conversation the
/// API takes has to open on the user: a request carrying no turn at all, or one
/// whose leading `Message.role` is any other `ConversationRole` value
/// (`assistant`, `system`), has nothing for the model to answer and is refused.
/// What comes AFTER the opening turn is not constrained here: `Messages` is a
/// plain list, so a conversation that repeats a role later on — two user turns
/// appended before the assistant has answered, a pending turn re-appended by a
/// retry — is served like any other.
///
/// `Err` carries the refusal sentence, which the caller answers with the
/// operation's own modelled `ValidationException` (400). `Converse` and
/// `ConverseStream` share this, so the same conversation is accepted or refused
/// identically by both.
///
/// The second rule it states is each turn's own: a `Message` whose `content`
/// list is empty carries nothing for the model to read, and is refused naming
/// that message's index ([`empty_message_content_message`]).
pub fn validate_conversation(messages: Option<&Vec<Message>>) -> Result<&Vec<Message>, String> {
    let messages = match messages {
        Some(messages)
            if messages
                .first()
                .is_some_and(|first| first.role == CONVERSATION_ROLE_USER) =>
        {
            messages
        }
        _ => return Err(NO_OPENING_USER_TURN.to_string()),
    };
    // A turn is its content: `Message.content` is `@required`, so a request that
    // binds it to an EMPTY list carries no content block for that turn and there
    // is nothing for the model to read in it. The refusal names the offending
    // message by its own position in the submitted list, so a conversation
    // assembled in a loop says which iteration dropped its content.
    if let Some(index) = messages
        .iter()
        .position(|message| message.content.is_empty())
    {
        return Err(empty_message_content_message(index));
    }
    Ok(messages)
}

/// What both Converse surfaces answer a `Message` whose `content` list is empty
/// with.
///
/// Fixed text for this refusal class, with ONE variable part: the zero-based
/// index of the offending message in the request's own `messages` list, which is
/// how a caller finds the turn it has to add a `ContentBlock` to.
pub fn empty_message_content_message(index: usize) -> String {
    format!(
        "The content field in the Message object at messages.{index} is empty. Add a \
         ContentBlock object to the content field and try again."
    )
}

/// The `ConversationRole` value a conversation has to open on
/// (smithy_query(query="shape", shape="ConversationRole")).
pub const CONVERSATION_ROLE_USER: &str = "user";

/// What both Converse surfaces answer a conversation that does not open on a
/// `user` turn with.
pub const NO_OPENING_USER_TURN: &str = "A conversation must start with a user message. Try again \
                                        with a conversation that starts with a user message.";

/// One served Converse-API turn: the assistant content this emulator answers
/// with, the token accounting computed for it, and whether the generation cap
/// cut it short.
#[derive(Debug, Clone)]
pub struct ConverseTurn {
    /// The assistant's text. No model runs here, so it is
    /// [`emulated_completion`] — never the request's own text echoed back.
    /// EMPTY for a turn that requests a tool, which generates no text.
    pub text: String,
    /// The tool this turn requests, when the request offered one the turn asks for
    /// ([`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::tool_use_for_turn`]).
    pub tool_use: Option<ToolUseBlock>,
    /// The input tokens this turn is charged for. A turn whose request marks a
    /// cacheable prefix is charged only for the prompt text after the last
    /// marker; the prefix is in [`Self::cache`].
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// The prompt-cache accounting of this turn, when the request marked a
    /// cacheable prefix at all.
    pub cache: Option<PromptCacheOutcome>,
    /// Whether the generation cap cut the text short, which is what makes the
    /// turn stop on `max_tokens` instead of `end_turn`. A turn whose text halted
    /// at a configured stop sequence ended on that sequence rather than at the
    /// cap, so this is `false` for it.
    pub truncated: bool,
    /// Whether a configured stop sequence completed inside the text, which is
    /// what makes the turn stop on `stop_sequence`
    /// ([`EmulatedCompletion::halted_at_stop_sequence`]).
    pub halted_at_stop_sequence: bool,
    /// Whether a guardrail blocked this turn, which is what makes it stop on
    /// `guardrail_intervened` and carry the guardrail's blocked messaging as its
    /// whole content ([`enforced_guardrail::EnforcedGuardrail`]).
    pub guardrail_intervened: bool,
}

impl ConverseTurn {
    /// The modelled `StopReason` this turn ended on.
    pub fn stop_reason(&self) -> &'static str {
        if self.guardrail_intervened {
            enforced_guardrail::STOP_REASON_GUARDRAIL_INTERVENED
        } else if self.tool_use.is_some() {
            "tool_use"
        } else if self.halted_at_stop_sequence {
            // The modelled `StopReason` names the cause of the stop, and a
            // request's `stopSequences` are "a sequence of characters that
            // causes the model to stop generating the response": a turn that
            // ended because one of them was generated reports THAT, not the
            // reason a turn which simply finished reports.
            "stop_sequence"
        } else if self.truncated {
            "max_tokens"
        } else {
            "end_turn"
        }
    }

    /// The assistant message's content, as the modelled `ContentBlock`s: the
    /// tool request alone for a turn that requests a tool, and otherwise the
    /// generated text.
    pub fn content(&self) -> Vec<ContentBlock> {
        match &self.tool_use {
            Some(tool_use) => vec![ContentBlock::toolUse(tool_use.clone())],
            None => vec![ContentBlock::Text(self.text.clone())],
        }
    }

    /// The modelled `TokenUsage` for this turn: the counts actually served, with
    /// `totalTokens` their sum.
    ///
    /// A turn that engaged the prompt cache reports the two cache counters too,
    /// and `totalTokens` closes over all four: the prefix tokens are billed as
    /// a cache write on the call that cached them and as a cache read on the
    /// call that matched them, never a second time as `inputTokens`.
    pub fn usage(&self) -> TokenUsage {
        let cached_tokens = self
            .cache
            .as_ref()
            .map(|cache| cache.read_tokens.saturating_add(cache.write_tokens))
            .unwrap_or_default();
        TokenUsage {
            input_tokens: token_count_i32(self.input_tokens),
            output_tokens: token_count_i32(self.output_tokens),
            total_tokens: token_count_i32(
                self.input_tokens
                    .saturating_add(self.output_tokens)
                    .saturating_add(cached_tokens),
            ),
            cache_read_input_tokens: self
                .cache
                .as_ref()
                .map(|cache| token_count_i32(cache.read_tokens)),
            cache_write_input_tokens: self
                .cache
                .as_ref()
                .map(|cache| token_count_i32(cache.write_tokens)),
            cache_details: self
                .cache
                .as_ref()
                .map(|cache| cache.cache_details.clone()),
        }
    }
}

/// The base inference parameters one Converse-API request configures, as
/// [`converse_turn`] reads them off `InferenceConfiguration`: the generation
/// ceiling and the stop sequences that halt generation.
///
/// `Converse` and `ConverseStream` bind the SAME `InferenceConfiguration`, so
/// both read their parameters through [`Self::of`] and neither can honour one
/// member the other drops. `temperature` and `topP` steer which tokens a model
/// would pick, which no model picks here, so they bound nothing about the text
/// that is served.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnInference<'a> {
    /// `inferenceConfig.maxTokens`: the maximum number of tokens to allow in the
    /// generated response.
    pub max_tokens: Option<i32>,
    /// `inferenceConfig.stopSequences`: the sequences whose first completed
    /// occurrence halts generation.
    pub stop_sequences: &'a [String],
}

impl<'a> TurnInference<'a> {
    /// The parameters a request's own `inferenceConfig` configures; a request
    /// that carries none configures none of them.
    pub fn of(config: Option<&'a InferenceConfiguration>) -> Self {
        Self {
            max_tokens: config.and_then(|config| config.max_tokens),
            stop_sequences: config
                .and_then(|config| config.stop_sequences.as_ref())
                .map(Vec::as_slice)
                .unwrap_or_default(),
        }
    }
}

/// Serve one Converse-API turn from the request that was actually supplied.
///
/// `Converse` and `ConverseStream` both call this, so the content, the stop
/// reason and the token counts the two report for identical input agree by
/// construction; the only difference between them is how the turn is framed on
/// the wire.
///
/// `tool_use` is the tool request this turn makes, from
/// [`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::tool_use_for_turn`]:
/// a turn that requests a tool generates no text, so the whole of what it
/// answers is that one content block and the tokens it reports for its output
/// are that block's.
///
/// `inference` is the request's own `inferenceConfig`, read through
/// [`TurnInference`]: the model's ceiling caps `maxTokens` and the completion is
/// composed to FIT that budget ([`emulated_completion`]) — only a budget it
/// cannot fit in at all cuts the turn short — and the text halts where the first
/// of `stopSequences` completes, so a configured stop sequence is the last thing
/// the turn serves however much budget is left.
///
/// `cache` is the prompt-cache stage's outcome
/// ([`ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::apply_prompt_cache`]):
/// when the request marked a cacheable prefix, the tokens this turn is charged
/// as input are only those after the last marker, the prefix itself having been
/// accounted for as a cache read or a cache write.
pub fn converse_turn(
    model: &ModelEntry,
    system: Option<&Vec<SystemContentBlock>>,
    messages: Option<&Vec<Message>>,
    tool_config: Option<&ToolConfiguration>,
    inference: TurnInference<'_>,
    tool_use: Option<ToolUseBlock>,
    cache: Option<PromptCacheOutcome>,
) -> ConverseTurn {
    let input_tokens = match &cache {
        Some(cache) => cache.input_tokens,
        None => converse_input_tokens(system, messages, tool_config),
    };
    if let Some(tool_use) = tool_use {
        let output_tokens = content_block_tokens(&ContentBlock::toolUse(tool_use.clone()));
        return ConverseTurn {
            text: String::new(),
            tool_use: Some(tool_use),
            input_tokens,
            output_tokens,
            cache,
            truncated: false,
            halted_at_stop_sequence: false,
            guardrail_intervened: false,
        };
    }
    let max_output_tokens = inference
        .max_tokens
        .and_then(|max| u32::try_from(max).ok())
        .unwrap_or(model.max_output_tokens)
        .min(model.max_output_tokens);
    let turns = messages.map(Vec::len).unwrap_or_default();
    let completion = emulated_completion_with(
        &model.model_id,
        u32::try_from(turns).unwrap_or(u32::MAX),
        input_tokens,
        max_output_tokens,
        inference.stop_sequences,
        // A turn handed a tool's result states that result: the exchange the tool
        // request started is only finished by an answer that reports what came back
        // ([`tool_use_turn::tool_result_clauses`]).
        &tool_use_turn::tool_result_clauses(messages),
    );
    let output_tokens = count_text_tokens(&completion.text);
    ConverseTurn {
        text: completion.text,
        tool_use: None,
        input_tokens,
        output_tokens,
        cache,
        truncated: completion.truncated,
        halted_at_stop_sequence: completion.halted_at_stop_sequence,
        guardrail_intervened: false,
    }
}

/// The field names an `InvokeModel` request body carries prompt text under.
///
/// One rule covers every family in the registry, because the per-family request
/// formats in "Inference request parameters and response fields for foundation
/// models" (Amazon Bedrock User Guide) all bind their prompt text to one of
/// these names: Anthropic Claude's `messages[].content[].text` / `system` /
/// legacy `prompt`, Titan Text's `inputText`, Nova's `messages[].content[].text`
/// and `system[].text`, Llama's and Mistral's `prompt`, and Cohere's `prompt` /
/// `message`.
pub const PROMPT_TEXT_FIELDS: [&str; 6] = [
    "prompt",
    "inputText",
    "text",
    "message",
    "system",
    "content",
];

/// The body members whose WHOLE subtree is prompt text.
///
/// A tool definition reaches the model verbatim — the same rule
/// [`converse_prompt_pieces`] states for `toolConfig` — so every string inside
/// one is prompt text, not only the strings bound to a
/// [`PROMPT_TEXT_FIELDS`] name. These are the names the per-family request
/// formats bind tool definitions to: Anthropic Claude's and Mistral's `tools`,
/// and Nova's `toolConfig`.
pub const TOOL_DEFINITION_FIELDS: [&str; 2] = ["tools", "toolConfig"];

/// Every string the body binds under a [`PROMPT_TEXT_FIELDS`] name, plus every
/// string inside a [`TOOL_DEFINITION_FIELDS`] subtree, joined by newlines in key
/// order. The walk stops at [`MAX_DOCUMENT_DEPTH`], so a deeply nested body
/// cannot drive unbounded recursion.
fn prompt_text_of_json(value: &serde_json::Value) -> String {
    fn walk(
        value: &serde_json::Value,
        field: Option<&str>,
        in_tool_definition: bool,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        if depth > MAX_DOCUMENT_DEPTH {
            return;
        }
        match value {
            serde_json::Value::String(text) => {
                if in_tool_definition
                    || field.is_some_and(|name| PROMPT_TEXT_FIELDS.contains(&name))
                {
                    out.push(text.clone());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, field, in_tool_definition, depth + 1, out);
                }
            }
            serde_json::Value::Object(members) => {
                for (name, member) in members {
                    let in_tool_definition = in_tool_definition
                        || TOOL_DEFINITION_FIELDS.contains(&name.as_str());
                    walk(
                        member,
                        Some(name.as_str()),
                        in_tool_definition,
                        depth + 1,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    walk(value, None, false, 0, &mut parts);
    parts.join("\n")
}

/// The field a request body of each family carries its generation cap in.
///
/// Source: the per-family request formats in "Inference request parameters and
/// response fields for foundation models" (Amazon Bedrock User Guide) —
/// Anthropic's `max_tokens`, Titan's `textGenerationConfig.maxTokenCount`,
/// Nova's `inferenceConfig.max_new_tokens`, Llama's `max_gen_len`, and
/// Mistral's and Cohere's `max_tokens`.
pub fn body_max_tokens(family: ModelFamily, body: &serde_json::Value) -> Option<u32> {
    let pointer = match family {
        ModelFamily::AnthropicMessages => "/max_tokens",
        ModelFamily::TitanText => "/textGenerationConfig/maxTokenCount",
        ModelFamily::Nova => "/inferenceConfig/max_new_tokens",
        ModelFamily::Llama => "/max_gen_len",
        ModelFamily::Mistral | ModelFamily::CohereCommand => "/max_tokens",
        ModelFamily::OpenAiChat => openai_chat::MAX_TOKENS_POINTER,
    };
    body.pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

/// The stop sequences a request body of each family configures, in the order the
/// body lists them.
///
/// Source: the per-family request formats in "Inference request parameters and
/// response fields for foundation models" (Amazon Bedrock User Guide) —
/// Anthropic's `stop_sequences`, Titan's `textGenerationConfig.stopSequences`,
/// Nova's `inferenceConfig.stopSequences`, Mistral's `stop` and Cohere's
/// `stop_sequences`. Llama's request format documents no stop-sequence
/// parameter, so a Llama body configures none. A sequence has to be a non-empty
/// string to stop anything, which is also what the Converse API's own
/// `NonEmptyStringList` requires of one.
///
/// The member is a list of sequences, and one sequence on its own is that list
/// with a single entry: a body that binds a bare string configures that one
/// sequence rather than none, since the halt a caller asked for cannot depend on
/// which of the two spellings the body used.
pub fn body_stop_sequences(family: ModelFamily, body: &serde_json::Value) -> Vec<String> {
    let pointer = match family {
        ModelFamily::AnthropicMessages => "/stop_sequences",
        ModelFamily::TitanText => "/textGenerationConfig/stopSequences",
        ModelFamily::Nova => "/inferenceConfig/stopSequences",
        ModelFamily::Llama => return Vec::new(),
        ModelFamily::Mistral => "/stop",
        ModelFamily::CohereCommand => "/stop_sequences",
        ModelFamily::OpenAiChat => openai_chat::STOP_SEQUENCES_POINTER,
    };
    let configured = match body.pointer(pointer) {
        Some(serde_json::Value::Array(sequences)) => sequences.as_slice(),
        Some(sequence @ serde_json::Value::String(_)) => std::slice::from_ref(sequence),
        _ => &[],
    };
    configured
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|sequence| !sequence.is_empty())
        .map(str::to_string)
        .collect()
}

/// One inference parameter of a family's own `InvokeModel` request body whose
/// value the provider bounds.
#[derive(Debug, Clone, Copy)]
pub struct BodyInferenceRange {
    /// The JSON Pointer the family's request format binds the parameter to.
    pub pointer: &'static str,
    /// The name the parameter is called by — the family's own spelling of it,
    /// which is also the name a refusal states.
    pub name: &'static str,
    /// The smallest value the provider documents for it.
    pub min: f64,
    /// The largest value the provider documents for it.
    pub max: f64,
}

/// The bounded inference parameters each family's `InvokeModel` request body
/// carries, with the range the provider documents for each.
///
/// Source: the per-family inference parameters in "Inference request parameters
/// and response fields for foundation models" (Amazon Bedrock User Guide), the
/// same table [`body_max_tokens`] and [`body_stop_sequences`] read their pointers
/// off — Anthropic Claude's `temperature` and `top_p`, Titan Text's
/// `textGenerationConfig.temperature` and `.topP`, Nova's
/// `inferenceConfig.temperature` and `.topP`, Llama's and Mistral's `temperature`
/// and `top_p`, and Cohere Command's `temperature` (which that provider bounds at
/// 5, not at 1) and `p`. The bounds are the PROVIDER's, so a family that admits a
/// wider range is not refused for a value the provider accepts.
pub fn body_inference_ranges(family: ModelFamily) -> &'static [BodyInferenceRange] {
    match family {
        ModelFamily::AnthropicMessages | ModelFamily::Llama | ModelFamily::Mistral => &[
            BodyInferenceRange {
                pointer: "/temperature",
                name: "temperature",
                min: 0.0,
                max: 1.0,
            },
            BodyInferenceRange {
                pointer: "/top_p",
                name: "top_p",
                min: 0.0,
                max: 1.0,
            },
        ],
        ModelFamily::TitanText => &[
            BodyInferenceRange {
                pointer: "/textGenerationConfig/temperature",
                name: "temperature",
                min: 0.0,
                max: 1.0,
            },
            BodyInferenceRange {
                pointer: "/textGenerationConfig/topP",
                name: "topP",
                min: 0.0,
                max: 1.0,
            },
        ],
        ModelFamily::Nova => &[
            BodyInferenceRange {
                pointer: "/inferenceConfig/temperature",
                name: "temperature",
                min: 0.0,
                max: 1.0,
            },
            BodyInferenceRange {
                pointer: "/inferenceConfig/topP",
                name: "topP",
                min: 0.0,
                max: 1.0,
            },
        ],
        ModelFamily::CohereCommand => &[
            BodyInferenceRange {
                pointer: "/temperature",
                name: "temperature",
                min: 0.0,
                max: 5.0,
            },
            BodyInferenceRange {
                pointer: "/p",
                name: "p",
                min: 0.0,
                max: 1.0,
            },
        ],
        // The OpenAI-compatible chat completions API bounds its sampling temperature at 2
        // and its nucleus mass at 1, under the two names it publishes them by.
        ModelFamily::OpenAiChat => &[
            BodyInferenceRange {
                pointer: "/temperature",
                name: "temperature",
                min: 0.0,
                max: 2.0,
            },
            BodyInferenceRange {
                pointer: "/top_p",
                name: "top_p",
                min: 0.0,
                max: 1.0,
            },
        ],
    }
}

/// The sentence the invoke APIs refuse an inference parameter outside its
/// documented range with.
///
/// Fixed text for this refusal class, with the parameter's own name and the two
/// bounds of its documented range interpolated — the bounds come from
/// [`body_inference_ranges`], never from the request — and each bound is written
/// as the decimal a probability-style parameter is documented with.
///
/// The RECORDED wording behind it is the one Bedrock answers a Nova body's
/// out-of-range `temperature` with; the same class of refusal is stated for every
/// other family and parameter by interpolating that parameter's own name and its
/// own bounds, so a later recording of one of them corrects only this template's
/// variable parts.
pub fn out_of_range_parameter_message(bound: &BodyInferenceRange) -> String {
    format!(
        "Malformed input request: the provided {} must be between {:.1} and {:.1}, please \
         reformat your input and try again.",
        bound.name, bound.min, bound.max
    )
}

/// The refusal an `InvokeModel` body earns for an inference parameter outside the
/// range its provider documents, or `None` when every parameter the body carries
/// lies inside it.
///
/// The body is the PROVIDER's schema rather than a modelled shape, so this is
/// where that schema's numeric bounds are applied: a parameter the body does not
/// carry, or carries as something other than a number, bounds nothing here, and
/// the parameters are checked in the order [`body_inference_ranges`] lists them so
/// a body wrong in two of them is refused on the first. `parsed` is the body its
/// caller has already decoded, so no request is parsed twice.
pub fn body_inference_range_violation(
    family: ModelFamily,
    parsed: Option<&serde_json::Value>,
) -> Option<String> {
    let value = parsed?;
    body_inference_ranges(family)
        .iter()
        .filter_map(|bound| {
            let submitted = value.pointer(bound.pointer)?.as_f64()?;
            (submitted < bound.min || submitted > bound.max)
                .then(|| out_of_range_parameter_message(bound))
        })
        .next()
}

/// The only body MIME type the invoke APIs accept: "The MIME type of the input
/// data in the request. You must specify application/json."
pub const JSON_MIME: &str = "application/json";

/// The sentence the service answers a `contentType` the model does not read
/// with. The member's own documentation states the rule ("You must specify
/// application/json"), and this is the wording the recorded corpus shows the
/// service using for the refusal.
pub const INVALID_CONTENT_TYPE_MESSAGE: &str =
    "The provided Content Type is invalid or not supported for this model";

/// The same, for the `accept` member: the MIME type the caller asked the
/// inference body to come back in.
pub const INVALID_ACCEPT_MESSAGE: &str =
    "The provided Accept header is invalid or not supported for this model";

/// The body field that carries a guardrail configuration inline. Naming it
/// without a `guardrailIdentifier` is one of the error situations both invoke
/// operations document.
pub const INLINE_GUARDRAIL_CONFIG_FIELD: &str = "amazon-bedrock-guardrailConfig";

/// A MIME type with its parameters (`; charset=utf-8`) and casing removed, so
/// `application/json; charset=utf-8` compares equal to `application/json`.
pub fn mime_essence(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Every refusal both invoke surfaces make, in the order they make them.
///
/// `InvokeModel` and `InvokeModelWithResponseStream` take the SAME request — the
/// two shapes differ only in which header `accept` binds — so they refuse the same
/// requests for the same reasons, and both call this. `Err` carries the sentence
/// the caller answers with its own modelled `ValidationException` (400).
pub fn validate_invoke_request(
    family: ModelFamily,
    content_type: Option<&str>,
    accept: Option<&str>,
    body: &[u8],
    guardrail_identifier: Option<&str>,
    guardrail_version: Option<&str>,
) -> Result<(), String> {
    // "You must provide the body in JSON format", and the response is in the
    // format the `accept` header asks for, whose only documented value is
    // `application/json` (its default).
    if let Some(content_type) = content_type {
        if mime_essence(content_type) != JSON_MIME {
            return Err(INVALID_CONTENT_TYPE_MESSAGE.to_string());
        }
    }
    if let Some(accept) = accept {
        let essence = mime_essence(accept);
        if essence != JSON_MIME && essence != "*/*" {
            return Err(INVALID_ACCEPT_MESSAGE.to_string());
        }
    }
    if body.is_empty() {
        return Err("The request body must contain the prompt and inference parameters.".to_string());
    }
    // The body, decoded ONCE: the guardrail rule below and the inference-parameter
    // ranges further down both read it, so no request is parsed twice.
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();

    // The three guardrail combinations the model documents as errors. The
    // second one ("you enable the guardrail but the contentType isn't
    // application/json") is already refused above, for guarded and unguarded
    // requests alike.
    let guardrail_identifier = guardrail_identifier.filter(|value| !value.is_empty());
    let guardrail_version = guardrail_version.filter(|value| !value.is_empty());
    if guardrail_identifier.is_some() && guardrail_version.is_none() {
        return Err(
            "guardrailVersion must be specified when guardrailIdentifier is provided.".to_string(),
        );
    }
    if guardrail_identifier.is_none()
        && parsed
            .as_ref()
            .is_some_and(|value| value.get(INLINE_GUARDRAIL_CONFIG_FIELD).is_some())
    {
        return Err(format!(
            "guardrailIdentifier must be specified when the request body contains \
             {INLINE_GUARDRAIL_CONFIG_FIELD}."
        ));
    }

    // The body is model-specific, so the model's own family is what says
    // whether it can be read: a body that is not a JSON object, or that omits a
    // member the family's documented request schema requires, is refused naming
    // the violation.
    if let Some(message) = invoke_body_violation(family, body) {
        return Err(message);
    }
    // The body's inference parameters, against the ranges the provider documents
    // for them: a value outside one is refused rather than ignored, which is the
    // same contract the Converse API's own `inferenceConfig` is validated under.
    if let Some(message) = body_inference_range_violation(family, parsed.as_ref()) {
        return Err(message);
    }
    Ok(())
}

/// One served invoke-API inference: the model's own response envelope and the
/// token accounting computed for it.
#[derive(Debug, Clone)]
pub struct InvokeTurn {
    /// The response body in the model family's own envelope, as
    /// [`invoke_model_response_body`] renders it.
    pub response: serde_json::Value,
    /// The generated text this turn is, which the family's response envelope and
    /// its streaming events both carry.
    pub text: String,
    /// The server-generated completion id the family's envelope reports, where it
    /// reports one.
    pub completion_id: String,
    /// Whether generation ended at the request's token cap rather than on its
    /// own, which is what picks the family's terminal stop reason.
    pub stopped_at_token_cap: bool,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Serve one invoke-API inference from the body that was actually supplied.
///
/// `InvokeModel` and `InvokeModelWithResponseStream` both call this, so the
/// inference the two answer for identical input agrees by construction: the
/// unary surface returns [`InvokeTurn::response`] as its body, and the streaming
/// one renders the SAME turn as the family's own incremental events (see
/// [`invoke_model_stream_chunks`]). `family` is the model's own response family,
/// which the caller has already resolved (a model this emulator renders no
/// envelope for is refused before any inference is served).
pub fn invoke_turn(model: &ModelEntry, family: ModelFamily, body: &[u8]) -> InvokeTurn {
    let parsed_body = serde_json::from_slice::<serde_json::Value>(body).ok();
    // The generation cap the body carries for this family, bounded by the model's
    // own ceiling. It is a ceiling, so the completion is composed to fit it and
    // the family's length stop reason is reported only for a completion that
    // really reached it.
    let max_output_tokens = parsed_body
        .as_ref()
        .and_then(|value| body_max_tokens(family, value))
        .unwrap_or(model.max_output_tokens)
        .min(model.max_output_tokens);

    let input_tokens = body_input_tokens(body);
    // The stop sequences the body carries for this family. Generation halts
    // where the first of them completes, so the text the envelope and the
    // streaming events carry ends with that sequence and nothing follows it.
    let stop_sequences = parsed_body
        .as_ref()
        .map(|value| body_stop_sequences(family, value))
        .unwrap_or_default();
    let completion = emulated_completion(
        &model.model_id,
        1,
        input_tokens,
        max_output_tokens,
        &stop_sequences,
    );
    let text = completion.text;
    let truncated = completion.truncated;
    let output_tokens = count_text_tokens(&text);

    // A completion id is a per-run, server-generated value; deriving it from the
    // request keeps it stable for an identical call without inventing an id that
    // looks recorded.
    let completion_id = {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(model.model_id.as_bytes());
        digest.update(body);
        hex::encode(digest.finalize())[..24].to_string()
    };

    InvokeTurn {
        response: invoke_model_response_body(
            family,
            &model.model_id,
            &completion_id,
            &text,
            input_tokens,
            output_tokens,
            truncated,
        ),
        text,
        completion_id,
        stopped_at_token_cap: truncated,
        input_tokens,
        output_tokens,
    }
}

/// The prompt text an `InvokeModel` body carries.
///
/// A JSON body contributes the strings it binds under a prompt-bearing field
/// name; a body that is not JSON, or that binds no such field, contributes its
/// own bytes as text, so no request is counted as zero tokens just because this
/// emulator does not know the field it used.
pub fn prompt_text_of_body(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        let text = prompt_text_of_json(&value);
        if !text.trim().is_empty() {
            return text;
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// The tokens an `InvokeModel` body is. `CountTokens`'s `invokeModel` variant
/// and `InvokeModel` itself both count with this.
pub fn body_input_tokens(body: &[u8]) -> u32 {
    count_text_tokens(&prompt_text_of_body(body))
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

/// A Smithy document as JSON. The walk stops at [`MAX_DOCUMENT_DEPTH`], past
/// which a branch renders as `null`, so a deeply nested caller document cannot
/// drive unbounded recursion.
///
/// Object members are emitted in SORTED key order. `Document::Object` is a
/// `HashMap`, whose iteration order differs between two instances holding the
/// same keys, and `serde_json::Map` preserves insertion order whenever anything
/// in the workspace enables `serde_json/preserve_order`. Without the sort, two
/// renderings of the same document can differ — which would make
/// [`async_invoke_fingerprint`] non-deterministic and turn an idempotent retry
/// into a `ConflictException`.
pub fn document_to_json(document: &::aws_smithy_types::Document) -> serde_json::Value {
    fn walk(document: &::aws_smithy_types::Document, depth: usize) -> serde_json::Value {
        use ::aws_smithy_types::{Document, Number};
        if depth > MAX_DOCUMENT_DEPTH {
            return serde_json::Value::Null;
        }
        match document {
            Document::Object(members) => {
                let mut pairs: Vec<(&String, &Document)> = members.iter().collect();
                pairs.sort_unstable_by_key(|(name, _)| *name);
                serde_json::Value::Object(
                    pairs
                        .into_iter()
                        .map(|(name, member)| (name.clone(), walk(member, depth + 1)))
                        .collect(),
                )
            }
            Document::Array(items) => {
                serde_json::Value::Array(items.iter().map(|item| walk(item, depth + 1)).collect())
            }
            Document::Number(Number::PosInt(value)) => serde_json::Value::from(*value),
            Document::Number(Number::NegInt(value)) => serde_json::Value::from(*value),
            Document::Number(Number::Float(value)) => serde_json::Number::from_f64(*value)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Document::String(value) => serde_json::Value::String(value.clone()),
            Document::Bool(value) => serde_json::Value::Bool(*value),
            Document::Null => serde_json::Value::Null,
        }
    }
    walk(document, 0)
}
/// A JSON value as a Smithy document — the inverse of [`document_to_json`], so a
/// document this service composes as JSON reaches the wire as the modelled
/// `Document` member it belongs to. The walk stops at [`MAX_DOCUMENT_DEPTH`] for
/// the same reason, past which a branch becomes `Document::Null`.
pub fn json_to_document(value: &serde_json::Value) -> ::aws_smithy_types::Document {
    fn walk(value: &serde_json::Value, depth: usize) -> ::aws_smithy_types::Document {
        use ::aws_smithy_types::{Document, Number};
        if depth > MAX_DOCUMENT_DEPTH {
            return Document::Null;
        }
        match value {
            serde_json::Value::Object(members) => Document::Object(
                members
                    .iter()
                    .map(|(name, member)| (name.clone(), walk(member, depth + 1)))
                    .collect(),
            ),
            serde_json::Value::Array(items) => {
                Document::Array(items.iter().map(|item| walk(item, depth + 1)).collect())
            }
            serde_json::Value::String(text) => Document::String(text.clone()),
            serde_json::Value::Bool(flag) => Document::Bool(*flag),
            serde_json::Value::Number(number) => {
                if let Some(value) = number.as_u64() {
                    Document::Number(Number::PosInt(value))
                } else if let Some(value) = number.as_i64() {
                    Document::Number(Number::NegInt(value))
                } else if let Some(value) = number.as_f64() {
                    Document::Number(Number::Float(value))
                } else {
                    Document::Null
                }
            }
            serde_json::Value::Null => Document::Null,
        }
    }
    walk(value, 0)
}

// ---------------------------------------------------------------------------
// Tool arguments
// ---------------------------------------------------------------------------

/// The most elements an instantiated array argument carries.
///
/// `minItems` is a CALLER's number, so this is the EMULATOR's bound on it: a
/// schema declaring an enormous one must not turn one request into an unbounded
/// response. It is far above any argument list a tool declares.
pub const MAX_ARGUMENT_ARRAY_ITEMS: usize = 8;

/// The longest string an instantiated string argument carries, bounding
/// `minLength` and `maxLength` for the same reason.
pub const MAX_ARGUMENT_STRING_LENGTH: usize = 256;

/// The most values ONE argument document carries, counting every member and
/// element at every level.
///
/// Nesting MULTIPLIES: a schema that is small on the wire can describe a tree of
/// arrays of objects whose instantiation is exponential in its depth. This is the
/// EMULATOR's ceiling on the whole document, so the response one request composes
/// stays bounded however the schema nests; it is far above the argument object of
/// any tool a client declares. A schema asking for more is instantiated as far as
/// the budget reaches: a member or an element past it is not carried at all, and
/// a value the schema FIXES costs the number of values it itself carries (see
/// [`fixed_value`]), so a large caller value cannot be reproduced many times
/// over.
pub const MAX_ARGUMENT_VALUES: usize = 4096;

/// The most bytes ONE argument document is composed of: every value's own
/// rendering, every member name, and a small allowance per value for the
/// punctuation around it.
///
/// A value COUNT bounds how many values a document carries but not how large
/// each one is, and a schema can fix a value of any size (a `const`, a
/// `default`, an `enum` member) and name it from as many properties as it likes.
/// This is the EMULATOR's ceiling on the SIZE of the document one request
/// composes, so a schema cannot amplify a large caller value into a far larger
/// response. It is far above the argument object of any tool a client declares.
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;

/// What one value costs before its own rendering: the separators, quotes and
/// brackets a document spends on carrying it.
const ARGUMENT_VALUE_OVERHEAD_BYTES: usize = 8;

/// What is left of the ceilings ONE argument document is composed within:
/// [`MAX_ARGUMENT_VALUES`] values and [`MAX_ARGUMENT_BYTES`] bytes.
///
/// Everything the projection emits is charged here — a member, an element, a
/// member name, a placeholder and a value the schema fixes alike — so the
/// document one request composes is bounded in both count and size.
#[derive(Debug)]
struct ArgumentBudget {
    values: usize,
    bytes: usize,
}

impl ArgumentBudget {
    /// A whole budget, for one argument document.
    fn new() -> Self {
        Self {
            values: MAX_ARGUMENT_VALUES,
            bytes: MAX_ARGUMENT_BYTES,
        }
    }

    /// Whether nothing more can be composed at all.
    fn is_spent(&self) -> bool {
        self.values == 0 || self.bytes == 0
    }

    /// Charge `values` values and `bytes` bytes, answering whether the budget
    /// could pay: what it cannot pay for is not carried.
    fn charge(&mut self, values: usize, bytes: usize) -> bool {
        if values > self.values || bytes > self.bytes {
            return false;
        }
        self.values -= values;
        self.bytes -= bytes;
        true
    }
}

/// The argument document a tool request carries: the instantiation of the input
/// schema the requested tool was DECLARED with.
///
/// A tool is called with the argument object its own
/// `toolSpec.inputSchema.json` describes — a JSON Schema whose top-level type is
/// `object` — so every name that schema REQUIRES is a key of the document,
/// carrying a value of the type the schema declares for it, and where the schema
/// requires nothing, every property it declares is a key. A tool entry that
/// declares no schema (a system-defined tool) names no arguments, so its request
/// carries an empty argument object.
///
/// No language model runs in this emulator, so a value the schema does not fix
/// itself (`const`, `default`, an `enum`'s first member) is a placeholder of the
/// declared type derived from the declaration ([`placeholder_string`] and the
/// numeric placeholders below) — never invented content presented as inference.
/// Which arguments a forced tool call carries is the schema's, not the model's.
pub fn tool_input_arguments(
    config: &ToolConfiguration,
    tool_name: &str,
) -> ::aws_smithy_types::Document {
    let declared = config.tools.iter().find_map(|tool| match tool {
        Tool::toolSpec(spec) if spec.name == tool_name => match &spec.input_schema {
            ToolInputSchema::Json(schema) => Some(document_to_json(schema)),
        },
        // Another tool's specification, a system-defined tool and a cache-point
        // marker all declare no schema for THIS request.
        Tool::toolSpec(_) | Tool::systemTool(_) | Tool::cachePoint(_) => None,
    });
    let mut budget = ArgumentBudget::new();
    let arguments = match &declared {
        Some(schema) => instantiate_schema(schema, schema, "", 0, &mut budget),
        None => serde_json::Value::Object(serde_json::Map::new()),
    };
    // "The top level schema type must be object": a schema describing anything
    // else names no arguments at all, so the tool is requested with an empty
    // argument object rather than with an input that is not one.
    let arguments = match arguments {
        object @ serde_json::Value::Object(_) => object,
        _ => serde_json::Value::Object(serde_json::Map::new()),
    };
    json_to_document(&arguments)
}

/// One instance of the JSON Schema `schema`: a value of the type it declares.
///
/// `root` is the whole schema a local `$ref` is resolved inside, `name` is the
/// property this value is being composed for (what a string placeholder is
/// derived from), `depth` bounds the walk at [`MAX_DOCUMENT_DEPTH`], so neither
/// deep nesting nor a `$ref` cycle drives unbounded recursion, and `budget` is
/// what is left of the document's ceilings ([`ArgumentBudget`]) — every value
/// emitted is charged for what it is and what it renders as, so the whole
/// document stays bounded whatever the schema nests, names or fixes.
fn instantiate_schema(
    schema: &serde_json::Value,
    root: &serde_json::Value,
    name: &str,
    depth: usize,
    budget: &mut ArgumentBudget,
) -> serde_json::Value {
    use serde_json::Value;
    if depth > MAX_DOCUMENT_DEPTH || !budget.charge(1, ARGUMENT_VALUE_OVERHEAD_BYTES) {
        return Value::Null;
    }
    // A schema that is not an object — `true`, the schema admitting anything —
    // declares no type to instantiate.
    let Some(members) = schema.as_object() else {
        return Value::Null;
    };
    // A value the schema itself carries IS the value: nothing is composed for a
    // property the declaration already pins down.
    if let Some(fixed) = members.get("const") {
        return fixed_value(fixed, budget);
    }
    if let Some(offered) = members
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return fixed_value(offered, budget);
    }
    if let Some(default) = members.get("default") {
        return fixed_value(default, budget);
    }
    if let Some(target) = members
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| resolve_local_ref(root, reference))
    {
        return instantiate_schema(target, root, name, depth + 1, budget);
    }
    match declared_type(members) {
        Some("object") => instantiate_object(members, root, depth, budget),
        Some("array") => instantiate_array(members, root, name, depth, budget),
        Some("string") => charged(Value::String(placeholder_string(members, name)), budget),
        Some("integer") => charged(placeholder_integer(members), budget),
        Some("number") => charged(placeholder_number(members), budget),
        Some("boolean") => Value::Bool(false),
        // `null`, and any type name this projection does not recognise: a value
        // of no composable type.
        Some(_) => Value::Null,
        // A composed schema (`allOf`, `anyOf`, `oneOf`) declares its type in its
        // branches, and the first branch that declares one is instantiated.
        None => match composed_branch(members) {
            Some(branch) => instantiate_schema(branch, root, name, depth + 1, budget),
            None => Value::Null,
        },
    }
}

/// A composed scalar, charged for the bytes it renders as — a declared bound can
/// make a number's rendering far longer than the allowance one value carries.
/// What the budget cannot pay for is not carried.
fn charged(value: serde_json::Value, budget: &mut ArgumentBudget) -> serde_json::Value {
    if budget.charge(0, value.to_string().len()) {
        value
    } else {
        serde_json::Value::Null
    }
}

/// The value a schema fixes (`const`, `default`, an `enum`'s first member),
/// charged by the number of values it CARRIES and by the bytes it renders as.
///
/// A fixed value is a caller document of any size and any shape, so paying one
/// value for it would let one schema amplify it into an arbitrarily large
/// argument document — the more so as a `$ref` can name the same definition from
/// as many properties as the schema likes. What the remaining budget cannot pay
/// for is not carried at all.
fn fixed_value(value: &serde_json::Value, budget: &mut ArgumentBudget) -> serde_json::Value {
    let values = counted_values(value, budget.values);
    if values > budget.values {
        return serde_json::Value::Null;
    }
    let bytes = match value.as_str() {
        // A string renders as itself between quotes, so an oversized one is
        // refused without rendering it at all.
        Some(text) => text.len().saturating_add(2),
        None => value.to_string().len(),
    };
    if !budget.charge(values, bytes) {
        return serde_json::Value::Null;
    }
    value.clone()
}

/// How many values `value` carries, counted no further than `ceiling`.
///
/// The count stops as soon as the ceiling is passed, so measuring an enormous
/// caller value costs a bounded walk; the walk is iterative, so it is not
/// bounded by the stack either.
fn counted_values(value: &serde_json::Value, ceiling: usize) -> usize {
    let mut pending = vec![value];
    let mut counted = 0usize;
    while let Some(next) = pending.pop() {
        counted += 1;
        if counted > ceiling {
            return counted;
        }
        match next {
            serde_json::Value::Object(members) => pending.extend(members.values()),
            serde_json::Value::Array(items) => pending.extend(items.iter()),
            _ => {}
        }
    }
    counted
}

/// The JSON type `members` declares, or `None` when it declares none.
///
/// `type` is a single name or a list of names any of which is valid, in which
/// case the first that is not `null` is the one instantiated. A schema naming no
/// `type` but describing members or elements declares its type that way.
fn declared_type(members: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    match members.get("type") {
        Some(serde_json::Value::String(name)) => return Some(name.as_str()),
        Some(serde_json::Value::Array(names)) => {
            let mut first = None;
            for name in names.iter().filter_map(serde_json::Value::as_str) {
                if name != "null" {
                    return Some(name);
                }
                first = first.or(Some(name));
            }
            if first.is_some() {
                return first;
            }
        }
        _ => {}
    }
    if members.contains_key("properties") || members.contains_key("required") {
        return Some("object");
    }
    if members.contains_key("items") {
        return Some("array");
    }
    None
}

/// The branch of a composed schema that is instantiated: the first entry of
/// `allOf`, `anyOf` or `oneOf` that is a schema.
fn composed_branch(
    members: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Value> {
    ["allOf", "anyOf", "oneOf"].into_iter().find_map(|keyword| {
        members
            .get(keyword)?
            .as_array()?
            .iter()
            .find(|branch| branch.is_object())
    })
}

/// The object an `object` schema describes: one member per name it requires,
/// and — where it requires none — one per property it declares, within what is
/// left of the document's ceilings: EVERY member emitted is charged for, its
/// name included and a member the schema declares no type for included, so a
/// schema cannot amplify one request by naming members the ceiling does not
/// cover.
fn instantiate_object(
    members: &serde_json::Map<String, serde_json::Value>,
    root: &serde_json::Value,
    depth: usize,
    budget: &mut ArgumentBudget,
) -> serde_json::Value {
    use serde_json::Value;
    let properties = members.get("properties").and_then(Value::as_object);
    let required: Vec<String> = members
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = if required.is_empty() {
        properties
            .map(|declared| declared.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        required
    };
    let mut arguments = serde_json::Map::new();
    for name in names {
        // The member's own name is part of the document, so it is charged for
        // before anything is composed for it.
        if budget.is_spent() || !budget.charge(0, name.len()) {
            break;
        }
        // A name the schema requires without declaring a property for it takes
        // whatever `additionalProperties` declares, and otherwise has no
        // declared type at all.
        let declared = properties.and_then(|declared| declared.get(&name)).or_else(|| {
            members
                .get("additionalProperties")
                .filter(|schema| schema.is_object())
        });
        let value = match declared {
            Some(schema) => instantiate_schema(schema, root, &name, depth + 1, budget),
            None => {
                // A member of no declared type is still a member, so it is
                // charged for like any other.
                if !budget.charge(1, ARGUMENT_VALUE_OVERHEAD_BYTES) {
                    break;
                }
                Value::Null
            }
        };
        arguments.insert(name, value);
    }
    Value::Object(arguments)
}

/// The array an `array` schema describes: its declared element type, as many
/// times as the schema's own bounds ask for, within
/// [`MAX_ARGUMENT_ARRAY_ITEMS`] and within what is left of the document's
/// ceilings — an element the budget cannot pay for is not carried at all,
/// rather than carried empty. A schema declaring no element type carries no
/// element.
fn instantiate_array(
    members: &serde_json::Map<String, serde_json::Value>,
    root: &serde_json::Value,
    name: &str,
    depth: usize,
    budget: &mut ArgumentBudget,
) -> serde_json::Value {
    use serde_json::Value;
    let Some(items) = members.get("items") else {
        return Value::Array(Vec::new());
    };
    // The tuple form: each entry of `items` declares the element at its own
    // position. Any other form declares one element type for the whole array,
    // instantiated as many times as its `minItems` asks for.
    let positional = items.as_array();
    let count = match positional {
        Some(schemas) => schemas.len().min(MAX_ARGUMENT_ARRAY_ITEMS),
        None => {
            let minimum = members.get("minItems").and_then(Value::as_u64).unwrap_or(1);
            let maximum = members
                .get("maxItems")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            usize::try_from(minimum.min(maximum))
                .unwrap_or(MAX_ARGUMENT_ARRAY_ITEMS)
                .min(MAX_ARGUMENT_ARRAY_ITEMS)
        }
    };
    let mut elements = Vec::new();
    for index in 0..count {
        if budget.is_spent() {
            break;
        }
        let element = match positional {
            Some(schemas) => match schemas.get(index) {
                Some(schema) => schema,
                None => break,
            },
            None => items,
        };
        elements.push(instantiate_schema(element, root, name, depth + 1, budget));
    }
    Value::Array(elements)
}

/// The string a `string` property is instantiated with: a placeholder derived
/// from the property's own NAME, cut to the schema's `maxLength` and padded to
/// its `minLength` so the value meets the declared bounds.
///
/// No language model runs in this emulator, so a property whose value the schema
/// does not fix carries no inferred content: the placeholder says which argument
/// it is and nothing more.
fn placeholder_string(members: &serde_json::Map<String, serde_json::Value>, name: &str) -> String {
    let maximum = members
        .get("maxLength")
        .and_then(serde_json::Value::as_u64)
        .map_or(MAX_ARGUMENT_STRING_LENGTH, |max| {
            usize::try_from(max)
                .unwrap_or(usize::MAX)
                .min(MAX_ARGUMENT_STRING_LENGTH)
        });
    let minimum = members
        .get("minLength")
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |min| usize::try_from(min).unwrap_or(usize::MAX))
        .min(maximum);
    let declared = if name.is_empty() { "value" } else { name };
    let mut text: String = declared.chars().take(maximum).collect();
    while text.chars().count() < minimum {
        text.push('_');
    }
    text
}

/// The integer an `integer` property is instantiated with: zero where the
/// schema's declared range admits it, and otherwise the nearest value it does.
fn placeholder_integer(members: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let lower = members
        .get("minimum")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(i64::MIN);
    let upper = members
        .get("maximum")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(i64::MAX);
    // A range whose bounds contradict each other admits nothing; its upper bound
    // is what such a declaration is instantiated with.
    let value = if upper < lower {
        upper
    } else {
        0i64.clamp(lower, upper)
    };
    serde_json::Value::from(value)
}

/// The number a `number` property is instantiated with, by the same rule
/// [`placeholder_integer`] states.
fn placeholder_number(members: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    // A bound that is not a finite number is no bound at all, which is also
    // what keeps the comparison below total.
    let lower = members
        .get("minimum")
        .and_then(serde_json::Value::as_f64)
        .filter(|bound| bound.is_finite())
        .unwrap_or(f64::NEG_INFINITY);
    let upper = members
        .get("maximum")
        .and_then(serde_json::Value::as_f64)
        .filter(|bound| bound.is_finite())
        .unwrap_or(f64::INFINITY);
    let value = if upper < lower {
        upper
    } else {
        0f64.clamp(lower, upper)
    };
    serde_json::Number::from_f64(value)
        .map_or(serde_json::Value::from(0), serde_json::Value::Number)
}

/// The schema a local `$ref` names, resolved inside `root` as the JSON Pointer
/// it is (`#/$defs/Name`), or `None` for a reference this projection cannot
/// resolve: one into another document, or the whole schema itself.
fn resolve_local_ref<'a>(
    root: &'a serde_json::Value,
    reference: &str,
) -> Option<&'a serde_json::Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return None;
    }
    root.pointer(pointer)
}

// ---------------------------------------------------------------------------
// Completions
// ---------------------------------------------------------------------------

/// The whole milliseconds a served invocation took, as every modelled latency
/// member reports it.
///
/// A latency member is the latency MEASURED for the invocation that was served,
/// and serving one takes a nonzero amount of time, so the measurement rounds UP
/// to the next whole millisecond rather than being reported as no time at all:
/// one millisecond is the granularity these members are reported in, and it is
/// the smallest value a served invocation can honestly report.
pub fn latency_ms(elapsed: std::time::Duration) -> i64 {
    let millis = elapsed.as_micros().div_ceil(1_000).max(1);
    i64::try_from(millis).unwrap_or(i64::MAX)
}

/// The completion `text` as a configured stop sequence halts it, or `None` when
/// it carries none of them.
///
/// A stop sequence is "a sequence of characters that causes the model to stop
/// generating the response" (`InferenceConfiguration.stopSequences`), so
/// generation ends the moment one of them has been generated in full: the served
/// text ends WITH that sequence and carries nothing after it, whatever
/// generation budget is left over. Several configured sequences are one rule —
/// generation stops at whichever of them completes first, which is the earliest
/// offset any occurrence ends at.
///
/// Every inference surface that lets a request configure stop sequences reads
/// this one function, so the text a halted turn serves is the same whether the
/// sequences arrived in `inferenceConfig` or in a model family's own request
/// body, and the streaming surfaces frame exactly the text their unary siblings
/// return.
pub fn halt_at_stop_sequence(text: &str, stop_sequences: &[String]) -> Option<String> {
    stop_sequences
        .iter()
        .filter(|sequence| !sequence.is_empty())
        .filter_map(|sequence| {
            text.find(sequence.as_str())
                .map(|at| at.saturating_add(sequence.len()))
        })
        .min()
        .map(|end| text[..end].to_string())
}

/// The whole completion [`emulated_completion`] composes when no generation cap
/// and no stop sequence constrains it.
pub fn emulated_completion_text(model_id: &str, messages: u32, input_tokens: u32) -> String {
    emulated_completion(model_id, messages, input_tokens, u32::MAX, &[]).text
}

/// The refusal message for an `InvokeModel` body that is not a readable request of the
/// invoked family at all: bytes whose JSON cannot be PARSED, or a JSON document that parses
/// to something other than an object, so no member of the family's documented request schema
/// can be read off it.
///
/// "You must provide the body in JSON format" is all the operation's own documentation says
/// about such a body, and this is the sentence Amazon Bedrock states the refusal in for a
/// model whose provider publishes its own native request envelope — the recorded one for
/// that class, for an unparseable document as for a parsed non-object. The fault is the
/// DOCUMENT rather than any one member of it, so the message names no member and carries no
/// value of the request; a body that IS the family's request object but omits a member it
/// requires is reported naming that member instead ([`invoke_body_violation`]).
///
/// It is not one constant for the whole catalogue: the body is the PROVIDER's format, so the
/// validator that refuses it — and the sentence it refuses it in — is the provider's. The
/// wording per family is [`malformed_body_message`].
pub const MALFORMED_BODY_MESSAGE: &str =
    "Malformed input request, please reformat your input and try again.";

/// The sentence a body that is no request object of `family` at all is refused in.
///
/// Amazon Bedrock hands `InvokeModel$body` to the provider of the invoked model in that
/// provider's own format, so the parse failure is reported by the provider's request
/// validator and the wording follows the FAMILY the model id resolves to, not the
/// operation: a provider served on the OpenAI-compatible chat-completions envelope answers
/// its own sentence ([`openai_chat::MALFORMED_BODY_MESSAGE`], recorded for a model of that
/// envelope), and a provider with its own native envelope answers
/// [`MALFORMED_BODY_MESSAGE`]. Both are entirely fixed text.
pub fn malformed_body_message(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::OpenAiChat => openai_chat::MALFORMED_BODY_MESSAGE,
        ModelFamily::AnthropicMessages
        | ModelFamily::TitanText
        | ModelFamily::Nova
        | ModelFamily::Llama
        | ModelFamily::Mistral
        | ModelFamily::CohereCommand => MALFORMED_BODY_MESSAGE,
    }
}

/// The members a family's `InvokeModel` request body must bind.
///
/// One entry per required slot, as documented per family in "Inference request
/// parameters and response fields for foundation models" in the Amazon Bedrock
/// User Guide: the body must bind one of the slot's names (a family that takes
/// either a raw prompt or a chat transcript has both), and the FIRST name is the
/// one a refusal reports as the missing key. Slots are ordered with the member
/// carrying the prompt first, so a body that binds nothing at all is reported
/// against it.
fn required_body_slots(family: ModelFamily) -> &'static [&'static [&'static str]] {
    match family {
        // Anthropic Claude Messages API: messages, max_tokens and
        // anthropic_version are all documented Required.
        ModelFamily::AnthropicMessages => &[&["messages"], &["max_tokens"], &["anthropic_version"]],
        ModelFamily::TitanText => &[&["inputText"]],
        ModelFamily::Nova => &[&["messages"]],
        ModelFamily::Llama => &[&["prompt"]],
        // Mistral takes either a raw prompt or a chat transcript.
        ModelFamily::Mistral => &[&["prompt", "messages"]],
        // Cohere Command (text) takes `prompt`, Command R takes `message`.
        ModelFamily::CohereCommand => &[&["prompt", "message"]],
        // The OpenAI-compatible chat completions API takes a chat transcript, or a raw
        // prompt in the completion form of the same API ([`openai_chat`]).
        ModelFamily::OpenAiChat => openai_chat::REQUIRED_SLOTS,
    }
}

/// The refusal `InvokeModel` owes for a body that does not satisfy `family`'s
/// documented request schema, or `None` when it does.
///
/// The body is model-specific, so this is the only place the request's own shape is
/// checked, and it can fail in two ways the service words differently: a body that is no
/// request of the family at all — bytes whose JSON cannot be parsed, or a document that
/// parses to something other than an object — is refused as the malformed input request it
/// is, in the words that family's provider states that class in
/// ([`malformed_body_message`]); one that IS the family's request object but omits a member
/// the family requires is reported naming that member, as a schema violation is.
pub fn invoke_body_violation(family: ModelFamily, body: &[u8]) -> Option<String> {
    let members = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(members)) => members,
        Ok(_) | Err(_) => return Some(malformed_body_message(family).to_string()),
    };
    for slot in required_body_slots(family) {
        if slot.iter().any(|name| members.contains_key(*name)) {
            continue;
        }
        let name = slot.first().copied().unwrap_or_default();
        return Some(format!(
            "Malformed input request: #: required key [{name}] not found, please reformat your \
             input and try again."
        ));
    }
    None
}

/// The `InvokeModel` response body for `family`, in that family's own response
/// envelope.
///
/// Envelopes as documented per family in "Inference request parameters and
/// response fields for foundation models" in the Amazon Bedrock User Guide.
pub fn invoke_model_response_body(
    family: ModelFamily,
    model_id: &str,
    completion_id: &str,
    text: &str,
    input_tokens: u32,
    output_tokens: u32,
    stop_reason_is_max_tokens: bool,
) -> serde_json::Value {
    let stop_reason = family_stop_reason(family, stop_reason_is_max_tokens);
    match family {
        ModelFamily::AnthropicMessages => serde_json::json!({
            "id": format!("msg_{completion_id}"),
            "type": "message",
            "role": "assistant",
            "model": model_id,
            "content": [{ "type": "text", "text": text }],
            "stop_reason": stop_reason,
            "stop_sequence": serde_json::Value::Null,
            "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens },
        }),
        ModelFamily::TitanText => serde_json::json!({
            "inputTextTokenCount": input_tokens,
            "results": [{
                "tokenCount": output_tokens,
                "outputText": text,
                "completionReason": stop_reason,
            }],
        }),
        ModelFamily::Nova => serde_json::json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": text }] } },
            "stopReason": stop_reason,
            "usage": nova_usage(input_tokens, output_tokens),
        }),
        ModelFamily::Llama => serde_json::json!({
            "generation": text,
            "prompt_token_count": input_tokens,
            "generation_token_count": output_tokens,
            "stop_reason": stop_reason,
        }),
        ModelFamily::Mistral => serde_json::json!({
            "outputs": [{
                "text": text,
                "stop_reason": stop_reason,
            }],
        }),
        ModelFamily::CohereCommand => serde_json::json!({
            "id": completion_id,
            "generations": [{
                "id": format!("gen_{completion_id}"),
                "text": text,
                "finish_reason": stop_reason,
            }],
        }),
        ModelFamily::OpenAiChat => openai_chat::response_body(
            model_id,
            completion_id,
            text,
            input_tokens,
            output_tokens,
            stop_reason,
            now_seconds(),
        ),
    }
}

/// The token accounting Amazon Nova reports, in the shape its response envelope
/// and its `metadata` streaming event both carry.
///
/// Nova reports its prompt-cache accounting on every invocation. A cache
/// checkpoint is marked with the Converse API's `cachePoint` block, which this
/// body envelope does not carry, so no token of an `InvokeModel` body was read
/// from or written to a cache.
fn nova_usage(input_tokens: u32, output_tokens: u32) -> serde_json::Value {
    serde_json::json!({
        "inputTokens": input_tokens,
        "outputTokens": output_tokens,
        "totalTokens": input_tokens.saturating_add(output_tokens),
        "cacheReadInputTokenCount": 0,
        "cacheWriteInputTokenCount": 0,
    })
}

/// The stop reason `family` reports for a turn that ended on its own, and for one
/// that ended at the request's generation cap — in the family's own vocabulary.
///
/// Per family as documented in "Inference request parameters and response fields
/// for foundation models" in the Amazon Bedrock User Guide. The unary response
/// envelope and the streaming events read it from here, so the two cannot report
/// a turn as having ended differently.
pub fn family_stop_reason(family: ModelFamily, stopped_at_token_cap: bool) -> &'static str {
    match (family, stopped_at_token_cap) {
        (ModelFamily::AnthropicMessages | ModelFamily::Nova, true) => "max_tokens",
        (ModelFamily::AnthropicMessages | ModelFamily::Nova, false) => "end_turn",
        (ModelFamily::TitanText, true) => "LENGTH",
        (ModelFamily::TitanText, false) => "FINISH",
        (ModelFamily::Llama | ModelFamily::Mistral, true) => "length",
        (ModelFamily::Llama | ModelFamily::Mistral, false) => "stop",
        (ModelFamily::CohereCommand, true) => "MAX_TOKENS",
        (ModelFamily::CohereCommand, false) => "COMPLETE",
        (ModelFamily::OpenAiChat, true) => openai_chat::FINISH_REASON_LENGTH,
        (ModelFamily::OpenAiChat, false) => openai_chat::FINISH_REASON_STOP,
    }
}

// ---------------------------------------------------------------------------
// Response streaming
// ---------------------------------------------------------------------------

/// The key Amazon Bedrock itself adds to the LAST chunk of a response stream,
/// beside the model's own terminal event: the per-invocation accounting the
/// service measured. It is not a member of any model's streaming schema and not a
/// modelled member of `PayloadPart`, which is why it travels inside the chunk's
/// payload rather than as a modelled field.
pub const INVOCATION_METRICS_FIELD: &str = "amazon-bedrock-invocationMetrics";

/// The accounting [`INVOCATION_METRICS_FIELD`] carries: the tokens the invocation
/// actually counted and the latencies it actually measured.
#[derive(Debug, Clone, Copy)]
pub struct InvocationMetrics {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// How long serving the whole invocation took.
    pub invocation_latency_ms: i64,
    /// How long it took until the stream's first chunk was ready.
    pub first_byte_latency_ms: i64,
}

impl InvocationMetrics {
    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "inputTokenCount": self.input_tokens,
            "outputTokenCount": self.output_tokens,
            "invocationLatency": self.invocation_latency_ms,
            "firstByteLatency": self.first_byte_latency_ms,
        })
    }
}

/// The payloads the `chunk` events of one `InvokeModelWithResponseStream` body
/// carry, in order: ONE incremental event of `family`'s own streaming protocol per
/// chunk, with [`INVOCATION_METRICS_FIELD`] added to the terminal one.
///
/// The event sequences are each family's own, as documented per family in
/// "Inference request parameters and response fields for foundation models" in the
/// Amazon Bedrock User Guide: a message-opening event where the family has one,
/// then the incremental content events, then the family's terminal event carrying
/// the stop reason and the invocation's token accounting. The streamed text and
/// the stop reason are the ones [`invoke_turn`] served, so the streaming surface
/// and the unary one cannot report different inference for the same request.
///
/// No language model runs in this emulator, so the turn's text is produced in one
/// step (see [`emulated_completion_text`]) and therefore arrives as one content
/// delta: nothing here splits it into fabricated increments.
pub fn invoke_model_stream_chunks(
    family: ModelFamily,
    model_id: &str,
    turn: &InvokeTurn,
    metrics: InvocationMetrics,
) -> Vec<serde_json::Value> {
    let text = turn.text.as_str();
    let completion_id = turn.completion_id.as_str();
    let stop_reason = family_stop_reason(family, turn.stopped_at_token_cap);
    let input_tokens = turn.input_tokens;
    let output_tokens = turn.output_tokens;

    let mut chunks = match family {
        // The Anthropic Claude Messages API's own server-sent event types, each
        // carried in its `type` member.
        ModelFamily::AnthropicMessages => vec![
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": format!("msg_{completion_id}"),
                    "type": "message",
                    "role": "assistant",
                    "model": model_id,
                    "content": [],
                    "stop_reason": serde_json::Value::Null,
                    "stop_sequence": serde_json::Value::Null,
                    "usage": { "input_tokens": input_tokens, "output_tokens": 0 },
                },
            }),
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" },
            }),
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": text },
            }),
            serde_json::json!({ "type": "content_block_stop", "index": 0 }),
            serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": serde_json::Value::Null },
                "usage": { "output_tokens": output_tokens },
            }),
            serde_json::json!({ "type": "message_stop" }),
        ],
        // Amazon Titan Text streams its result as output-text chunks; the
        // terminal one reports the completion reason and the token counts.
        ModelFamily::TitanText => vec![
            serde_json::json!({
                "outputText": text,
                "index": 0,
                "totalOutputTextTokenCount": serde_json::Value::Null,
                "completionReason": serde_json::Value::Null,
                "inputTextTokenCount": input_tokens,
            }),
            serde_json::json!({
                "outputText": "",
                "index": 0,
                "totalOutputTextTokenCount": output_tokens,
                "completionReason": stop_reason,
                "inputTextTokenCount": input_tokens,
            }),
        ],
        // Amazon Nova's body is Converse-shaped, so its streaming events are the
        // Converse stream's: the message opens, each content block is delivered
        // as deltas and closed, the message stops, and the turn's usage and
        // measured latency follow as metadata.
        ModelFamily::Nova => vec![
            serde_json::json!({ "messageStart": { "role": "assistant" } }),
            serde_json::json!({
                "contentBlockDelta": { "contentBlockIndex": 0, "delta": { "text": text } },
            }),
            serde_json::json!({ "contentBlockStop": { "contentBlockIndex": 0 } }),
            serde_json::json!({ "messageStop": { "stopReason": stop_reason } }),
            serde_json::json!({
                "metadata": {
                    "usage": nova_usage(input_tokens, output_tokens),
                    "metrics": { "latencyMs": metrics.invocation_latency_ms },
                },
            }),
        ],
        // Meta Llama streams generation chunks whose terminal one carries the
        // stop reason and the prompt and generation token counts.
        ModelFamily::Llama => vec![
            serde_json::json!({
                "generation": text,
                "prompt_token_count": serde_json::Value::Null,
                "generation_token_count": output_tokens,
                "stop_reason": serde_json::Value::Null,
            }),
            serde_json::json!({
                "generation": "",
                "prompt_token_count": input_tokens,
                "generation_token_count": output_tokens,
                "stop_reason": stop_reason,
            }),
        ],
        // Mistral AI streams its `outputs` envelope, the terminal chunk carrying
        // the stop reason.
        ModelFamily::Mistral => vec![
            serde_json::json!({
                "outputs": [{ "text": text, "stop_reason": serde_json::Value::Null }],
            }),
            serde_json::json!({
                "outputs": [{ "text": "", "stop_reason": stop_reason }],
            }),
        ],
        // Cohere Command streams typed events, ending with the stream-end event
        // that repeats the whole response.
        ModelFamily::CohereCommand => vec![
            serde_json::json!({
                "is_finished": false,
                "event_type": "text-generation",
                "text": text,
            }),
            serde_json::json!({
                "is_finished": true,
                "event_type": "stream-end",
                "finish_reason": stop_reason,
                "response": turn.response.clone(),
            }),
        ],
        // The streaming form of the OpenAI-compatible chat completions envelope:
        // `chat.completion.chunk` objects whose `delta` carries the increment
        // ([`openai_chat`]).
        ModelFamily::OpenAiChat => openai_chat::stream_chunks(
            model_id,
            completion_id,
            text,
            input_tokens,
            output_tokens,
            stop_reason,
            now_seconds(),
        ),
    };

    // The service's own accounting, on the terminal chunk and nowhere else.
    if let Some(last) = chunks.last_mut().and_then(serde_json::Value::as_object_mut) {
        last.insert(INVOCATION_METRICS_FIELD.to_string(), metrics.to_json());
    }
    chunks
}

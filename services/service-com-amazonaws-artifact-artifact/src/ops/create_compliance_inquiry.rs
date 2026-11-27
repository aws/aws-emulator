// SPDX-License-Identifier: CC0-1.0

//! @op CreateComplianceInquiry
//! Creates a compliance inquiry: mints its `InquiryId`, enumerates the queries its
//! content carries, and persists the record in [`crate::state`], so every later read of
//! it — `GetComplianceInquiryMetadata` here, `ListComplianceInquiries` and the tag
//! operations once they are implemented — answers from the one stored resource.
//!
//! Four rules shape the handler:
//!
//! * The inquiry is created NON-TERMINAL. AWS answers this operation with HTTP 202 and
//!   an inquiry whose `status` is `PROCESSING`, so the terminal status is owed by the
//!   CLOCK, not by any later call: the due instant is stored
//!   (`crate::state::INQUIRY_PROCESSING_SPAN`) and
//!   [`crate::state::ComplianceInquiry::materialise`] applies the transition when a
//!   reader arrives after it. Nothing sleeps, and a caller polling straight after the
//!   create still observes `PROCESSING`.
//! * `clientToken` is this operation's `@idempotencyToken`: a repeat carrying the same
//!   token answers with the inquiry the first call created rather than creating a second,
//!   and the same token on DIFFERENT content is refused with the modelled
//!   `ConflictException` — an idempotency token identifies one call, so honouring it on
//!   other content would hide the second submission.
//! * The name every READ of the inquiry reports is DERIVED FROM ITS CONTENT when the
//!   content derives one: a submission made of a single text query is reported under that
//!   query text, which is the only content-derived handle `InquirySummary` exposes, so a
//!   caller that submitted a query finds its inquiry in `ListComplianceInquiries` under
//!   exactly what it sent. The caller-supplied title is kept verbatim on the record and
//!   stands as the reported name only for content that derives none (an uploaded
//!   document, whose queries are written inside the file). The one projection
//!   ([`crate::state::ComplianceInquiry::display_name`]) serves this create, the listing,
//!   `GetComplianceInquiryMetadata` and the exported document.
//! * The queries an inquiry owns are the UNITS OF WORK its submitted content carries, one
//!   each: the query a text submission carries, and one per section of an uploaded
//!   document — "[l]ist of file sections/sheets to process"
//!   (`InquiryFileContent$fileSections`) — with the document as a whole when the request
//!   names no section. The one enumeration
//!   ([`crate::state::InquiryContentRecord::work_units`]) serves every content form, so
//!   `ListComplianceInquiryQueries` reports what was submitted for processing whatever
//!   form it took, and every one of them carries the review the one support mode the model
//!   admits asks for ([`crate::state::REVIEW_TYPE_AI`]).
//! * `supportMode` "[d]efaults to AI_ONLY if not specified."
//!   (`CreateComplianceInquiryRequest$supportMode`), and `AI_ONLY` is the whole of the
//!   modelled `InquirySupportMode` enum, so a request naming no mode is processed under
//!   that documented default and a request naming any other value is refused against the
//!   enum before this handler runs. The inquiry therefore only ever stores — and every read
//!   only ever reports — a support mode its queries really were reviewed under.
//! * A tag the request carries is written onto the inquiry it creates, under the same rule
//!   the tag write enforces: a key in the reserved `aws:` namespace
//!   ([`crate::state::RESERVED_TAG_KEY_PREFIX`]) is Amazon Web Services' own, so such a
//!   request is refused with the modelled 400 `ValidationException` naming `tags` and no
//!   inquiry is created.
//! * The inquiry's RESPONSES are RETRIEVED from the published report catalog this service
//!   serves: the documents whose text carries the submitted query's terms are the sources,
//!   and the response a query reports is the passages of those documents, each under the
//!   source it was read out of, with a citation naming where that document is published
//!   ([`crate::state::answer_from_catalog`]). Nothing is synthesised to look generated — a
//!   query the catalog holds nothing bearing on carries no `response` and no citation, a
//!   query is reported under the handle its submission gave it, never under question text
//!   nothing read, and no query is failed for work nothing ran.
//! * Only a submission carrying NOTHING to process ends in the modelled failure state
//!   ([`crate::state::InquiryContentRecord::nothing_to_process`], read by `materialise`).
//!   A unit of work whose handle this API's own `query` member cannot carry is reported as
//!   no query, which is a limit of what can be REPORTED and not a processing failure, so
//!   it leaves the inquiry's terminal status alone.
//!
//! The create persists a resource, so it is ATTRIBUTABLE on the same terms as this crate's
//! other state changes: this handler leaves exactly one structured record naming the caller
//! (the access-key id and the credential-scope region the request presented), what the call
//! addressed and the outcome — on the refusal and replay paths as much as on the success one,
//! since a state-changing call that changed nothing is still attributable. Never the
//! inquiry's name or content, both `@sensitive` in the model.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateComplianceInquiry::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateComplianceInquiry`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json")
//! ```
//!
//! Create a new compliance inquiry.
//!
//! ## Input
//!
//! * `clientToken` (IdempotentClientToken, len 1..128, pattern ^[\w\-]+$, @idempotencyToken)
//!   — Idempotency token for the request.
//! * `inquiryContent` (InquiryContent) **required** — Content for creating a compliance
//!   inquiry - either a single query or file content.
//! * `name` (InquiryName) **required** — Title of the inquiry.
//! * `supportMode` (InquirySupportMode, one of: AI_ONLY) — Support mode for
//!   inquiry processing. Only supported for file upload mode. Defaults to AI_ONLY if not
//!   specified.
//! * `tags` (TagsMap, len ..50) — Tags to associate with the compliance inquiry resource.
//!
//! ## Output
//!
//! * `complianceInquirySummary` (InquirySummary) — Summary information about the created
//!   compliance inquiry.
//! * `tags` (TagsMap, len ..50) — Tags associated with the compliance inquiry resource.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Request to create/modify content would result in a
//!   conflict.
//! * `InternalServerException` (HTTP 500) — An unknown server exception has occurred.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — Request fails to satisfy the constraints specified by
//!   an AWS service.

#[allow(unused_imports)]
use super::types::*;
// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateComplianceInquiryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-artifact-artifact.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateComplianceInquiryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.artifact#CreateComplianceInquiry")]
#[derive(Debug)]
pub enum CreateComplianceInquiryErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_compliance_inquiry::CreateComplianceInquiryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateComplianceInquiryError = crate::scaffolded::Scaffolded<CreateComplianceInquiryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateComplianceInquiryErrorModeled> for CreateComplianceInquiryError {
    fn from(modeled: CreateComplianceInquiryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateComplianceInquiry` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateComplianceInquiry<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateComplianceInquiry<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateComplianceInquiry<Eng>
{
    type Input = CreateComplianceInquiryInput;
    type Output = CreateComplianceInquiryOutput;
    type Error = CreateComplianceInquiryError;

    const NAME: &'static str = "CreateComplianceInquiry";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_COMPLIANCE_INQUIRY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_COMPLIANCE_INQUIRY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing in the request metadata shapes the inquiry's content; the request's
        // credential scope attributes the creation below, exactly as the account-wide
        // write and the tag writes read it. The ARN the create mints carries this
        // service's own Region rather than the scope's, because that is the Region the
        // call was served in whatever the caller signed for.
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx.parts());

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // A state-changing call that changed nothing is still attributable, so the
            // failure path records its outcome rather than passing silently.
            ::tracing::info!(
                target: "emulator::audit",
                operation = "CreateComplianceInquiry",
                akid = %akid,
                region = %region,
                outcome = "unavailable",
                "state-changing operation"
            );
            return Err(CreateComplianceInquiryErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the artifact compliance inquiry store is unavailable".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();

        let content = content_record(input.inquiry_content);
        // WHICH form the submission took, which is a label this API itself reports
        // (`InquirySummary$inputSource`) and not caller content.
        let input_source = content.input_source();
        // `supportMode` "[d]efaults to AI_ONLY if not specified."
        // (`CreateComplianceInquiryRequest$supportMode`), and that is the one value the
        // modelled enum admits, so a request naming no mode is processed under the
        // documented default and a request naming any other value is refused against the
        // enum before this handler runs.
        let support_mode = input
            .support_mode
            .unwrap_or_else(|| crate::state::SUPPORT_MODE_AI_ONLY.to_string());
        let tags: ::std::collections::BTreeMap<String, String> =
            input.tags.unwrap_or_default().into_iter().collect();
        // The same reserved-namespace rule the tag write enforces
        // ([`crate::state::RESERVED_TAG_KEY_PREFIX`]): a key AWS owns may not be created
        // through this path either, so the inquiry is not minted and the request is refused
        // as a field validation failure naming the member that carried the key.
        if crate::state::reserved_tag_key(tags.keys().map(String::as_str)).is_some() {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "CreateComplianceInquiry",
                akid = %akid,
                region = %region,
                input_source,
                outcome = "refused",
                reason = "tags name a key in the reserved namespace",
                "state-changing operation"
            );
            return Err(CreateComplianceInquiryErrorModeled::ValidationException(
                crate::state::reserved_tag_keys("tags"),
            )
            .into());
        }
        let fingerprint = request_fingerprint(&input.name, &support_mode, &content, &tags);
        // The catalog each query is answered out of, copied out under the lock so the
        // retrieval below runs WITHOUT holding it: the catalog is service-owned and never
        // written, so what was copied cannot go stale.
        let corpus = crate::state::AnswerCorpus::from_catalog(&state.lock().reports);
        let queries = enumerate_queries(&input.name, &content, now, &corpus);

        let mut store = state.lock();

        // `clientToken` is this operation's `@idempotencyToken`. Checked BEFORE the
        // capacity bound below, so a retry of a call that already succeeded still
        // replays from a full store.
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = store.inquiry_create_tokens.get(token).cloned() {
                if record.fingerprint != fingerprint {
                    ::tracing::info!(
                        target: "emulator::audit",
                        operation = "CreateComplianceInquiry",
                        akid = %akid,
                        region = %region,
                        input_source,
                        outcome = "refused",
                        reason = "the clientToken was replayed on a different request",
                        "state-changing operation"
                    );
                    return Err(CreateComplianceInquiryErrorModeled::ConflictException(
                        crate::state::inquiry_token_mismatch(&record.inquiry_id),
                    )
                    .into());
                }
                if let Some(inquiry) = store.inquiries.get_mut(&record.inquiry_id) {
                    // A replay reports the inquiry as it stands now, including any
                    // transition the clock owed it since the first call.
                    inquiry.materialise(now);
                    let response = created(inquiry);
                    // The repeat is still a call on a state-changing operation, so it is
                    // attributable: it created nothing further, which is its outcome.
                    ::tracing::info!(
                        target: "emulator::audit",
                        operation = "CreateComplianceInquiry",
                        akid = %akid,
                        region = %region,
                        compliance_inquiry_id = %record.inquiry_id,
                        input_source,
                        outcome = "replayed",
                        "state-changing operation"
                    );
                    return Ok(response);
                }
            }
        }

        // Residency bound: this store holds at most `INQUIRY_CAPACITY` inquiries, so
        // repeated creates cannot grow the emulator's memory without end.
        if !store.accepts_another_inquiry() {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "CreateComplianceInquiry",
                akid = %akid,
                region = %region,
                input_source,
                outcome = "refused",
                reason = "the account holds as many compliance inquiries as it may",
                "state-changing operation"
            );
            return Err(CreateComplianceInquiryErrorModeled::ThrottlingException(
                ThrottlingException {
                    message: format!(
                        "this emulator stores at most {} compliance inquiries",
                        crate::state::INQUIRY_CAPACITY
                    ),
                    service_code: Some("artifact".to_string()),
                    quota_code: Some("ComplianceInquiriesPerAccount".to_string()),
                    retry_after_seconds: None,
                },
            )
            .into());
        }

        let id = store.mint_inquiry_id();
        let inquiry = crate::state::ComplianceInquiry {
            arn: crate::state::inquiry_arn(&id),
            // The submitted title, kept verbatim as the title of record. The name every
            // read REPORTS is the projection
            // [`crate::state::ComplianceInquiry::display_name`]: content that derives its
            // own name (a single text query) is reported under that name, and the title
            // stands only for content that derives none.
            name: input.name,
            status: crate::state::INQUIRY_STATUS_PROCESSING.to_string(),
            status_message: crate::state::INQUIRY_MESSAGE_IN_PROGRESS.to_string(),
            support_mode,
            queries,
            content,
            created_at: now,
            updated_at: now,
            // The transition the clock owes this inquiry. The span is never zero, so the
            // caller of this very call sees the non-terminal status it just returned.
            processing_due_at: Some(now + crate::state::INQUIRY_PROCESSING_SPAN),
            tags,
            // No feedback until `PutComplianceInquiryFeedback` records some.
            feedback: ::std::collections::BTreeMap::new(),
            id: id.clone(),
        };

        let response = created(&inquiry);
        store.inquiries.insert(id.clone(), inquiry);
        if let Some(token) = input.client_token {
            store.record_inquiry_create(token, &id, &fingerprint);
        }

        // Attributable: the caller's access key id (a public identifier, never a secret),
        // the scope's region, the resource created and how it was submitted. NEVER the
        // inquiry's content or name — both are `@sensitive` in the model.
        ::tracing::info!(
            target: "emulator::audit",
            operation = "CreateComplianceInquiry",
            akid = %akid,
            region = %region,
            compliance_inquiry_id = %id,
            input_source,
            outcome = "created",
            "state-changing operation"
        );

        Ok(response)
    }
}

/// The 202 response for a stored inquiry — every member read back off the record, so the
/// create and a later `GetComplianceInquiryMetadata` cannot disagree.
fn created(inquiry: &crate::state::ComplianceInquiry) -> CreateComplianceInquiryOutput {
    CreateComplianceInquiryOutput {
        compliance_inquiry_summary: Some(inquiry.to_summary()),
        // The one tags projection every inquiry operation answers with
        // ([`crate::state::ComplianceInquiry::reported_tags`]).
        tags: inquiry.reported_tags(),
    }
}

/// The submitted content, as the state persists it.
fn content_record(content: InquiryContent) -> crate::state::InquiryContentRecord {
    match content {
        InquiryContent::Query(query) => crate::state::InquiryContentRecord::Text(query),
        InquiryContent::fileContent(file) => crate::state::InquiryContentRecord::File {
            sections: file.file_sections.unwrap_or_default(),
            content: file.content,
        },
    }
}

/// The queries the inquiry owns at creation: one per UNIT OF WORK the submitted content
/// carries ([`crate::state::InquiryContentRecord::work_units`]), numbered from 1 in the
/// order they are processed ("[s]equential identifier of the query within the inquiry").
///
/// Every query starts non-terminal, like the inquiry itself, and carries the review the
/// one support mode the model admits asks for ([`crate::state::REVIEW_TYPE_AI`]); the clock
/// moves them on (see [`crate::state::ComplianceInquiry::materialise`]).
///
/// A unit whose handle cannot be reported inside the modelled bounds of
/// `QuerySummary$query` ([`crate::state::reportable_query_text`]) yields no query, rather
/// than one reported outside the shape the model declares for it — `query` is `@required`
/// and `@length(min: 1)`, so there is no query to report a unit the submission gave no
/// reportable handle, and no handle is invented for it. That is a bound on what this API
/// can REPORT, so it decides neither whether the submission was processed nor what became
/// of it (see [`crate::state::ComplianceInquiry::materialise`]).
///
/// Each query carries the ANSWER the retrieval over the published report catalog produced
/// for its text ([`crate::state::answer_from_catalog`]), which the query reports once the
/// clock has made its processing due; a query the catalog holds nothing bearing on carries
/// none.
fn enumerate_queries(
    title: &str,
    content: &crate::state::InquiryContentRecord,
    now: ::std::time::SystemTime,
    catalog: &crate::state::AnswerCorpus,
) -> Vec<crate::state::InquiryQuery> {
    let review_type = crate::state::REVIEW_TYPE_AI;
    let texts: Vec<String> = content
        .work_units(title)
        .iter()
        .filter_map(|unit| crate::state::reportable_query_text(unit))
        .collect();
    (1i32..)
        .zip(texts)
        .map(|(identifier, query)| crate::state::InquiryQuery {
            identifier,
            answer: crate::state::answer_from_catalog(&query, catalog),
            query,
            review_type: Some(review_type.to_string()),
            status: crate::state::QUERY_STATUS_PROCESSING.to_string(),
            status_message: crate::state::QUERY_MESSAGE_IN_PROGRESS.to_string(),
            created_at: now,
        })
        .collect()
}

/// A fingerprint of the request this create was made from, so a replayed `clientToken`
/// can be told from a token reused on different content.
///
/// Every member that shapes the created inquiry is absorbed into the shared
/// [`crate::state::RequestFingerprint`], which labels and length-prefixes each one so no
/// two different requests can produce the same digest by splitting their fields
/// differently. The tags are absorbed in key order (`BTreeMap`), because a map's wire
/// order is not part of the request.
fn request_fingerprint(
    name: &str,
    support_mode: &str,
    content: &crate::state::InquiryContentRecord,
    tags: &::std::collections::BTreeMap<String, String>,
) -> String {
    let mut fingerprint = crate::state::RequestFingerprint::new();
    fingerprint.absorb_str("name", name);
    fingerprint.absorb_str("supportMode", support_mode);
    match content {
        crate::state::InquiryContentRecord::Text(query) => {
            fingerprint.absorb_str("query", query);
        }
        crate::state::InquiryContentRecord::File { sections, content } => {
            for section in sections {
                fingerprint.absorb_str("fileSection", section);
            }
            fingerprint.absorb("fileContent", content);
        }
    }
    for (key, value) in tags {
        fingerprint.absorb_str("tagKey", key);
        fingerprint.absorb_str("tagValue", value);
    }
    fingerprint.finish()
}

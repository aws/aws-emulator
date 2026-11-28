// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-polly-parrot-v1` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! Beside the seams, this file owns one more piece of wording: [`errors`], the fixed
//! message Amazon Polly puts on each modelled exception. Every handler in `ops/`
//! builds its modelled failures through it, so one error code cannot acquire two
//! different messages.
//!
//! `override_refusal` is the seam this service DOES take, for one thing the framework
//! cannot know: the accepted value set a refusal states for an enumerated member is
//! the SERVICE's published set, not the order the model declares that enum in. See
//! [`validation`].
//!
//! `override_protocol` is the seam this service DOES take, for two things the generic
//! renderer cannot express: an error document whose `message` member is present and
//! `null` when the code carries no message, and an errortype header qualified by this
//! service's error namespace. See [`wire`].

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use crate::ops::start_speech_synthesis_stream::{self, StartSpeechSynthesisStream};
use service_common::event_stream::EventStreamRoute;
use service_common::request_path;

/// The accepted value set this service states when it refuses a value outside an
/// enumerated member.
pub mod validation;
pub mod wire;
/// What XML itself admits around a document element, for the operations that read one.
pub mod xml;

/// The declaration of this service's HAND-AUTHORED type modules, which the `@generated`
/// `ops/types/mod.rs` cannot carry because a model update rewrites it. Declared here
/// because this file is emitted once and never regenerated.
#[path = "ops/types_local/mod.rs"]
pub mod types_local;

/// The `com-amazonaws-polly-parrot-v1` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol: the model's own `restJson1`, with the error
    /// document of [`wire`]. Built once here because [`Service::override_protocol`]
    /// hands back a borrow of it.
    protocol: wire::NullMessageRestJson,
    /// The one event-stream operation this service serves. It is in the generated
    /// routing table too, but not in a servable form: the generated
    /// `ErasedOperation` shim of an operation whose output is a `@streaming` union
    /// declines the request (it cannot drive `serialize_as_event_stream<P>` through
    /// an object-safe `run`), so this copy is held behind an [`EventStreamRoute`]
    /// and routed by [`EmulatedService::override_operation`] — which the router
    /// consults FIRST, before the generated table. Both copies read the same
    /// lexicons and the same voice catalog, because that state lives in the shared
    /// engine rather than on the operation.
    start_speech_synthesis_stream: EventStreamRoute<StartSpeechSynthesisStream<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            protocol: wire::NullMessageRestJson::new(),
            start_speech_synthesis_stream: EventStreamRoute(StartSpeechSynthesisStream::new(
                engine,
            )),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // ONE operation is routed here: `StartSpeechSynthesisStream`, whose output
        // is a `@streaming` union and whose generated shim therefore declines the
        // request. It is matched by the method and the path the model binds it to,
        // through the operation's own const, so this table cannot drift from the
        // model; every other request still resolves through the generated table.
        if req.method() == "POST"
            && request_path(req.uri()) == start_speech_synthesis_stream::MODELED_PATH
        {
            return Some(&self.start_speech_synthesis_stream);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
        //
        // ONE thing this service states differently: the accepted values of an
        // enumerated member are stated as the SERVICE publishes that enum's set,
        // which is not the order the model happens to declare its members in (see
        // [`validation`]). The code stays `ValidationException` and the envelope
        // stays the framework's, so only the list of values inside the clause moves;
        // every refusal this service states exactly as the framework does keeps the
        // framework's own rendering.
        validation::stated_refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the TWO differences the generic
        // renderer cannot express: a modelled exception's `message` member is present
        // in the error document even when the code carries no message, as JSON `null`
        // rather than dropped, and the `x-amzn-errortype` header names the error shape
        // qualified by this service's error namespace (see `wire`). Borrowed from the
        // field built in `new`, never constructed per request.
        //
        // This seam drives the MODELLED sequence only. A framework-level refusal (a
        // `ValidationException` for a `TaskId` outside the model's pattern) still
        // renders in the generated entry's envelope, which is correct: that message is
        // built from the request and is never absent.
        Some(&self.protocol)
    }
}

/// How this service words the modelled exceptions its handlers return.
///
/// Amazon Polly's error shapes all carry one generic `message` member, and the
/// service fills it with ONE FIXED SENTENCE PER ERROR CODE: the text is a property of
/// the error, not of the request, so a lexicon name, a pagination token, a size, a
/// sample rate or a character count is never interpolated into it (and never echoed
/// back to the caller). Every handler in `ops/` builds its modelled failures through
/// the constructors below, so one error code cannot acquire two different messages.
///
/// Where a sentence is marked OBSERVED it is the text Amazon Polly's own responses
/// carry for that code. The rest are the sentence this service's Smithy model
/// documents for that error shape — equally fixed per code, and the only wording the
/// contract offers. One code carries NO message at all
/// (`SynthesisTaskNotFoundException`), which is likewise what Amazon Polly answers.
///
/// FOUR modelled errors are deliberately absent: `UnsupportedPlsAlphabetException`,
/// `EngineNotSupportedException`, `InvalidTaskIdException` and
/// `LanguageNotSupportedException`. Amazon Polly does not answer with any of them — a
/// lexicon declaring an alphabet it does not support is
/// refused as an invalid PLS document (`InvalidLexiconException`), a voice/engine
/// mismatch is refused as a request-shape failure (`ValidationException`, whose message
/// the framework builds from the request), a `TaskId` is either one the model's
/// pattern admits, in which case it names a task or names none
/// (`SynthesisTaskNotFoundException`), or one it does not, which the request boundary
/// refuses, and a `LanguageCode` the model's enum admits is SERVED whatever voice the
/// request names — the member selects the language of a bilingual voice and states no
/// requirement on the voice, so no synthesis request reaches a language the service
/// cannot speak at all. So there is nothing here to construct them with.
///
/// THREE more are absent for a reason of their own, and none of them is a fixed
/// sentence this module could hold:
///
/// * `AccessDeniedException` — this emulator evaluates no permission and holds no
///   Amazon Web Services KMS key, so no call of its is denied one;
/// * `ThrottlingException` — it rate-limits nobody, so no request of its is throttled;
/// * `ValidationException` — its message is built FROM THE REQUEST, naming the member
///   and the value that failed, so it is worded where the refusal is made
///   ([`crate::service::validation`]) rather than fixed per code here.
pub mod errors {
    #[allow(unused_imports)]
    use crate::ops::types::*;

    /// OBSERVED: `InvalidLexiconException`.
    pub const INVALID_LEXICON: &str = "Invalid PLS Lexicon";
    /// OBSERVED: `InvalidNextTokenException`.
    pub const INVALID_NEXT_TOKEN: &str = "Invalid NextToken parameter";
    /// OBSERVED: `InvalidSampleRateException`.
    pub const INVALID_SAMPLE_RATE: &str = "Invalid SampleRate parameter";
    /// OBSERVED: `InvalidSsmlException`.
    pub const INVALID_SSML: &str = "Invalid SSML request";
    /// OBSERVED: `LexiconNotFoundException`.
    pub const LEXICON_NOT_FOUND: &str = "Lexicon not found";
    /// OBSERVED: `LexiconSizeExceededException`.
    pub const LEXICON_SIZE_EXCEEDED: &str = "Maximum lexicons size has been exceeded";
    /// OBSERVED: `MarksNotSupportedForFormatException`.
    pub const MARKS_NOT_SUPPORTED_FOR_FORMAT: &str =
        "Speech marks are not supported for this format";
    /// OBSERVED: `MaxLexemeLengthExceededException`. Amazon Polly words this one about
    /// the LEXEME RULE, not about "the maximum size of the lexeme" the model's
    /// documentation prose for the shape uses: the documentation of an error shape is
    /// not the sentence the service puts on the wire for that code.
    pub const MAX_LEXEME_LENGTH_EXCEEDED: &str = "Lexeme rule length has been exceeded";
    /// OBSERVED: `MaxLexiconsNumberExceededException`.
    pub const MAX_LEXICONS_NUMBER_EXCEEDED: &str = "Max lexicon number has been exceeded";
    /// OBSERVED: `SsmlMarksNotSupportedForTextTypeException`.
    pub const SSML_MARKS_NOT_SUPPORTED_FOR_TEXT_TYPE: &str =
        "SSML Speech marks are not supported for this text type";
    /// OBSERVED: `TextLengthExceededException`.
    pub const TEXT_LENGTH_EXCEEDED: &str = "Maximum text length has been exceeded";
    /// OBSERVED: `UnsupportedPlsLanguageException`.
    pub const UNSUPPORTED_PLS_LANGUAGE: &str = "Unsupported PLS language";
    /// The model's own sentence for `ServiceFailureException`.
    pub const SERVICE_FAILURE: &str = "An unknown condition has caused a service failure.";
    /// The model's own sentence for `ConflictException`.
    pub const CONFLICT: &str = "The request conflicts with the current state of the resource. \
         For a custom voice, the voice isn't in the status that the operation requires. Check \
         the status with GetVoice, and then try again.";
    /// The model's own sentence for `InvalidTagParameterException`.
    pub const INVALID_TAG_PARAMETER: &str = "The resource ARN or a tag in the request isn't \
         valid. Specify the ARN of a supported resource in the Amazon Web Services account and \
         Region of the request. Tag keys can have a maximum length of 128 characters, and tag \
         values can have a maximum length of 256 characters. Correct the ARN or the tags, and \
         then try again.";
    /// The model's own sentence for `ResourceNotFoundException`.
    pub const RESOURCE_NOT_FOUND: &str = "Amazon Polly can't find the specified resource. \
         Verify that the resource exists in the Amazon Web Services account and Region of the \
         request and that its ARN or ID is spelled correctly, and then try again.";
    /// The model's own sentence for `ServiceQuotaExceededException`.
    pub const SERVICE_QUOTA_EXCEEDED: &str = "The request would cause a service quota to be \
         exceeded.";
    /// The model's own sentence for `TooManyTagsException`.
    pub const TOO_MANY_TAGS: &str = "The list of tags on the resource is over the limit. The \
         maximum number of tags that can be applied to a resource is 50. Remove unused tags or \
         reduce the number of tags in the request, and then try again.";

    /// The `ServiceCode` every quota failure of this service names.
    pub const SERVICE_CODE: &str = "polly";

    /// The `QuotaCode` of the quota on how many custom voices an account holds in a
    /// Region, which `CreateVoice` answers with when another voice would pass it.
    pub const CUSTOM_VOICE_MAX_COUNT_QUOTA: &str = "custom-voice-max-count";

    /// Every error code this service answers with, paired with the message it carries
    /// — `None` where Amazon Polly sends the error with no message at all.
    ///
    /// This is the table, and the constructors below are its only users, so a test can
    /// walk it and a reader can see the whole error vocabulary in one place.
    pub const CANONICAL_MESSAGES: &[(&str, Option<&str>)] = &[
        ("ConflictException", Some(CONFLICT)),
        ("InvalidLexiconException", Some(INVALID_LEXICON)),
        ("InvalidNextTokenException", Some(INVALID_NEXT_TOKEN)),
        ("InvalidSampleRateException", Some(INVALID_SAMPLE_RATE)),
        ("InvalidSsmlException", Some(INVALID_SSML)),
        ("InvalidTagParameterException", Some(INVALID_TAG_PARAMETER)),
        ("LexiconNotFoundException", Some(LEXICON_NOT_FOUND)),
        ("LexiconSizeExceededException", Some(LEXICON_SIZE_EXCEEDED)),
        (
            "MarksNotSupportedForFormatException",
            Some(MARKS_NOT_SUPPORTED_FOR_FORMAT),
        ),
        (
            "MaxLexemeLengthExceededException",
            Some(MAX_LEXEME_LENGTH_EXCEEDED),
        ),
        (
            "MaxLexiconsNumberExceededException",
            Some(MAX_LEXICONS_NUMBER_EXCEEDED),
        ),
        ("ServiceFailureException", Some(SERVICE_FAILURE)),
        ("ResourceNotFoundException", Some(RESOURCE_NOT_FOUND)),
        (
            "ServiceQuotaExceededException",
            Some(SERVICE_QUOTA_EXCEEDED),
        ),
        ("TooManyTagsException", Some(TOO_MANY_TAGS)),
        (
            "SsmlMarksNotSupportedForTextTypeException",
            Some(SSML_MARKS_NOT_SUPPORTED_FOR_TEXT_TYPE),
        ),
        // Amazon Polly answers this one with a null message body.
        ("SynthesisTaskNotFoundException", None),
        ("TextLengthExceededException", Some(TEXT_LENGTH_EXCEEDED)),
        ("UnsupportedPlsLanguageException", Some(UNSUPPORTED_PLS_LANGUAGE)),
    ];

    /// The PLS document is not a lexicon Amazon Polly can read.
    pub fn invalid_lexicon() -> InvalidLexiconException {
        InvalidLexiconException {
            message: Some(INVALID_LEXICON.to_string()),
        }
    }

    /// The `NextToken` was not one this service minted for this query.
    pub fn invalid_next_token() -> InvalidNextTokenException {
        InvalidNextTokenException {
            message: Some(INVALID_NEXT_TOKEN.to_string()),
        }
    }

    /// The `SampleRate` is not one the chosen `OutputFormat` accepts.
    pub fn invalid_sample_rate() -> InvalidSampleRateException {
        InvalidSampleRateException {
            message: Some(INVALID_SAMPLE_RATE.to_string()),
        }
    }

    /// The SSML input is not well-formed.
    pub fn invalid_ssml() -> InvalidSsmlException {
        InvalidSsmlException {
            message: Some(INVALID_SSML.to_string()),
        }
    }

    /// No lexicon is stored under the requested name.
    pub fn lexicon_not_found() -> LexiconNotFoundException {
        LexiconNotFoundException {
            message: Some(LEXICON_NOT_FOUND.to_string()),
        }
    }

    /// The lexicon is larger than the published per-lexicon size quota.
    pub fn lexicon_size_exceeded() -> LexiconSizeExceededException {
        LexiconSizeExceededException {
            message: Some(LEXICON_SIZE_EXCEEDED.to_string()),
        }
    }

    /// Speech marks were asked for with an audio `OutputFormat`.
    pub fn marks_not_supported_for_format() -> MarksNotSupportedForFormatException {
        MarksNotSupportedForFormatException {
            message: Some(MARKS_NOT_SUPPORTED_FOR_FORMAT.to_string()),
        }
    }

    /// A lexeme in the document is longer than the published lexeme quota.
    pub fn max_lexeme_length_exceeded() -> MaxLexemeLengthExceededException {
        MaxLexemeLengthExceededException {
            message: Some(MAX_LEXEME_LENGTH_EXCEEDED.to_string()),
        }
    }

    /// Storing another lexicon would pass the published per-Region count quota.
    pub fn max_lexicons_number_exceeded() -> MaxLexiconsNumberExceededException {
        MaxLexiconsNumberExceededException {
            message: Some(MAX_LEXICONS_NUMBER_EXCEEDED.to_string()),
        }
    }

    /// This service's own state could not be reached (an HTTP 500 server fault).
    pub fn service_failure() -> ServiceFailureException {
        ServiceFailureException {
            message: Some(SERVICE_FAILURE.to_string()),
        }
    }

    /// SSML speech marks were asked for over plain-text input.
    pub fn ssml_marks_not_supported_for_text_type() -> SsmlMarksNotSupportedForTextTypeException {
        SsmlMarksNotSupportedForTextTypeException {
            message: Some(SSML_MARKS_NOT_SUPPORTED_FOR_TEXT_TYPE.to_string()),
        }
    }

    /// No synthesis task is stored under the requested `TaskId`.
    pub fn synthesis_task_not_found() -> SynthesisTaskNotFoundException {
        // No message: Amazon Polly answers this error with a null message body, so
        // inventing one here would be a difference a client can see.
        SynthesisTaskNotFoundException { message: None }
    }

    /// The text is longer than the published synthesis limits.
    pub fn text_length_exceeded() -> TextLengthExceededException {
        TextLengthExceededException {
            message: Some(TEXT_LENGTH_EXCEEDED.to_string()),
        }
    }

    /// The lexicon declares a language Amazon Polly does not support.
    pub fn unsupported_pls_language() -> UnsupportedPlsLanguageException {
        UnsupportedPlsLanguageException {
            message: Some(UNSUPPORTED_PLS_LANGUAGE.to_string()),
        }
    }

    /// The resource is not in the state the operation needs — a custom voice the caller
    /// asked to delete while Amazon Polly is still creating it.
    ///
    /// The message is the code's own fixed sentence; `resource_id` and `resource_type`
    /// are the two REQUIRED members the shape carries beside it, and they name the
    /// resource the request conflicted with, so they come from the request and the
    /// store rather than from this sentence.
    pub fn conflict(resource_id: &str, resource_type: &str) -> ConflictException {
        ConflictException {
            message: CONFLICT.to_string(),
            resource_id: resource_id.to_string(),
            resource_type: resource_type.to_string(),
        }
    }

    /// The ARN or a tag in the request is not one this service accepts.
    pub fn invalid_tag_parameter() -> InvalidTagParameterException {
        InvalidTagParameterException {
            message: Some(INVALID_TAG_PARAMETER.to_string()),
        }
    }

    /// No resource of this account and Region answers to the identifier the request
    /// named. The two required members name what was looked for, as for
    /// [`conflict`].
    pub fn resource_not_found(resource_id: &str, resource_type: &str) -> ResourceNotFoundException {
        ResourceNotFoundException {
            message: RESOURCE_NOT_FOUND.to_string(),
            resource_id: resource_id.to_string(),
            resource_type: resource_type.to_string(),
        }
    }

    /// The request would pass one of this service's quotas; `quota_code` names which,
    /// as a value of the model's `QuotaCode` enum.
    pub fn service_quota_exceeded(quota_code: &str) -> ServiceQuotaExceededException {
        ServiceQuotaExceededException {
            message: SERVICE_QUOTA_EXCEEDED.to_string(),
            quota_code: quota_code.to_string(),
            service_code: SERVICE_CODE.to_string(),
        }
    }

    /// The request would leave a resource holding more tags than one may carry.
    pub fn too_many_tags() -> TooManyTagsException {
        TooManyTagsException {
            message: Some(TOO_MANY_TAGS.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every constructor must carry exactly the table's entry for its code: the
        /// message is a function of the code and of nothing else.
        #[test]
        fn every_constructor_carries_its_table_entry() {
            let built: Vec<(&str, Option<String>)> = vec![
                ("ConflictException", Some(conflict("vc-1", "voice").message)),
                ("InvalidLexiconException", invalid_lexicon().message),
                ("InvalidNextTokenException", invalid_next_token().message),
                ("InvalidSampleRateException", invalid_sample_rate().message),
                ("InvalidSsmlException", invalid_ssml().message),
                (
                    "InvalidTagParameterException",
                    invalid_tag_parameter().message,
                ),
                ("LexiconNotFoundException", lexicon_not_found().message),
                (
                    "LexiconSizeExceededException",
                    lexicon_size_exceeded().message,
                ),
                (
                    "MarksNotSupportedForFormatException",
                    marks_not_supported_for_format().message,
                ),
                (
                    "MaxLexemeLengthExceededException",
                    max_lexeme_length_exceeded().message,
                ),
                (
                    "MaxLexiconsNumberExceededException",
                    max_lexicons_number_exceeded().message,
                ),
                ("ServiceFailureException", service_failure().message),
                (
                    "ResourceNotFoundException",
                    Some(resource_not_found("Colors", "lexicon").message),
                ),
                (
                    "ServiceQuotaExceededException",
                    Some(service_quota_exceeded(CUSTOM_VOICE_MAX_COUNT_QUOTA).message),
                ),
                ("TooManyTagsException", too_many_tags().message),
                (
                    "SsmlMarksNotSupportedForTextTypeException",
                    ssml_marks_not_supported_for_text_type().message,
                ),
                (
                    "SynthesisTaskNotFoundException",
                    synthesis_task_not_found().message,
                ),
                ("TextLengthExceededException", text_length_exceeded().message),
                (
                    "UnsupportedPlsLanguageException",
                    unsupported_pls_language().message,
                ),
            ];
            assert_eq!(
                built.len(),
                CANONICAL_MESSAGES.len(),
                "every code in the table needs a constructor and the reverse"
            );
            for (code, message) in built {
                let expected = CANONICAL_MESSAGES
                    .iter()
                    .find(|(table_code, _)| *table_code == code)
                    .map(|(_, message)| message.map(str::to_string))
                    .unwrap_or_else(|| panic!("code {code:?} is not in CANONICAL_MESSAGES"));
                assert_eq!(
                    message, expected,
                    "code {code:?}: the constructed message must be the table's"
                );
            }
        }

        /// A fixed message says nothing about the request, so none of them may carry
        /// a placeholder or an interpolation marker.
        #[test]
        fn no_canonical_message_is_request_shaped() {
            for (code, message) in CANONICAL_MESSAGES {
                let Some(message) = message else { continue };
                assert!(
                    !message.is_empty(),
                    "code {code:?}: the message must not be empty"
                );
                for marker in ['{', '}', ':'] {
                    assert!(
                        !message.contains(marker),
                        "code {code:?}: message {message:?} looks built from the request \
                         (it contains {marker:?})"
                    );
                }
            }
        }
    }
}

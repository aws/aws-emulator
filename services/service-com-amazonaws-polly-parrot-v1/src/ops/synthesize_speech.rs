// SPDX-License-Identifier: CC0-1.0

//! @op SynthesizeSpeech
//! Serves one synthesis request: resolves the lexicons it names, settles the output
//! encoding and returns an audio stream in it.
//!
//! ## How the speech is produced
//!
//! The `AudioStream` is RENDERED from the text, in this process. The text is read into
//! phonemes by English spelling rules ([`phonemes`]), the lexicons the request names and
//! the SSML it carries are applied to that reading ([`plan`]), and the resulting timed
//! sequence of phonemes is produced by a formant synthesiser — a glottal source and the
//! resonances of the vocal tract ([`render`]) — then written in the encoding
//! `OutputFormat` asked for ([`audio`]). Nothing is echoed back, no bytes are matched
//! against the input and no stored audio exists: the waveform follows from what the
//! request says, so changing the text, a lexicon, the markup or the voice changes the
//! samples.
//!
//! Everything AROUND the synthesis is computed from the request and from state the same
//! way:
//!
//! * every name in `LexiconNames` is resolved in the lexicon store of the Region the
//!   request was signed for, and a name that Region does not store is
//!   `LexiconNotFoundException` (404);
//! * `Engine` is checked against the voice the request names, as the voice catalog
//!   `DescribeVoices` serves records it, so the two operations cannot disagree: an
//!   engine the voice does not support is the request-shape refusal
//!   `ValidationException` (which is what Amazon Polly answers for it, not the modelled
//!   `EngineNotSupportedException`). A voice the seeded catalog does not carry is not
//!   narrowed, because that catalog is a stated subset. `LanguageCode` is NOT such a
//!   check: it selects which language a bilingual voice speaks and states no
//!   requirement on the voice, so a language the chosen voice does not speak is
//!   synthesized with that voice in its own language rather than refused;
//! * `ContentType` is the `OutputFormat`'s documented media type;
//! * `ssml` input is read as an XML DOCUMENT — exactly one root element, and nothing
//!   but whitespace, comments and processing instructions around it — and checked
//!   against the SSML vocabulary Amazon Polly publishes — its
//!   tag set, each tag's attributes and each attribute's value domain (see
//!   [`ssml::SUPPORTED_ELEMENTS`]) — so a document that is not well-formed, or markup
//!   outside that vocabulary, is `InvalidSsmlException` rather than a green light for a
//!   document the real service refuses;
//! * `RequestCharacters` is the billed character count — every character of plain
//!   text, and for `ssml` the text content only, because "SSML tags are not counted
//!   as billed characters";
//! * `SampleRate` is a string whose domain the model enumerates per format, so it is
//!   accepted only when it is one of those spellings exactly, and defaults per format
//!   and engine the same way;
//! * the text limits (6000 total / 3000 billed characters) are enforced;
//! * `json` output is the speech-mark stream of the same utterance, in all four modelled
//!   types: `sentence` and `word` carry the byte offsets and the text they cover in
//!   `Text`, `viseme` carries the mouth shape of each phoneme as it is spoken and `ssml`
//!   carries each `<mark>` the document placed, each at the time the renderer produces
//!   it; a request naming no mark type gets an empty stream, for every `TextType`,
//!   rather than a refusal Amazon Polly does not declare.
//!
//! Two formats are not rendered: `mp3` needs an MPEG Layer III encoder, whose only
//! published Rust binding is under a licence this project cannot take, and `ogg_opus`
//! needs an Opus encoder, which no binding available to this image can build. Each
//! carries its own real framing around silence, lasting as long as the speech does — see
//! [`legacy_audio`], where that is stated rather than dressed up.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `SynthesizeSpeech::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `SynthesizeSpeech`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json")
//! ```
//!
//! Synthesizes UTF-8 input, plain text or SSML, to a stream of bytes. SSML input must be
//! valid, well-formed SSML. Some alphabets might not be available with all the voices (for
//! example, Cyrillic might not be read at all by English voices) unless phoneme mapping is
//! used. For more information, see How it Works.
//!
//! ## Input
//!
//! * `Engine` (Engine, one of: generative | long-form | neural | standard) — Specifies the
//!   engine (standard, neural, long-form, or generative) for Amazon Polly to use when
//!   processing input text for speech synthesis. Provide an engine that is supported by the
//!   voice you select. If you don't provide an engine, the standard engine is selected by
//!   default. If a chosen voice isn't supported by the standard engine, this will result in
//!   an error. For information on Amazon Polly voices and which voices are available for each
//!   engine, see Available Voices.
//! * `LanguageCode` (LanguageCode, one of: ar-AE | arb | ca-ES | cmn-CN | cs-CZ | cy-GB |
//!   da-DK | de-AT | de-CH | de-DE | en-AU | en-GB | en-GB-WLS | en-IE | en-IN | en-NZ |
//!   en-SG | en-US | en-ZA | es-ES | es-MX | es-US | fi-FI | fr-BE | fr-CA | fr-FR | hi-IN |
//!   is-IS | it-IT | …) — Optional language code for the Synthesize Speech request. This is
//!   only necessary if using a bilingual voice, such as Aditi, which can be used for either
//!   Indian English (en-IN) or Hindi (hi-IN). If a bilingual voice is used and no language
//!   code is specified, Amazon Polly uses the default language of the bilingual voice. The
//!   default language for any voice is the one returned by the DescribeVoices operation for
//!   the LanguageCode parameter. For example, if no language code is specified, Aditi will
//!   use Indian English rather than Hindi.
//! * `LexiconNames` (LexiconNameList, len 0..5) — List of one or more pronunciation lexicon
//!   names you want the service to apply during synthesis. Lexicons are applied only if the
//!   language of the lexicon is the same as the language of the voice. For information about
//!   storing lexicons, see PutLexicon.
//! * `OutputFormat` (OutputFormat, one of: alaw | json | mp3 | mulaw | ogg_opus | ogg_vorbis
//!   | pcm) **required** — The format in which the returned output will be encoded. For audio
//!   stream, this will be mp3, ogg_vorbis, ogg_opus, mu-law, a-law or pcm. For speech marks,
//!   this will be json. When pcm is used, the content returned is audio/pcm in a signed
//!   16-bit, 1 channel (mono), little-endian format.
//! * `SampleRate` (SampleRate) — The audio frequency specified in Hz. The valid values for
//!   mp3 and ogg_vorbis are "8000", "16000", "22050", "24000", "44100" and "48000". The
//!   default value for standard voices is "22050". The default value for neural voices is
//!   "24000". The default value for long-form voices is "24000". The default value for
//!   generative voices is "24000". Valid values for pcm are "8000" and "16000" The default
//!   value is "16000". Valid value for ogg_opus is "48000". Valid value for mu-law and a-law
//!   is "8000".
//! * `SpeechMarkTypes` (SpeechMarkTypeList, len 0..4) — The type of speech marks returned for
//!   the input text.
//! * `Text` (Text) **required** — Input text to synthesize. If you specify ssml as the
//!   TextType, follow the SSML format for the input text.
//! * `TextType` (TextType, one of: ssml | text) — Specifies whether the input text is plain
//!   text or SSML. The default value is plain text. For more information, see Using SSML.
//! * `VoiceId` (VoiceId, one of: Aditi | Adriano | Ambre | Amy | Andres | Aria | Arjun |
//!   Arlet | Arthur | Astrid | Ayanda | Beatrice | Bianca | Brian | Burcu | Camila | Carla |
//!   Carlos | Carmen | Carolina | Celine | Chantal | Conchita | Cristiano | Curtis | Daniel |
//!   …) **required** — Voice ID to use for the synthesis. You can get a list of available
//!   voice IDs by calling the DescribeVoices operation.
//!
//! ## Output
//!
//! * `AudioStream` (AudioStream, default "") **required** — Stream containing the synthesized
//!   speech.
//! * `ContentType` (ContentType) — Specifies the type audio stream. This should reflect the
//!   OutputFormat parameter in your request. If you request mp3 as the OutputFormat, the
//!   ContentType returned is audio/mpeg. If you request ogg_vorbis as the OutputFormat, the
//!   ContentType returned is audio/ogg. If you request ogg_opus as the OutputFormat, the
//!   ContentType returned is audio/ogg. If you request pcm as the OutputFormat, the
//!   ContentType returned is audio/pcm in a signed 16-bit, 1 channel (mono), little-endian
//!   format. If you request mu-law as the OutputFormat, the ContentType returned is
//!   audio/mulaw. If you request a-law as the OutputFormat, the ContentType returned is
//!   audio/alaw. If you request json as the OutputFormat, the …
//! * `RequestCharacters` (RequestCharacters, default 0) **required** — Number of characters
//!   synthesized.
//!
//! ## Errors
//!
//! * `EngineNotSupportedException` (HTTP 400) — This engine is not compatible with the voice
//!   that you have designated. Choose a new voice that is compatible with the engine or
//!   change the engine and restart the operation.
//! * `InvalidSampleRateException` (HTTP 400) — The specified sample rate is not valid.
//! * `InvalidSsmlException` (HTTP 400) — The SSML you provided is invalid. Verify the SSML
//!   syntax, spelling of tags and values, and then try again.
//! * `LanguageNotSupportedException` (HTTP 400) — The language specified is not currently
//!   supported by Amazon Polly in this capacity.
//! * `LexiconNotFoundException` (HTTP 404) — Amazon Polly can't find the specified lexicon.
//!   This could be caused by a lexicon that is missing, its name is misspelled or specifying
//!   a lexicon that is in a different region. Verify that the lexicon exists, is in the
//!   region (see ListLexicons) and that you spelled its name is spelled correctly. Then try
//!   again.
//! * `MarksNotSupportedForFormatException` (HTTP 400) — Speech marks are not supported for
//!   the OutputFormat selected. Speech marks are only available for content in json format.
//! * `ServiceFailureException` (HTTP 500) — An unknown condition has caused a service
//!   failure.
//! * `SsmlMarksNotSupportedForTextTypeException` (HTTP 400) — SSML speech marks are not
//!   supported for plain text-type input.
//! * `TextLengthExceededException` (HTTP 400) — The value of the "Text" parameter is longer
//!   than the accepted limits. For the SynthesizeSpeech API, the limit for input text is a
//!   maximum of 6000 characters total, of which no more than 3000 can be billed characters.
//!   For the StartSpeechSynthesisTask API, the maximum is 200,000 characters, of which no
//!   more than 100,000 can be billed characters. SSML tags are not counted as billed
//!   characters.

#[allow(unused_imports)]
use super::types::*;

// The synthesiser's parts. Each declaration carries an explicit `#[path]`: `ops/mod.rs`
// reaches this file through `include!`, so a bare `mod x;` here resolves against
// `src/ops/` rather than against this file's own directory — the same reason the
// generated declaration of this module carries one.
#[path = "synthesize_speech/audio.rs"]
pub mod audio;
#[path = "synthesize_speech/lexicon.rs"]
pub mod lexicon;
#[path = "synthesize_speech/phonemes.rs"]
pub mod phonemes;
#[path = "synthesize_speech/plan.rs"]
pub mod plan;
#[path = "synthesize_speech/render.rs"]
pub mod render;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SynthesizeSpeechError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-polly-parrot-v1.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SynthesizeSpeechError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.polly#SynthesizeSpeech")]
#[derive(Debug)]
pub enum SynthesizeSpeechErrorModeled {
    // The two errors the model added for CUSTOM voices, declared because the operation
    // lists them and constructed by nothing here: `VoiceId` targets the model's closed
    // `VoiceId` enum, so the `vc-` id `CreateVoice` mints is refused by the request
    // boundary before this handler runs (`crate::service::validation`), and no
    // synthesis request of this emulator can name a custom voice that is missing or
    // not yet active.
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    EngineNotSupportedException(EngineNotSupportedException),
    InvalidSampleRateException(InvalidSampleRateException),
    InvalidSsmlException(InvalidSsmlException),
    LanguageNotSupportedException(LanguageNotSupportedException),
    LexiconNotFoundException(LexiconNotFoundException),
    MarksNotSupportedForFormatException(MarksNotSupportedForFormatException),
    ServiceFailureException(ServiceFailureException),
    SsmlMarksNotSupportedForTextTypeException(SsmlMarksNotSupportedForTextTypeException),
    TextLengthExceededException(TextLengthExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::synthesize_speech::SynthesizeSpeechError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SynthesizeSpeechError = crate::scaffolded::Scaffolded<SynthesizeSpeechErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SynthesizeSpeechErrorModeled> for SynthesizeSpeechError {
    fn from(modeled: SynthesizeSpeechErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

// ---------------------------------------------------------------------------
// The request's documented limits and derived values
// ---------------------------------------------------------------------------

/// The total characters `Text` may carry.
///
/// From this operation's `TextLengthExceededException` documentation: "For the
/// SynthesizeSpeech API, the limit for input text is a maximum of 6000 characters
/// total, of which no more than 3000 can be billed characters."
pub const MAX_TOTAL_CHARACTERS: usize = 6000;

/// The billed characters `Text` may carry — the same documented sentence.
pub const MAX_BILLED_CHARACTERS: usize = 3000;

/// The media type the response carries for `output_format`.
///
/// The mapping is `SynthesizeSpeechOutput$ContentType`'s own documentation: mp3 →
/// audio/mpeg, ogg_vorbis → audio/ogg, ogg_opus → audio/ogg, pcm → audio/pcm, mu-law
/// → audio/mulaw, a-law → audio/alaw, json → application/x-json-stream.
pub fn content_type_for(output_format: &str) -> Option<&'static str> {
    Some(match output_format {
        "mp3" => "audio/mpeg",
        "ogg_vorbis" | "ogg_opus" => "audio/ogg",
        "pcm" => "audio/pcm",
        "mulaw" => "audio/mulaw",
        "alaw" => "audio/alaw",
        "json" => "application/x-json-stream",
        _ => return None,
    })
}

/// The `SampleRate` values valid for `output_format`, as the STRINGS the
/// documentation enumerates.
///
/// Verbatim from `SynthesizeSpeechInput$SampleRate`'s documentation: "The valid
/// values for mp3 and ogg_vorbis are \"8000\", \"16000\", \"22050\", \"24000\",
/// \"44100\" and \"48000\" … Valid values for pcm are \"8000\" and \"16000\" … Valid
/// value for ogg_opus is \"48000\" … Valid value for mu-law and a-law is \"8000\"."
/// `json` carries speech marks rather than audio, so no sample rate applies to it.
///
/// `SampleRate` is a string member whose domain the documentation gives as these
/// quoted spellings, so the set is kept as strings: membership is a comparison of the
/// caller's characters with them, not of a number parsed out of them.
pub fn valid_sample_rates(output_format: &str) -> &'static [&'static str] {
    match output_format {
        "mp3" | "ogg_vorbis" => &["8000", "16000", "22050", "24000", "44100", "48000"],
        "pcm" => &["8000", "16000"],
        "ogg_opus" => &["48000"],
        "mulaw" | "alaw" => &["8000"],
        _ => &[],
    }
}

/// The frequency in Hz that `requested` names for `output_format`, or `None` when that
/// format's documented value set does not contain that exact spelling.
///
/// The caller's string has to BE one of the enumerated values — a spelling that merely
/// denotes the same number (a leading sign, leading zeros, surrounding space) is not in
/// the set and is therefore refused. The Hz figure is then read off the matched
/// enumerated value, so it can only ever be one of the documented frequencies.
pub fn resolve_sample_rate(output_format: &str, requested: &str) -> Option<u32> {
    valid_sample_rates(output_format)
        .iter()
        .find(|valid| **valid == requested)
        .and_then(|valid| valid.parse::<u32>().ok())
}

/// The sample rate used when the request names none.
///
/// Also from `SynthesizeSpeechInput$SampleRate`: for mp3 and ogg_vorbis "The default
/// value for standard voices is 22050. The default value for neural voices is 24000.
/// The default value for long-form voices is 24000. The default value for generative
/// voices is 24000"; for pcm "The default value is 16000"; ogg_opus and the companded
/// formats document a single valid value, which is therefore also the default.
pub fn default_sample_rate(output_format: &str, engine: &str) -> u32 {
    match output_format {
        "pcm" => 16000,
        "ogg_opus" => 48000,
        "mulaw" | "alaw" => 8000,
        // mp3 and ogg_vorbis: the default follows the engine.
        _ if engine == "standard" => 22050,
        _ => 24000,
    }
}

/// The engine a request without an explicit `Engine` runs on.
///
/// `SynthesizeSpeechInput$Engine`: "If you don't provide an engine, the standard
/// engine is selected by default."
pub const DEFAULT_ENGINE: &str = "standard";

/// The `TextType` a request without an explicit one carries.
///
/// `SynthesizeSpeechInput$TextType`: "The default value is plain text."
pub const DEFAULT_TEXT_TYPE: &str = "text";

/// The SSML vocabulary Amazon Polly accepts: its elements, their attributes and the
/// value domain of each attribute.
///
/// Amazon Polly does not accept SSML in general — it accepts the finite tag set its
/// supported-SSML-tags documentation publishes, with the attributes that
/// documentation gives each tag and the values it gives each attribute. Anything
/// outside that vocabulary is `InvalidSsmlException`, whose own documentation asks the
/// caller to "verify the SSML syntax, spelling of tags and values": a tag or a value
/// outside the published set is as much a refusal as malformed XML.
///
/// Everything here is a transcription of that documentation. An element's row names
/// the tag as the documentation spells it (`amazon:effect` keeps its prefix, because
/// that is the tag's published name) and lists only the attributes documented for it;
/// the [`Domain`] on each attribute is the value set documented for that attribute.
/// Nothing in this table is derived from any request the emulator has seen.
pub mod ssml {
    /// What an attribute accepts, as the tag's documentation states it.
    #[derive(Debug, Clone, Copy)]
    pub enum Domain {
        /// Exactly one of a published list of keywords.
        Keywords(&'static [&'static str]),
        /// A keyword, or a relative loudness in decibels (`+6dB`, `-6dB`).
        KeywordsOrDecibels(&'static [&'static str]),
        /// A keyword, or a percentage: an UNSIGNED percentage is an absolute setting
        /// and is bounded below by `min_percent`, while a SIGNED one is a change
        /// relative to the current value and so carries no such floor.
        KeywordsOrPercent {
            /// The keywords the attribute also accepts.
            keywords: &'static [&'static str],
            /// The smallest percentage the documentation allows.
            min_percent: f64,
        },
        /// A duration in seconds or milliseconds (`2s`, `500ms`), at most `max_ms`
        /// where the documentation publishes a cap.
        Duration {
            /// The documented maximum, in milliseconds.
            max_ms: Option<f64>,
        },
        /// A language tag, as `xml:lang` carries one.
        LanguageTag,
        /// A free value the service passes through — a mark name, an alias, a phoneme
        /// string, an audio source. The documentation publishes no value set for it,
        /// so none is invented here.
        Text,
    }

    impl Domain {
        /// Whether `value` is inside this domain.
        pub fn accepts(&self, value: &str) -> bool {
            match self {
                Self::Keywords(keywords) => keywords.contains(&value),
                Self::KeywordsOrDecibels(keywords) => {
                    keywords.contains(&value) || decibels(value).is_some()
                }
                Self::KeywordsOrPercent {
                    keywords,
                    min_percent,
                } => {
                    keywords.contains(&value)
                        || percentage(value)
                            .is_some_and(|(signed, magnitude)| signed || magnitude >= *min_percent)
                }
                Self::Duration { max_ms } => duration_ms(value)
                    .is_some_and(|ms| max_ms.is_none_or(|max| ms <= max)),
                Self::LanguageTag => {
                    !value.is_empty()
                        && value.split('-').all(|subtag| {
                            !subtag.is_empty() && subtag.chars().all(|c| c.is_ascii_alphanumeric())
                        })
                }
                Self::Text => true,
            }
        }
    }

    /// A non-negative decimal number, as SSML spells one. No sign, no exponent.
    fn decimal(text: &str) -> Option<f64> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        text.parse::<f64>().ok().filter(|n| n.is_finite())
    }

    /// A duration in milliseconds, from a value spelled in seconds or milliseconds.
    pub fn duration_ms(value: &str) -> Option<f64> {
        if let Some(number) = value.strip_suffix("ms") {
            decimal(number)
        } else {
            decimal(value.strip_suffix('s')?).map(|seconds| seconds * 1000.0)
        }
    }

    /// A percentage, as `(whether it carried a sign, its magnitude)`. A signed
    /// percentage is a change relative to the current value; an unsigned one is an
    /// absolute setting.
    pub fn percentage(value: &str) -> Option<(bool, f64)> {
        let body = value.strip_suffix('%')?;
        match body.strip_prefix(['+', '-']) {
            Some(magnitude) => decimal(magnitude).map(|n| (true, n)),
            None => decimal(body).map(|n| (false, n)),
        }
    }

    /// A loudness in decibels, as `volume` spells one.
    pub fn decibels(value: &str) -> Option<f64> {
        let body = value.strip_suffix("dB")?;
        decimal(body.strip_prefix(['+', '-']).unwrap_or(body))
    }

    /// One element of the vocabulary.
    #[derive(Debug, Clone, Copy)]
    pub struct Element {
        /// The tag's published name, prefix included.
        pub name: &'static str,
        /// The attributes the tag documents, each with its value domain.
        pub attributes: &'static [(&'static str, Domain)],
    }

    /// `<break strength>`.
    const BREAK_STRENGTH: &[&str] = &["none", "x-weak", "weak", "medium", "strong", "x-strong"];
    /// `<emphasis level>`.
    const EMPHASIS_LEVEL: &[&str] = &["strong", "moderate", "reduced"];
    /// `<phoneme alphabet>`.
    const PHONEME_ALPHABET: &[&str] = &["ipa", "x-sampa"];
    /// `<prosody volume>`, beside a decibel offset.
    const PROSODY_VOLUME: &[&str] = &[
        "default", "silent", "x-soft", "soft", "medium", "loud", "x-loud",
    ];
    /// `<prosody rate>`, beside a percentage.
    const PROSODY_RATE: &[&str] = &["x-slow", "slow", "medium", "fast", "x-fast"];
    /// `<prosody pitch>`, beside a percentage.
    const PROSODY_PITCH: &[&str] = &["default", "x-low", "low", "medium", "high", "x-high"];
    /// `<say-as interpret-as>`.
    const SAY_AS_INTERPRET_AS: &[&str] = &[
        "characters",
        "spell-out",
        "cardinal",
        "number",
        "ordinal",
        "digits",
        "fraction",
        "unit",
        "date",
        "time",
        "address",
        "expletive",
        "telephone",
    ];
    /// `<say-as format>`: the date formats, plus the two clock formats.
    const SAY_AS_FORMAT: &[&str] = &[
        "mdy", "dmy", "ymd", "md", "dm", "ym", "my", "y", "m", "d", "yyyymmdd", "hms12", "hms24",
    ];
    /// `<w role>`.
    const W_ROLE: &[&str] = &["amazon:VB", "amazon:VBD", "amazon:NN", "amazon:SENSE_1"];
    /// `<amazon:effect name>`.
    const EFFECT_NAME: &[&str] = &["whispered", "drc"];
    /// `<amazon:effect phonation>`.
    const EFFECT_PHONATION: &[&str] = &["soft"];
    /// `<amazon:domain name>`.
    const DOMAIN_NAME: &[&str] = &["news", "conversational"];
    /// `<amazon:breath duration>` and `<amazon:auto-breaths duration>`.
    const BREATH_DURATION: &[&str] = &["default", "x-short", "short", "medium", "long", "x-long"];
    /// `<amazon:breath volume>` and `<amazon:auto-breaths volume>`.
    const BREATH_VOLUME: &[&str] = &["default", "x-soft", "soft", "medium", "loud", "x-loud"];
    /// `<amazon:auto-breaths frequency>`.
    const BREATH_FREQUENCY: &[&str] = &["default", "x-low", "low", "medium", "high", "x-high"];

    /// The longest pause `<break time>` may ask for: the documentation caps it at ten
    /// seconds.
    pub const MAX_BREAK_MS: f64 = 10_000.0;

    /// The smallest `<prosody rate>` percentage the documentation allows.
    pub const MIN_RATE_PERCENT: f64 = 20.0;

    /// Every element Amazon Polly's supported-SSML-tags documentation publishes, with
    /// that documentation's attributes and value domains.
    pub const SUPPORTED_ELEMENTS: &[Element] = &[
        Element {
            // The document element. `version` and `xml:lang` are SSML's own
            // declarations on it; namespace declarations are XML's and are not
            // attributes of any tag.
            name: "speak",
            attributes: &[("version", Domain::Text), ("xml:lang", Domain::LanguageTag)],
        },
        Element {
            name: "break",
            attributes: &[
                ("strength", Domain::Keywords(BREAK_STRENGTH)),
                (
                    "time",
                    Domain::Duration {
                        max_ms: Some(MAX_BREAK_MS),
                    },
                ),
            ],
        },
        Element {
            name: "emphasis",
            attributes: &[("level", Domain::Keywords(EMPHASIS_LEVEL))],
        },
        Element {
            name: "lang",
            attributes: &[("xml:lang", Domain::LanguageTag)],
        },
        Element {
            name: "mark",
            attributes: &[("name", Domain::Text)],
        },
        Element {
            name: "p",
            attributes: &[],
        },
        Element {
            name: "phoneme",
            attributes: &[
                ("alphabet", Domain::Keywords(PHONEME_ALPHABET)),
                ("ph", Domain::Text),
            ],
        },
        Element {
            name: "prosody",
            attributes: &[
                ("volume", Domain::KeywordsOrDecibels(PROSODY_VOLUME)),
                (
                    "rate",
                    Domain::KeywordsOrPercent {
                        keywords: PROSODY_RATE,
                        min_percent: MIN_RATE_PERCENT,
                    },
                ),
                (
                    "pitch",
                    Domain::KeywordsOrPercent {
                        keywords: PROSODY_PITCH,
                        min_percent: 0.0,
                    },
                ),
                // The maximum-duration attribute, which the documentation spells with
                // the vendor prefix and publishes no cap for.
                ("amazon:max-duration", Domain::Duration { max_ms: None }),
            ],
        },
        Element {
            name: "s",
            attributes: &[],
        },
        Element {
            name: "say-as",
            attributes: &[
                ("interpret-as", Domain::Keywords(SAY_AS_INTERPRET_AS)),
                ("format", Domain::Keywords(SAY_AS_FORMAT)),
            ],
        },
        Element {
            name: "sub",
            attributes: &[("alias", Domain::Text)],
        },
        Element {
            name: "w",
            attributes: &[("role", Domain::Keywords(W_ROLE))],
        },
        Element {
            name: "audio",
            attributes: &[("src", Domain::Text)],
        },
        Element {
            name: "amazon:effect",
            attributes: &[
                ("name", Domain::Keywords(EFFECT_NAME)),
                ("phonation", Domain::Keywords(EFFECT_PHONATION)),
                (
                    "vocal-tract-length",
                    Domain::KeywordsOrPercent {
                        keywords: &[],
                        min_percent: 0.0,
                    },
                ),
            ],
        },
        Element {
            name: "amazon:domain",
            attributes: &[("name", Domain::Keywords(DOMAIN_NAME))],
        },
        Element {
            name: "amazon:breath",
            attributes: &[
                ("duration", Domain::Keywords(BREATH_DURATION)),
                ("volume", Domain::Keywords(BREATH_VOLUME)),
            ],
        },
        Element {
            name: "amazon:auto-breaths",
            attributes: &[
                ("duration", Domain::Keywords(BREATH_DURATION)),
                ("volume", Domain::Keywords(BREATH_VOLUME)),
                ("frequency", Domain::Keywords(BREATH_FREQUENCY)),
            ],
        },
    ];

    /// The name this element is spelled with, as the vocabulary names one.
    ///
    /// A vendor-prefixed tag (`amazon:effect`) is published WITH its prefix, so the
    /// prefix is part of its name; every other tag is matched on its local name, so a
    /// document that binds SSML to a prefix of its own still reads as SSML.
    pub fn element_name(element: &quick_xml::events::BytesStart<'_>) -> String {
        let qualified = String::from_utf8_lossy(element.name().as_ref()).to_string();
        if qualified.starts_with("amazon:") {
            qualified
        } else {
            String::from_utf8_lossy(element.local_name().as_ref()).to_string()
        }
    }

    /// Check one element — its tag and every attribute it carries — against the
    /// vocabulary.
    ///
    /// `Err` carries why the document is outside it, which the caller answers with
    /// `InvalidSsmlException`.
    pub fn check_element(
        name: &str,
        element: &quick_xml::events::BytesStart<'_>,
    ) -> Result<(), String> {
        let Some(declared) = SUPPORTED_ELEMENTS.iter().find(|known| known.name == name) else {
            return Err(format!("<{name}> is not an SSML element Amazon Polly supports"));
        };
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|e| e.to_string())?;
            let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
            // A namespace declaration is XML's own and belongs to no tag's attribute
            // list, so it is not checked against one.
            if key == "xmlns" || key.starts_with("xmlns:") {
                continue;
            }
            let raw = String::from_utf8_lossy(attribute.value.as_ref()).to_string();
            let value = quick_xml::escape::unescape(&raw)
                .map_err(|e| e.to_string())?
                .to_string();
            let Some((_, domain)) = declared
                .attributes
                .iter()
                .find(|(declared_key, _)| *declared_key == key)
            else {
                return Err(format!("<{name}> documents no {key} attribute"));
            };
            if !domain.accepts(&value) {
                return Err(format!(
                    "the {key} attribute of <{name}> does not accept {value:?}"
                ));
            }
        }
        Ok(())
    }
}

/// The characters Amazon Polly bills for.
///
/// Every character of plain text; for `ssml`, the text content only, because "SSML
/// tags are not counted as billed characters" (`TextLengthExceededException`). The
/// text content of an SSML document is every character it carries between its tags,
/// however that character is spelled: a character written as an entity reference or a
/// numeric character reference bills as the one character it stands for, the same way
/// a literal one does. The
/// SSML document is read with a real XML parser and every element it carries is
/// checked against the published vocabulary ([`ssml`]), so `Err` here means the SSML
/// is not well-formed, or not the SSML Amazon Polly accepts — both of which are
/// `InvalidSsmlException`. Well-formed means one whole XML document: exactly one
/// document element, every element closed, and nothing but whitespace outside the
/// document element, so a second root element or character data before or after it is
/// refused rather than billed. The whitespace XML admits there is XML's own
/// ([`crate::service::xml::is_xml_whitespace`]), not every character Unicode calls
/// blank.
pub fn billed_characters(text: &str, text_type: &str) -> Result<usize, String> {
    if text_type != "ssml" {
        return Ok(text.chars().count());
    }
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(text);
    let mut billed = 0usize;
    let mut depth = 0i32;
    let mut root_seen = false;
    loop {
        // Taken as a value so the event itself can be handed to the shared
        // character-data rule below.
        let event = reader.read_event().map_err(|e| e.to_string())?;
        match event {
            Event::Eof => break,
            Event::Start(ref e) => {
                check_document_element_is_the_only_one(depth, root_seen)?;
                check_ssml_element(e, &mut root_seen)?;
                depth += 1;
            }
            Event::Empty(ref e) => {
                check_document_element_is_the_only_one(depth, root_seen)?;
                check_ssml_element(e, &mut root_seen)?;
            }
            Event::End(_) => depth -= 1,
            Event::Text(ref t) if depth == 0 => {
                // Character data belongs to no document before the document element
                // opens or after it closes; XML admits only ITS OWN whitespace there,
                // so anything else is not well-formed.
                if !crate::service::xml::is_xml_whitespace(&String::from_utf8_lossy(t.as_ref())) {
                    return Err(CHARACTER_DATA_OUTSIDE_THE_DOCUMENT.to_string());
                }
            }
            // A CDATA section and a reference are character data too, and neither has
            // a whitespace-only form XML admits outside the document element.
            Event::CData(_) | Event::GeneralRef(_) if depth == 0 => {
                return Err(CHARACTER_DATA_OUTSIDE_THE_DOCUMENT.to_string());
            }
            // Text, a CDATA section and a reference are all character data of the
            // document, and what is billed is the characters that data stands for —
            // the service's one character-data rule
            // ([`crate::service::xml::character_data_length`]), which is where the
            // unescaping, the verbatim CDATA and the reference the reader emits on its
            // own are decided, and which `PutLexicon` measures a replacement with.
            // Only the accumulator is this operation's: text content is billed, and
            // markup, which contributes nothing, is not.
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                billed = billed
                    .saturating_add(crate::service::xml::character_data_length(&event)?);
            }
            _ => {}
        }
    }
    if !root_seen {
        return Err("the text carries no SSML element".to_string());
    }
    if depth != 0 {
        return Err("an SSML element is left unclosed".to_string());
    }
    Ok(billed)
}

/// Why a document carrying character data outside its document element is refused.
const CHARACTER_DATA_OUTSIDE_THE_DOCUMENT: &str =
    "the text carries character data outside the document element";

/// Check that an element opening here is not a SECOND document element: XML admits
/// exactly one, so an element that opens outside every other one once the document
/// element has been read makes the text two documents rather than one.
fn check_document_element_is_the_only_one(depth: i32, root_seen: bool) -> Result<(), String> {
    if depth == 0 && root_seen {
        return Err("a second document element follows the SSML document".to_string());
    }
    Ok(())
}

/// Check one element of an SSML document: that the document element is `<speak>`, as
/// SSML requires, and that the element and its attributes are inside the vocabulary
/// Amazon Polly publishes ([`ssml::check_element`]).
fn check_ssml_element(
    element: &quick_xml::events::BytesStart<'_>,
    root_seen: &mut bool,
) -> Result<(), String> {
    let name = ssml::element_name(element);
    if !*root_seen {
        if name != "speak" {
            return Err(format!("the document element is <{name}>, not <speak>"));
        }
        *root_seen = true;
    }
    ssml::check_element(&name, element)
}

// ---------------------------------------------------------------------------
// The synthesiser's parts
// ---------------------------------------------------------------------------

/// The two `OutputFormat` values whose streams are silence rather than the rendered
/// waveform, and why.
#[path = "synthesize_speech/legacy_audio.rs"]
pub mod legacy_audio;
#[path = "synthesize_speech/marks.rs"]
mod marks;
pub use marks::*;

// ---------------------------------------------------------------------------
// The synthesis itself
// ---------------------------------------------------------------------------

/// How long one request's synthesis may take.
///
/// CHOSEN. The pass is bounded in SIZE by [`plan::MAX_UTTERANCE_MS`] and
/// [`render::max_samples`], so this is the second half of the same bound: the caller's
/// wait. A `spawn_blocking` task cannot be aborted, so the budget bounds how long a
/// request waits for one, not the task itself.
pub const SYNTHESIS_TIME_BUDGET: ::std::time::Duration = ::std::time::Duration::from_secs(20);

/// Why a synthesis produced nothing.
pub enum SynthesisFailure {
    /// The `ssml` document could not be read as XML at all — the same failure
    /// [`billed_characters`] reports.
    InvalidSsml,
    /// The encoder refused the stream, which is this emulator's own fault rather than
    /// the request's.
    Encoding,
    /// An `OutputFormat` outside the model's values reached the encoders.
    Format(String),
}

/// One synthesis, owned so it can run on the blocking pool.
///
/// Both operations that answer with audio build one of these, so a stream's chunk and a
/// single request's body for the same text are the same bytes.
pub struct Synthesis {
    /// What to speak.
    pub text: String,
    /// How to read it: `text` or `ssml`.
    pub text_type: String,
    /// The encoding to write.
    pub output_format: String,
    /// The frequency to render at; `None` only for `json`, which carries marks.
    pub sample_rate: Option<u32>,
    /// The speech-mark types the request named.
    pub marks: Vec<String>,
    /// The lexemes to apply, in the order the request named their lexicons.
    pub lexicons: lexicon::Lexicons,
    /// The timbre of the voice the request named.
    pub timbre: render::Timbre,
}

impl Synthesis {
    /// The response body: the speech-mark stream for `json`, and the rendered waveform in
    /// its own encoding for every audio format.
    ///
    /// Synchronous and CPU-bound by nature, so every caller runs it on the blocking pool.
    pub fn produce(&self) -> Result<Vec<u8>, SynthesisFailure> {
        let utterance = plan::plan(&self.text, &self.text_type, &self.lexicons)
            .map_err(|_| SynthesisFailure::InvalidSsml)?;
        // `json` IS the speech-mark stream: the marks the request asked for, off the same
        // timeline the audio formats carry. A request that names no mark type asks for no
        // marks, so the stream is empty — Amazon Polly declares no error for an empty one.
        let Some(rate) = self.sample_rate else {
            return Ok(audio::speech_marks_stream(
                &utterance.marks_of_types(&self.marks),
            ));
        };
        // Every audio format carries the SAME rendered waveform, encoded its own way, so
        // one text is one utterance whichever format asks for it. The renderer produces at
        // most [`render::max_samples`] of them, so this buffer is bounded before it is
        // filled.
        let samples = render::render(&utterance, rate, self.timbre);
        let encoded = match self.output_format.as_str() {
            "pcm" => Some(audio::pcm(&samples)),
            "mulaw" => Some(audio::mulaw(&samples)),
            "alaw" => Some(audio::alaw(&samples)),
            "ogg_vorbis" => audio::ogg_vorbis(&samples, rate),
            // The two formats whose samples are not written: see [`legacy_audio`]. Their
            // streams still last as long as the speech.
            "ogg_opus" => audio::ogg_opus_silence_for(&samples, rate),
            "mp3" => audio::mp3_silence_for(&samples, rate),
            // `content_type_for` has already narrowed `OutputFormat` to the model's seven
            // values, so this is the exhaustiveness of the match rather than a path a
            // request takes.
            other => return Err(SynthesisFailure::Format(other.to_string())),
        };
        // The rate was settled against this format's own documented value set, and every
        // one of those values is one these encoders carry, so a `None` here is this
        // emulator failing to encode a rate it admitted — never a second refusal of a
        // member the request was already told was acceptable.
        encoded.ok_or(SynthesisFailure::Encoding)
    }
}

/// The `SynthesizeSpeech` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SynthesizeSpeech<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SynthesizeSpeech<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SynthesizeSpeech<Eng>
{
    type Input = SynthesizeSpeechInput;
    type Output = SynthesizeSpeechOutput;
    type Error = SynthesizeSpeechError;

    const NAME: &'static str = "SynthesizeSpeech";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SYNTHESIZE_SPEECH_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SYNTHESIZE_SPEECH_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let output_format = input.output_format.as_str();
        let content_type = content_type_for(output_format).ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(format!(
                "OutputFormat must be one of {}; got {output_format}",
                OUTPUT_FORMAT_VALUES.join(", ")
            ))
        })?;
        let engine = input.engine.as_deref().unwrap_or(DEFAULT_ENGINE);
        let text_type = input.text_type.as_deref().unwrap_or(DEFAULT_TEXT_TYPE);
        let marks = input.speech_mark_types.clone().unwrap_or_default();

        // Speech marks ride the `json` stream, so asking for them with an audio
        // format is the failure `MarksNotSupportedForFormatException` names.
        if !marks.is_empty() && output_format != "json" {
            return Err(SynthesizeSpeechErrorModeled::MarksNotSupportedForFormatException(
                crate::service::errors::marks_not_supported_for_format(),
            )
            .into());
        }
        if marks.iter().any(|mark| mark == "ssml") && text_type != "ssml" {
            return Err(
                SynthesizeSpeechErrorModeled::SsmlMarksNotSupportedForTextTypeException(
                    crate::service::errors::ssml_marks_not_supported_for_text_type(),
                )
                .into(),
            );
        }

        // The two documented text limits. `Text` carries no modeled length
        // constraint, so the boundary validator has not checked either.
        let total_characters = input.text.chars().count();
        if total_characters > MAX_TOTAL_CHARACTERS {
            return Err(SynthesizeSpeechErrorModeled::TextLengthExceededException(
                crate::service::errors::text_length_exceeded(),
            )
            .into());
        }
        let billed = billed_characters(&input.text, text_type).map_err(|_| {
            SynthesizeSpeechErrorModeled::InvalidSsmlException(
                crate::service::errors::invalid_ssml(),
            )
        })?;
        if billed > MAX_BILLED_CHARACTERS {
            return Err(SynthesizeSpeechErrorModeled::TextLengthExceededException(
                crate::service::errors::text_length_exceeded(),
            )
            .into());
        }

        // The sample rate, checked against the values the chosen format documents.
        // `json` carries speech marks rather than audio, so no rate applies to it and
        // none is resolved for it: a `SampleRate` sent with `json` is IGNORED, not
        // validated, because `SynthesizeSpeechInput$SampleRate` documents valid values
        // only per audio format and declares none for `json`.
        let sample_rate: Option<u32> = if output_format == "json" {
            None
        } else {
            Some(match input.sample_rate.as_deref() {
                Some(requested) => resolve_sample_rate(output_format, requested).ok_or_else(
                    || {
                        SynthesizeSpeechErrorModeled::InvalidSampleRateException(
                            crate::service::errors::invalid_sample_rate(),
                        )
                    },
                )?,
                None => default_sample_rate(output_format, engine),
            })
        };

        // Every lexicon the request names must be in the store, in the Region this
        // request was signed for: a lexicon is a regional resource, and
        // `LexiconNotFoundException` documents "a lexicon that is in a different
        // region" as one of the things it reports. This is the one place synthesis
        // reads state, and it is why an unknown name is a 404 rather than a silently
        // ignored input.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            SynthesizeSpeechErrorModeled::ServiceFailureException(
                crate::service::errors::service_failure(),
            )
        })?;
        // The lexemes of those lexicons, in the order the request named them, are what
        // synthesis applies to the text: a grapheme one of them defines is spoken as
        // that lexeme's replacement.
        let mut lexicons = lexicon::Lexicons::default();
        if let Some(names) = input.lexicon_names.as_ref() {
            let region = crate::state::request_region(ctx.parts());
            let guard = state.lock();
            for name in names {
                let Some(stored) = guard.lexicon(&region, name) else {
                    return Err(SynthesizeSpeechErrorModeled::LexiconNotFoundException(
                        crate::service::errors::lexicon_not_found(),
                    )
                    .into());
                };
                lexicons.add(&stored.content);
            }
        }

        // `VoiceId` decides which engines the request may name, and the voice catalog
        // is where that is recorded — so the check reads the same state
        // `DescribeVoices` reports and the two cannot disagree. A `VoiceId` the catalog
        // does not list is still resolved to the voice it names
        // (`crate::state::UNLISTED_VOICES`), so every value the model declares is
        // narrowed by its own voice's engines.
        if let Some(voice) = state.lock().voice(&input.voice_id).copied() {
            // "Provide an engine that is supported by the voice you select … If a
            // chosen voice isn't supported by the standard engine, this will result
            // in an error" (`SynthesizeSpeechInput$Engine`).
            if !voice.supports_engine(engine) {
                // Amazon Polly refuses this combination as a REQUEST-SHAPE failure —
                // HTTP 400 `ValidationException` naming the engine — rather than with
                // the `EngineNotSupportedException` the model also declares, so a
                // client that branches on the code takes the request-validation
                // branch for it.
                return Err(crate::scaffolded::Scaffolded::validation(format!(
                    "This voice does not support the selected engine: {engine}"
                )));
            }
            // "Optional language code … This is only necessary if using a bilingual
            // voice, such as Aditi, which can be used for either Indian English
            // (en-IN) or Hindi (hi-IN)" (`SynthesizeSpeechInput$LanguageCode`): the
            // member SELECTS the language of a bilingual voice and states no
            // requirement on the voice, so a value the chosen voice does not speak is
            // not a refusal — Amazon Polly serves such a request, synthesizing with
            // the voice the request named. The language is therefore read by the
            // bilingual voice's own default and never checked against the catalog
            // here; the modelled `LanguageNotSupportedException` is for a language the
            // service cannot synthesize at all (see `crate::service::errors`).
        }

        // The voice the request named decides the timbre of the speech, read off the
        // same catalog row its engine and language were checked against.
        let timbre = render::Timbre::for_gender(
            state
                .lock()
                .voice(&input.voice_id)
                .map(|voice| voice.gender),
        );
        // Reading the document, reading it into phonemes, producing the waveform and
        // encoding it is synchronous, CPU-bound work with no await of its own, so it goes
        // to the BLOCKING pool and the worker serving this request is released while it
        // runs. The `spawn_blocking(..).await` is a real await point, so the health
        // endpoint, a cancellation and a graceful shutdown keep working while a long
        // utterance is produced, and the wait is bounded by [`SYNTHESIS_TIME_BUDGET`].
        let job = Synthesis {
            text: input.text.clone(),
            text_type: text_type.to_string(),
            output_format: output_format.to_string(),
            sample_rate,
            marks,
            lexicons,
            timbre,
        };
        let audio_stream = match ::tokio::time::timeout(
            SYNTHESIS_TIME_BUDGET,
            ::tokio::task::spawn_blocking(move || job.produce()),
        )
        .await
        {
            Ok(Ok(Ok(body))) => body,
            Ok(Ok(Err(SynthesisFailure::InvalidSsml))) => {
                return Err(SynthesizeSpeechErrorModeled::InvalidSsmlException(
                    crate::service::errors::invalid_ssml(),
                )
                .into())
            }
            Ok(Ok(Err(SynthesisFailure::Format(format)))) => {
                return Err(crate::scaffolded::Scaffolded::validation(format!(
                    "OutputFormat must be one of {}; got {format}",
                    OUTPUT_FORMAT_VALUES.join(", ")
                )))
            }
            // The encoder refused a rate this format documents, the pass did not finish
            // inside the budget, or it did not run at all: in each case nothing was
            // synthesized, so the modelled server error is the answer rather than a
            // stream that stands for no speech.
            Ok(Ok(Err(SynthesisFailure::Encoding))) | Ok(Err(_)) | Err(_) => {
                return Err(SynthesizeSpeechErrorModeled::ServiceFailureException(
                    crate::service::errors::service_failure(),
                )
                .into())
            }
        };

        Ok(SynthesizeSpeechOutput {
            audio_stream,
            content_type: Some(content_type.to_string()),
            // "Number of characters synthesized" — the billed count, which for SSML
            // excludes the tags.
            request_characters: i32::try_from(billed).unwrap_or(i32::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_follows_the_documented_mapping() {
        for (format, expected) in [
            ("mp3", "audio/mpeg"),
            ("ogg_vorbis", "audio/ogg"),
            ("ogg_opus", "audio/ogg"),
            ("pcm", "audio/pcm"),
            ("mulaw", "audio/mulaw"),
            ("alaw", "audio/alaw"),
            ("json", "application/x-json-stream"),
        ] {
            assert_eq!(
                content_type_for(format),
                Some(expected),
                "format {format:?}: wrong content type"
            );
        }
        // Every modeled OutputFormat value is covered, so no request reaches the
        // handler's refusal path.
        for format in OUTPUT_FORMAT_VALUES {
            assert!(
                content_type_for(format).is_some(),
                "modeled OutputFormat {format:?} has no content type"
            );
        }
    }

    #[test]
    fn ssml_tags_are_not_billed() {
        let plain = billed_characters("hello world", "text").expect("plain text is never invalid");
        assert_eq!(plain, 11, "plain text bills every character");
        let ssml = billed_characters("<speak>hello world</speak>", "ssml").expect("valid ssml");
        assert_eq!(ssml, 11, "SSML tags are not billed, got {ssml}");
        // A character spelled as a reference is text content, so it bills as the one
        // character it stands for, whichever spelling it arrives in.
        let table: [(&str, &str, usize); 4] = [
            ("predefined entity", "<speak>a&amp;b</speak>", 3),
            ("numeric reference", "<speak>&#120;&#120;&#120;</speak>", 3),
            ("hexadecimal reference", "<speak>&#x78;y</speak>", 2),
            (
                "references around markup",
                "<speak>&lt;<break time=\"1s\"/>&gt;</speak>",
                2,
            ),
        ];
        for (case, text, expected) in table {
            let billed = billed_characters(text, "ssml").expect("valid ssml");
            assert_eq!(
                billed, expected,
                "case {case:?}: expected {expected} billed characters, got {billed}"
            );
        }
        assert!(
            billed_characters("<speak>unclosed", "ssml").is_err(),
            "malformed SSML must be rejected"
        );
    }

    /// RULE: the sentence and word marks of the utterance report the text they cover
    /// and the byte offsets of it in `Text`.
    #[test]
    fn speech_marks_are_computed_from_the_text() {
        let utterance = plan::plan("Hi there. Go!", "text", &lexicon::Lexicons::default())
            .expect("plain text plans");
        let covering: Vec<(&str, usize, usize, &str)> = utterance
            .marks
            .iter()
            .filter_map(|mark| {
                mark.span
                    .map(|(start, end)| (mark.mark_type, start, end, mark.value.as_str()))
            })
            .collect();
        assert_eq!(
            covering,
            vec![
                ("sentence", 0, 9, "Hi there."),
                ("word", 0, 2, "Hi"),
                ("word", 3, 8, "there"),
                ("sentence", 10, 13, "Go!"),
                ("word", 10, 12, "Go"),
            ],
            "marks must track the text's sentences and words"
        );
        let stream = audio::speech_marks_stream(&utterance.marks);
        assert_eq!(
            stream.iter().filter(|b| **b == b'\n').count(),
            utterance.marks.len(),
            "the stream is one newline-terminated JSON object per mark"
        );
    }
}

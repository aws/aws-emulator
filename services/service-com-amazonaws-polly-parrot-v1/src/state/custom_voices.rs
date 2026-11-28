// SPDX-License-Identifier: CC0-1.0

//! The CUSTOM voices `CreateVoice` makes, and the one projection of one onto the wire.
//!
//! A custom voice is a REGIONAL resource of an account — "If no custom voice with the
//! specified ID exists in your account and Region, the request fails with
//! `ResourceNotFoundException`" (`GetVoice`) — so it is keyed by the Region the create
//! call was signed for and then by the id this service minted, exactly as a lexicon is
//! keyed ([`crate::state::LexiconKey`]). One Region's voices therefore sit contiguously
//! in id order and a read of another Region sees nothing of them.
//!
//! Unlike the seeded catalog ([`crate::state::VOICE_CATALOG`]), which is a compile-time
//! constant of the voices Amazon Polly PROVIDES, these are created and deleted at
//! runtime and carry a STATUS: `CreateVoice` answers with a voice in `pending` status
//! and the voice reaches `active` on its own, which is what
//! [`settle_voice_status`] materialises on a read.
//!
//! Two projections, because the model gives two: `DescribeVoices` reports a subset of a
//! custom voice's members ([`described_custom_voice`]) and `GetVoice` reports all of
//! them ([`reported_custom_voice`]). Both are built here, so no two operations can
//! report one stored voice differently.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{CustomerManagedEncryption, Recording, RecordingData, Voice, VoiceCreationData};

use super::{reported_engines, reported_list, VOICE_CATALOG};

/// The key one stored custom voice sits under: the Region it was created in, then its
/// id.
///
/// Region first and id second, so the derived `Ord` lays one Region's voices out
/// contiguously in id order and
/// [`crate::state::ComAmazonawsPollyParrotV1State::custom_voices_in_region`] is a range
/// scan rather than a filter over the whole store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CustomVoiceKey {
    /// The Region the voice lives in, as [`crate::state::request_region`] read it from
    /// the creating call's credential scope.
    pub region: String,
    /// The minted `CustomVoiceId`, unique within `region`.
    pub voice_id: String,
}

impl CustomVoiceKey {
    /// The key for `voice_id` in `region`.
    pub fn new(region: &str, voice_id: &str) -> Self {
        Self {
            region: region.to_string(),
            voice_id: voice_id.to_string(),
        }
    }
}

/// One recording of a stored custom voice, as the create call recorded it.
///
/// `s3_url` is `None` for a recording whose audio arrived as `Bytes`: "Amazon Polly
/// doesn't store the audio after it creates the voice, and doesn't return this field in
/// responses" (`RecordingData$Bytes`), so the bytes are neither kept here nor reported,
/// and such a recording "has no fields in `Data`" (`Voice$VoiceCreationData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecording {
    /// The recording's `AudioFormat`.
    pub format: String,
    /// The Amazon S3 URL the audio was read from, for a recording that named one.
    pub s3_url: Option<String>,
}

/// The customer managed key a stored voice is encrypted with, when the create call named
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEncryption {
    /// `CustomerManagedEncryption$KmsKeyArn`.
    pub kms_key_arn: String,
    /// The encryption context the key is used with, in key order.
    pub encryption_context: BTreeMap<String, String>,
}

/// A status transition the clock owes a custom voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingVoiceStatus {
    /// The instant from which the voice is in `status`.
    pub due_at: SystemTime,
    /// The `VoiceStatus` the voice moves to.
    pub status: String,
    /// The `StatusReason` that moves with it.
    pub reason: Option<String>,
}

/// A custom voice exactly as `CreateVoice` recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCustomVoice {
    /// The minted `CustomVoiceId` (`vc-` and hexadecimal characters), which is also
    /// this entry's key.
    pub voice_id: String,
    /// The `Name` the create call chose. "The name doesn't have to be unique."
    pub name: String,
    /// The `CustomVoiceLanguageCode` of the recordings.
    pub language_code: String,
    /// `arn:aws:polly:<region>:<account>:voice/<voice-id>`.
    pub voice_arn: String,
    /// "The date and time when you created the custom voice."
    pub creation_time: SystemTime,
    /// The voice's status NOW — a value of the model's `VoiceStatus` enum.
    pub status: String,
    /// "Additional information about the status of a custom voice."
    pub status_reason: Option<String>,
    /// The recordings the voice was created from.
    pub recordings: Vec<StoredRecording>,
    /// The customer managed key the create call named, if any.
    pub customer_managed_encryption: Option<StoredEncryption>,
    /// The `ClientToken` the create call carried, which is what makes a retry of that
    /// call answer with this voice instead of creating a second one.
    pub client_token: Option<String>,
    /// The tags on this voice, in key order.
    pub tags: BTreeMap<String, String>,
    /// The status the CLOCK owes this voice, and when it falls due. A voice Amazon Polly
    /// answers with in a non-terminal status reaches its terminal one without any
    /// further call, so the due instant is stored and [`settle_voice_status`]
    /// materialises it when a call arrives after it.
    pub pending_status: Option<PendingVoiceStatus>,
}

impl StoredCustomVoice {
    /// Whether this voice speaks `language`.
    ///
    /// A custom voice is created from recordings in ONE language ("The language of the
    /// recordings... Amazon Polly doesn't detect the language from the recordings") and
    /// has no additional ones, so `DescribeVoices`'
    /// `IncludeAdditionalLanguageCodes` cannot widen or narrow this.
    pub fn speaks(&self, language: &str) -> bool {
        self.language_code == language
    }

    /// Whether this voice is available on `engine` ([`CUSTOM_VOICE_ENGINES`]).
    pub fn supports_engine(&self, engine: &str) -> bool {
        CUSTOM_VOICE_ENGINES.contains(&engine)
    }
}

/// `VoiceStatus`: "Amazon Polly is creating the voice."
pub const VOICE_STATUS_PENDING: &str = "pending";
/// `VoiceStatus`: "The voice is ready for speech synthesis."
pub const VOICE_STATUS_ACTIVE: &str = "active";
/// `VoiceStatus`: "Amazon Polly couldn't create the voice."
pub const VOICE_STATUS_FAILED: &str = "failed";

/// `VoiceOrigin`: "A voice that Amazon Polly provides."
pub const VOICE_ORIGIN_AWS: &str = "aws";
/// `VoiceOrigin`: "A custom voice that you created with `CreateVoice`."
pub const VOICE_ORIGIN_CUSTOMER: &str = "customer";

/// How long after the create call a custom voice reaches `active`.
///
/// CHOSEN: Amazon Polly's own voice creation reads the recordings and trains on them,
/// which takes as long as the audio makes it take, and nothing here trains anything.
/// The span is deliberately NOT zero, so a caller that reads straight after
/// `CreateVoice` still observes the `pending` status the create call answered with — and
/// the `ConflictException` `DeleteVoice` answers in that status — and it is short enough
/// that a caller polling for `active` need not wait long.
pub const VOICE_ACTIVATION_DELAY: Duration = Duration::from_secs(3);

/// The engines a custom voice is offered on.
///
/// Amazon Polly's custom voices are synthesised by its newest engine, the one the model
/// names `generative`: it is the engine the one synthesis path added beside custom
/// voices accepts at all ("Currently, only the `generative` engine is supported" —
/// `StartSpeechSynthesisStreamInput$Engine`). Every value here is a value of the
/// model's `Engine` enum, and the list is what `Voice$SupportedEngines` reports for a
/// custom voice, so the `Engine` filter of `DescribeVoices` narrows custom voices "in
/// the same way as to voices that Amazon Polly provides".
pub const CUSTOM_VOICE_ENGINES: &[&str] = &["generative"];

/// Materialise the status transition the clock has ALREADY made due on `voice`.
///
/// A call makes this before reporting a voice: it changes nothing a caller could not
/// have observed by waiting, because the transition was owed at `now` whether or not
/// anybody called. It never moves a status that is not yet due.
pub fn settle_voice_status(voice: &mut StoredCustomVoice, now: SystemTime) {
    if voice.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
        let pending = voice
            .pending_status
            .take()
            .expect("the pending status was just observed to be due");
        voice.status = pending.status;
        voice.status_reason = pending.reason;
    }
}

/// The language's name in English for a `LanguageCode`, read from the seeded voices.
///
/// `Voice$LanguageName` is "human readable name of the language in English", a property
/// of the LANGUAGE and not of the voice — one language is named the same way by every
/// voice that speaks it ([`VOICE_CATALOG`]) — so a custom voice reports the name this
/// service already carries for its language rather than a second spelling of it. Both
/// seeded lists are read, because a language whose voices this service does not OFFER
/// ([`super::UNLISTED_VOICES`]) is still a language a custom voice can be created in.
pub fn language_name(language_code: &str) -> Option<&'static str> {
    VOICE_CATALOG
        .iter()
        .chain(super::UNLISTED_VOICES.iter())
        .find(|voice| voice.language_code == language_code)
        .map(|voice| voice.language_name)
}

/// The `Voice` shape `DescribeVoices` reports for a custom voice.
///
/// The SUBSET that operation documents: "For a custom voice, `DescribeVoices` returns
/// `Origin`, `Id`, `Name`, `Status`, `LanguageCode`, `LanguageName`,
/// `SupportedEngines`, and `CreationTime`. To get `Arn`, `StatusReason`,
/// `VoiceCreationData`, and `CustomerManagedEncryption`, use `GetVoice`." `Gender` is
/// absent too: "Custom voices don't include this field."
pub fn described_custom_voice(stored: &StoredCustomVoice) -> Voice {
    Voice {
        id: Some(stored.voice_id.clone()),
        name: Some(stored.name.clone()),
        origin: Some(VOICE_ORIGIN_CUSTOMER.to_string()),
        status: Some(stored.status.clone()),
        language_code: Some(stored.language_code.clone()),
        language_name: language_name(&stored.language_code).map(str::to_string),
        supported_engines: reported_engines(CUSTOM_VOICE_ENGINES),
        creation_time: Some(::aws_smithy_types::DateTime::from(stored.creation_time)),
        // The members `DescribeVoices` does not report for a custom voice, and the one
        // no custom voice has at all.
        arn: None,
        gender: None,
        status_reason: None,
        additional_language_codes: None,
        voice_creation_data: None,
        customer_managed_encryption: None,
    }
}

/// The `Voice` shape `CreateVoice` and `GetVoice` report: every member of the stored
/// voice.
///
/// "Unlike `DescribeVoices`, `GetVoice` also returns `Arn`, `VoiceCreationData`,
/// `CustomerManagedEncryption`, and `StatusReason`" — so this is
/// [`described_custom_voice`] plus those four, and `Bytes` is in none of them.
pub fn reported_custom_voice(stored: &StoredCustomVoice) -> Voice {
    Voice {
        arn: Some(stored.voice_arn.clone()),
        status_reason: stored.status_reason.clone(),
        voice_creation_data: Some(VoiceCreationData {
            recordings: stored
                .recordings
                .iter()
                .map(|recording| Recording {
                    format: recording.format.clone(),
                    data: RecordingData {
                        s3_url: recording.s3_url.clone(),
                        // Never reported: the service does not keep the audio a
                        // recording passed as `Bytes`, so there is nothing to report.
                        bytes: None,
                    },
                })
                .collect(),
        }),
        customer_managed_encryption: stored.customer_managed_encryption.as_ref().map(
            |encryption| CustomerManagedEncryption {
                kms_key_arn: encryption.kms_key_arn.clone(),
                encryption_context: (!encryption.encryption_context.is_empty())
                    .then(|| encryption.encryption_context.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
            },
        ),
        ..described_custom_voice(stored)
    }
}

/// The `Voice` shape every operation reports for a voice Amazon Polly PROVIDES.
///
/// The seeded catalog's own rows: `Origin` says who provides the voice, and the members
/// only a custom voice has — the status, the ARN, the creation time, the recordings and
/// the key — are absent, which is what the model documents ("Voices that Amazon Polly
/// provides don't include this field").
pub fn provided_voice(stored: &super::StoredVoice) -> Voice {
    Voice {
        id: Some(stored.id.to_string()),
        name: Some(stored.name.to_string()),
        gender: Some(stored.gender.to_string()),
        origin: Some(VOICE_ORIGIN_AWS.to_string()),
        language_code: Some(stored.language_code.to_string()),
        language_name: Some(stored.language_name.to_string()),
        // Both list members are reported only when the voice has values for them: a
        // voice that speaks only its default language carries NO
        // `AdditionalLanguageCodes` key, rather than an empty array.
        additional_language_codes: reported_list(stored.additional_language_codes),
        supported_engines: reported_engines(stored.supported_engines),
        arn: None,
        status: None,
        status_reason: None,
        voice_creation_data: None,
        customer_managed_encryption: None,
        creation_time: None,
    }
}

/// A fresh `CustomVoiceId`, in the form Amazon Polly mints one: `vc-` followed by
/// hexadecimal characters (`Voice$Id`: "Amazon Polly assigns an ID that consists of
/// `vc-` followed by hexadecimal characters, for example, `vc-9a8fa85dd3`").
///
/// The hexadecimal half is [`MINTED_VOICE_ID_HEX_CHARACTERS`] characters of a random
/// UUID, so the id is as unique as the UUID is and the whole id satisfies the model's
/// `CustomVoiceId` pattern and length. What a READ accepts is not narrowed to this
/// length: an id the model's pattern admits simply names whatever voice the store holds
/// under it, or no voice at all.
pub fn mint_custom_voice_id() -> String {
    let hex = ::uuid::Uuid::new_v4().simple().to_string();
    format!("vc-{}", &hex[..MINTED_VOICE_ID_HEX_CHARACTERS])
}

/// How many hexadecimal characters a minted `CustomVoiceId` carries: as many as the
/// example in `Voice$Id`'s own documentation (`vc-9a8fa85dd3`).
pub const MINTED_VOICE_ID_HEX_CHARACTERS: usize = 10;

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice as a create call would have recorded it, in `pending` status with the
    /// transition the clock owes it.
    fn pending_voice(due_at: SystemTime) -> StoredCustomVoice {
        StoredCustomVoice {
            voice_id: "vc-0123456789".to_string(),
            name: "Narrator".to_string(),
            language_code: "en-US".to_string(),
            voice_arn: ::engine::emulator_arn("polly", "us-east-1", "voice/vc-0123456789"),
            creation_time: SystemTime::UNIX_EPOCH,
            status: VOICE_STATUS_PENDING.to_string(),
            status_reason: None,
            recordings: vec![StoredRecording {
                format: "wav".to_string(),
                s3_url: Some("s3://recordings/one.wav".to_string()),
            }],
            customer_managed_encryption: None,
            client_token: None,
            tags: BTreeMap::new(),
            pending_status: Some(PendingVoiceStatus {
                due_at,
                status: VOICE_STATUS_ACTIVE.to_string(),
                reason: None,
            }),
        }
    }

    /// The owed transition is materialised once it is due and never before.
    #[test]
    fn a_pending_status_is_settled_only_once_it_is_due() {
        let due_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        for (case, now, expected) in [
            ("before the due instant", SystemTime::UNIX_EPOCH, VOICE_STATUS_PENDING),
            ("at the due instant", due_at, VOICE_STATUS_ACTIVE),
            ("after the due instant", due_at + Duration::from_secs(1), VOICE_STATUS_ACTIVE),
        ] {
            let mut voice = pending_voice(due_at);
            settle_voice_status(&mut voice, now);
            assert_eq!(
                voice.status, expected,
                "case {case:?}: wrong status, got {voice:?}"
            );
            assert_eq!(
                voice.pending_status.is_some(),
                expected == VOICE_STATUS_PENDING,
                "case {case:?}: a settled transition must be taken, an unsettled one kept"
            );
        }
    }

    /// `DescribeVoices` reports the documented subset; `GetVoice` reports that subset
    /// plus the four members it adds — and neither reports the audio.
    #[test]
    fn the_two_projections_differ_in_exactly_the_four_documented_members() {
        let voice = pending_voice(SystemTime::UNIX_EPOCH);
        let described = described_custom_voice(&voice);
        let reported = reported_custom_voice(&voice);

        for (case, value) in [
            ("arn", described.arn.is_some()),
            ("voiceCreationData", described.voice_creation_data.is_some()),
        ] {
            assert!(
                !value,
                "case {case:?}: DescribeVoices does not report this member for a custom voice"
            );
        }
        assert_eq!(
            described.id, reported.id,
            "both projections report the same voice id"
        );
        assert_eq!(
            reported.arn,
            Some(voice.voice_arn.clone()),
            "GetVoice reports the voice's ARN, got {:?}",
            reported.arn
        );
        let recordings = reported
            .voice_creation_data
            .expect("GetVoice reports the creation data")
            .recordings;
        assert_eq!(
            recordings.len(),
            1,
            "one stored recording is one reported recording, got {recordings:?}"
        );
        assert_eq!(
            recordings[0].data.s3_url.as_deref(),
            Some("s3://recordings/one.wav"),
            "a recording read from Amazon S3 reports its URL, got {recordings:?}"
        );
        assert!(
            recordings[0].data.bytes.is_none(),
            "the audio is never reported, got {recordings:?}"
        );
    }

    /// Every minted id satisfies the model's `CustomVoiceId` pattern and length, and no
    /// two are the same.
    #[test]
    fn every_minted_voice_id_is_one_the_model_admits() {
        let mut minted = std::collections::BTreeSet::new();
        for round in 0..64 {
            let id = mint_custom_voice_id();
            let hex = id
                .strip_prefix("vc-")
                .unwrap_or_else(|| panic!("round {round}: {id:?} does not start with vc-"));
            assert!(
                !hex.is_empty() && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                "round {round}: the minted id {id:?} is outside the modeled CustomVoiceId pattern"
            );
            assert!(
                id.len() <= 32,
                "round {round}: the minted id {id:?} is longer than the modeled maximum"
            );
            assert!(
                minted.insert(id.clone()),
                "round {round}: the minted id {id:?} was minted twice"
            );
        }
    }

    /// A custom voice's language name is the one this service already carries, for every
    /// language `CreateVoice` admits.
    #[test]
    fn every_custom_voice_language_is_named_as_the_catalog_names_it() {
        for code in crate::ops::types::CUSTOM_VOICE_LANGUAGE_CODE_VALUES {
            let named = language_name(code);
            assert!(
                named.is_some(),
                "language {code:?}: no seeded voice names it, got {named:?}"
            );
        }
        assert_eq!(
            language_name("xx-XX"),
            None,
            "a language no seeded voice speaks is named by nothing"
        );
    }
}

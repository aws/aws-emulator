Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9353c1bb256472de141f0c55c99b9667dc6fcfc6

# The PlainTextMessageType/SSMLMessageType -> VoiceTextRecord member mapping is written out twice in send_voice_message

_Filed: 2026-11-24T19:25:46Z_
_Operation: SendVoiceMessage | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9353c1bb256472de141f0c55c99b9667dc6fcfc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two spoken-message shapes carry the same three members and are stored in the same record type, `state::VoiceTextRecord`. The handler writes that member-for-member mapping twice, once per body, inside one expression — so a member added to those shapes, or a field renamed on `VoiceTextRecord`, has to be edited in two places and can be carried by one copy and dropped by the other.

This crate already states the opposite rule for the structurally identical case: the three nested event-destination shapes were converted inline in two handlers, that duplication was removed, and the mapping now lives once in `ops/types/event_destination_records.rs` as `From` impls that each writer reaches through `.into()`. The voice-content mapping is the same shape of duplication that module was created to forbid, and it is the only one left. Moving it to one `From`/`Into` per shape (or one conversion of the three shared members) leaves every response byte and every existing test unchanged.

## Evidence

The duplicated mapping — services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/send_voice_message.rs:286-297:

                plain_text: content.plain_text_message.map(|body| {
                    crate::state::VoiceTextRecord {
                        text: body.text,
                        language_code: body.language_code,
                        voice_id: body.voice_id,
                    }
                }),
                ssml: content.ssml_message.map(|body| crate::state::VoiceTextRecord {
                    text: body.text,
                    language_code: body.language_code,
                    voice_id: body.voice_id,
                }),

One record type for both shapes — src/state.rs:165-172:

    /// Stored form of the two spoken-message shapes (`PlainTextMessageType`,
    /// `SSMLMessageType`), which carry the same three members.
    #[derive(Debug, Clone, Default)]
    pub struct VoiceTextRecord {
        pub text: Option<String>,
        pub language_code: Option<String>,
        pub voice_id: Option<String>,
    }

The rule this crate already states for the identical case — src/ops/types/event_destination_records.rs:10-15:

    //! ... The mapping of each shape
    //! (`CloudWatchLogsDestination`, `KinesisFirehoseDestination`, `SnsDestination` ...)
    //! is therefore written ONCE, here, in each direction: a member added to one of those
    //! shapes, or a renamed state field, is carried by every handler that converts it or by
    //! none — it cannot be carried by one and dropped by another.

and the writers that reach it through `.into()` — src/ops/create_configuration_set_event_destination.rs:297-299 (`cloud_watch_logs: definition.cloud_watch_logs_destination.map(Into::into),` ...) and src/ops/get_configuration_set_event_destinations.rs:205-207 (`cloud_watch_logs_destination: record.cloud_watch_logs.as_ref().map(Into::into),` ...).

Tests that pin the behaviour the refactor must not change: tests/all/voice_message_content_record.rs:174-259 (`every_supplied_body_is_recorded_as_present_with_the_members_it_carried` — rows for plain text and SSML with every member, with only `Text`, and with no members at all, asserting `text`/`language_code`/`voice_id` per body via `members()` at :143-162) and tests/all/configuration_set_lifecycle.rs:932-1046 (each accepted send recorded with its body).

## Steps to Reproduce

1. Read src/ops/send_voice_message.rs:286-297: the same three-member mapping onto `state::VoiceTextRecord` appears twice, once for `plain_text_message` and once for `ssml_message`.
2. Read src/ops/types/event_destination_records.rs:10-15 and :30-80: this crate already removed the identical duplication for the three nested destination shapes by writing each mapping ONCE as a `From` impl, which the create handler and the reader both reach through `.into()`.
3. Add a member to `state::VoiceTextRecord` (or rename one): both copies in send_voice_message.rs must be edited, and editing only one compiles for the other body only if the shapes' members happen to line up — the drift event event_destination_records.rs:13-15 says cannot happen for the destination shapes.
4. Verify the scope of the fix: `cargo test -p service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice` must stay green with no test edited, in particular tests/all/voice_message_content_record.rs:174-259.

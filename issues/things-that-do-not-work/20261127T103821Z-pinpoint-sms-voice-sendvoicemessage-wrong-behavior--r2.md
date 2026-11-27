Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21

# The minted MessageId concatenates the mint timestamp and the raw sequence, so the "not a readable call count" rule its own doc states is broken

_Filed: 2026-11-27T10:38:21Z_
_Operation: SendVoiceMessage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`mint_message_id` is documented as producing an opaque identifier in which the wall-clock nanosecond is "mixed in so the id is not a readable call count". The implementation directly below that sentence does not mix anything: it CONCATENATES the two values, `format!("{nanos:016x}{seq:016x}")`. A u64 is exactly 16 hex characters, so the two halves never overlap and both are readable verbatim out of every response: characters 0..16 are the mint instant in nanoseconds since the epoch, and characters 16..32 are the store's `next_message_seq` — the count of messages this emulator has accepted — zero-padded in hex. The first accepted message is therefore reported as `<nanos>0000000000000001`, the second `<nanos>0000000000000002`, which is precisely the readable call count the doc says the design prevents, and the id is not opaque in either half.

The defect is the computation, not the uniqueness: ids stay unique (the sequence advances before the mint, as `crate::state` states), so nothing else in the crate changes. Deriving the 32 hex characters from the two inputs instead of pasting them together (the "mixed in" the doc already claims) satisfies the stated rule and leaves the length, the character set and every existing assertion intact.

## Evidence

The rule, stated immediately above the code that breaks it — services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/send_voice_message.rs:303-308:
    /// Mint the opaque `MessageId` for the `seq`-th message this emulator accepted.
    ///
    /// Uniqueness comes from `seq`, a monotonic per-service counter; the wall-clock
    /// nanosecond at mint time is mixed in so the id is not a readable call count. The
    /// result is 32 lowercase hex characters — an opaque identifier, derived from nothing
    /// in the request.

The code under it — send_voice_message.rs:309-314:
    fn mint_message_id(seq: u64) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u64, |since| since.as_nanos() as u64);
        format!("{nanos:016x}{seq:016x}")
    }
`{seq:016x}` is a u64 in exactly 16 hex characters, so the second half of the returned id IS `seq` written out; nothing is mixed.

What `seq` is, from this crate's own state module — src/state.rs:62-64:
    /// Monotonic sequence behind the `MessageId` `SendVoiceMessage` mints, so two
    /// calls can never be handed the same id.
    pub next_message_seq: u64,
and the handler that advances it before minting — src/ops/send_voice_message.rs:264-265:
        store.next_message_seq += 1;
        let message_id = mint_message_id(store.next_message_seq);

Test coverage that constrains the id, and what it does NOT pin — tests/all/configuration_set_lifecycle.rs:986-994 asserts only that `MessageId` is non-empty and that no id repeats:
            assert!(
                !message_id.is_empty(),
                "case {case:?} attempt {attempt}: MessageId must be non-empty, got {}",
                answer.body
            );
            assert!(
                seen.insert(message_id.clone()),
                "case {case:?} attempt {attempt}: MessageId {message_id} was already issued"
            );
No test in the crate pins the 16+16 layout or the visibility of the counter, so no test states a reason for the current spelling.

## Steps to Reproduce

1. Boot the emulator with this service mounted and grant the account the entitlement the resource-scoped actions need (as tests/all/configuration_set_lifecycle.rs's harness does: set `legacy_surface_entitled = true`), so SendVoiceMessage is served rather than denied 403.
2. POST /v1/sms-voice/voice/message with {"DestinationPhoneNumber": "+12065550100", "Content": {"PlainTextMessage": {"Text": "one"}}} and read `MessageId` from the 200 response.
3. Repeat the identical request twice more, reading each `MessageId`.
4. Split each id after character 16: the second half of the three ids is `0000000000000001`, `0000000000000002`, `0000000000000003` — the emulator's accepted-message count in hex, i.e. the "readable call count" send_voice_message.rs:305-306 says the mint prevents; the first half is the mint instant in nanoseconds, equally readable.
5. Compare with the sentence at src/ops/send_voice_message.rs:303-308 ("mixed in so the id is not a readable call count ... an opaque identifier") and the `format!("{nanos:016x}{seq:016x}")` at line 313 that produces it.

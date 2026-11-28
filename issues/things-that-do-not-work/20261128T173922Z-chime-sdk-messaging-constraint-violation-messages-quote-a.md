Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-message-delete-rejected
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-message-delete-rejected chime-sdk-messaging-channel-message-redact-rejected chime-sdk-messaging-get-channel-message-malformed-requests-rejected chime-sdk-messaging-search-channels-malformed-fields-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|DeleteChannelMessage|4xx->4xx|$.message
Issue-Key: chime-sdk-messaging|RedactChannelMessage|4xx->4xx|$.message
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->4xx|$.message
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->4xx|$.message
Issue-Commit: 757e19016492578f68fb6ad3f54c4ad74c32c8c1

# chime-sdk-messaging constraint-violation messages quote a re-anchored regex and a rendered item count

> _Note: this issue was identified against the code base at commit `757e19016492578f68fb6ad3f54c4ad74c32c8c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChannelMessage

## Description
Four net-new wire divergences with one root cause: the emulator's generic constraint-violation renderer builds AWS's '<n> validation error(s) detected: ...' sentence from the MODEL's internal representation rather than from the wire-facing form AWS prints. The status (400), the error class and the sentence's skeleton are right; two of its interpolated slots are wrong. All four causes are numbered below and each carries its own fixture and step; all four must be closed before this issue is submitted.

Shared AWS behavior: the framework-level constraint refusal is not in the Smithy model (it is recorded behavior only; the model supplies only the constraint itself). The recordings show AWS answering HTTP 400 with the body member 'message' (lower-case, the framework envelope, distinct from the service's own 'Message') and the fixed skeleton: '<count> validation error detected: Value '<the submitted value>' at '<the member's wire name>' failed to satisfy constraint: <the constraint clause>'. Fixed text: ' validation error detected: Value ', ' at ', ' failed to satisfy constraint: '. Variable parts, taken from the request and the model: the error count, the submitted value, the member name, and the constraint clause — 'Member must satisfy regular expression pattern: <pattern>' for a @pattern violation and 'Member must have length greater than or equal to <min>' for a @length violation.

Two slots are rendered wrongly, and each slot is one cause family:
(a) the PATTERN slot: AWS prints the pattern as the API publishes it, WITHOUT the '^'/'$' anchors — for MessageId the published form is '[-_a-zA-Z0-9]*'. The emulator prints the model's internal anchored source, '^[-_a-zA-Z0-9]*$'.
(b) the VALUE slot for a collection: AWS prints the submitted collection's literal wire rendering — for an empty list, '[]'. The emulator prints a human summary of it, '0 items'.

Shared why: this sentence is the one AWS surface clients and SDK integration tests match on verbatim; anchors or a humanised count in it break error-mapping tables, assertion libraries and log alarms. The anchored form is also actively misleading — it is not a regex AWS ever publishes for that member, so a developer copying it into their own pre-validation gets a different (anchored) matcher than the API documents.

Shared fix: the crate's shared constraint-violation message renderer (one path — all four operations emit the identical malformation) must strip the '^'...'$' anchors when interpolating a @pattern into the 'Member must satisfy regular expression pattern: ' clause, and must interpolate a collection value as its literal wire rendering ('[]' for an empty list) rather than an '<n> items' summary.

Cause 1 of 4: DeleteChannelMessage MessageId @pattern violation prints the anchored pattern.
AWS: Smithy DeleteChannelMessage.MessageId is a required @httpLabel MessageId (@length 1..128, pattern '^[-_a-zA-Z0-9]*$'); the recorded refusal prints it unanchored as '[-_a-zA-Z0-9]*'.
Emulator: prints '^[-_a-zA-Z0-9]*$'.
Fix: the shared constraint-violation renderer, reached from DeleteChannelMessage's input validation.
Traces: chime-sdk-messaging-channel-message-delete-rejected#task#10.

Cause 2 of 4: RedactChannelMessage MessageId @pattern violation prints the anchored pattern.
AWS: Smithy RedactChannelMessage.MessageId is a required @httpLabel MessageId with the same constraint; recorded refusal prints '[-_a-zA-Z0-9]*'.
Emulator: prints '^[-_a-zA-Z0-9]*$'.
Fix: as cause 1, from RedactChannelMessage's input validation.
Traces: chime-sdk-messaging-channel-message-redact-rejected#task#6.

Cause 3 of 4: GetChannelMessage MessageId @pattern violation prints the anchored pattern.
AWS: Smithy GetChannelMessage.MessageId is a required @httpLabel MessageId with the same constraint; recorded refusal prints '[-_a-zA-Z0-9]*'.
Emulator: prints '^[-_a-zA-Z0-9]*$'.
Fix: as cause 1, from GetChannelMessage's input validation.
Traces: chime-sdk-messaging-get-channel-message-malformed-requests-rejected#task#8.

Cause 4 of 4: SearchChannels Fields @length violation renders the empty list as '0 items' instead of '[]'.
AWS: Smithy SearchChannels.Fields is a required SearchFields list with @length min 1, max 20; the recorded refusal is "1 validation error detected: Value '[]' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1" — the value slot carries the collection's literal rendering '[]' and the member slot the wire name 'fields'.
Emulator: the same sentence but with the value slot rendered as '0 items'.
Fix: the shared constraint-violation renderer's collection-value interpolation, reached from SearchChannels' input validation.
Traces: chime-sdk-messaging-search-channels-malformed-fields-rejected#task#6.

## Evidence
Replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout. In all four clusters status (400) and everything but the framework 'message' member matched; only $.message diverged.

Cause 1 — DeleteChannelMessage (chime-sdk-messaging-channel-message-delete-rejected, task#10):
  $.message expected: "1 validation error detected: Value 'sim2real.bad.id' at 'messageId' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_a-zA-Z0-9]*"
  $.message got:      "1 validation error detected: Value 'sim2real.bad.id' at 'messageId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-_a-zA-Z0-9]*$"

Cause 2 — RedactChannelMessage (chime-sdk-messaging-channel-message-redact-rejected, task#6): byte-identical divergence, same expected/actual pair.

Cause 3 — GetChannelMessage (chime-sdk-messaging-get-channel-message-malformed-requests-rejected, task#8):
  $.message expected: "1 validation error detected: Value 'not.a+valid+id' at 'messageId' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_a-zA-Z0-9]*"
  $.message got:      "... Member must satisfy regular expression pattern: ^[-_a-zA-Z0-9]*$"

Cause 4 — SearchChannels (chime-sdk-messaging-search-channels-malformed-fields-rejected, task#6):
  $.message expected: "1 validation error detected: Value '[]' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1"
  $.message got:      "1 validation error detected: Value '0 items' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1"

Message anatomy: the only diverging slots are the pattern (anchors must not be printed) and the collection value ('[]', not '0 items'). 'sim2real.bad.id' and 'not.a+valid+id' are the values those fixtures happened to submit — they are echoed correctly and are NOT part of the requirement; the requirement is that the value slot echoes whatever the request sent and the pattern slot is unanchored.

Related but NOT covering: the open issue 20261127T195730Z-chime-sdk-media-pipelines-pattern-constraint-validation-messages.md reports the same renderer defect in a DIFFERENT service (chime-sdk-media-pipelines) and carries no key for these steps.

## Steps to Reproduce
Region us-east-1. Create an AppInstance and an AppInstanceUser (chime-sdk-identity) and a standard channel; use the AppInstanceUser ARN as x-amz-chime-bearer.

1. DELETE /channels/{ChannelArn}/messages/bad.message.id — a MessageId containing '.' violates MessageId's pattern. Expect HTTP 400 with message "1 validation error detected: Value 'bad.message.id' at 'messageId' failed to satisfy constraint: Member must satisfy regular expression pattern: [-_a-zA-Z0-9]*" — note NO '^' and NO '$'.
2. POST /channels/{ChannelArn}/messages/bad.message.id?operation=redact — same expected message.
3. GET /channels/{ChannelArn}/messages/bad.message.id — same expected message.
4. POST /channels?operation=search with body {"Fields": []} — expect HTTP 400 with message "1 validation error detected: Value '[]' at 'fields' failed to satisfy constraint: Member must have length greater than or equal to 1". The emulator renders the value slot as '0 items'.

In steps 1-3 the emulator appends/prepends the anchors; in step 4 it humanises the empty list.

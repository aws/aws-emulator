Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-memberships-list-invalid-query-rejected
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-memberships-list-invalid-query-rejected chime-sdk-messaging-channel-message-delete-rejected chime-sdk-messaging-channel-message-redact-rejected chime-sdk-messaging-get-channel-message-malformed-requests-rejected chime-sdk-messaging-message-status-sub-channel-id-on-standard-channel-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|ListChannelMemberships|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|DeleteChannelMessage|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|RedactChannelMessage|4xx->4xx|$.Message
Issue-Related: 20261128T173922Z-chime-sdk-messaging-input-arn-and-chimebearer-refusals.md
Issue-Commit: 757e19016492578f68fb6ad3f54c4ad74c32c8c1

# chime-sdk-messaging rejects SubChannelId on a standard channel with an emulator-authored message instead of AWS's sentence

> _Note: this issue was identified against the code base at commit `757e19016492578f68fb6ad3f54c4ad74c32c8c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannelMemberships

## Description
Four net-new wire divergences with one root cause: every operation that accepts an optional SubChannelId refuses it on a non-elastic channel with the SAME emulator-authored sentence, which discloses an emulator limitation instead of stating AWS's rule. The status (400) and the error class are right; the message is not. All four causes are numbered below and each carries its own fixture and step; all four must be closed before this issue is submitted.

Shared AWS behavior: Smithy models SubChannelId as an OPTIONAL member on these operations (ListChannelMemberships/GetChannelMessage/DeleteChannelMessage via @httpQuery('sub-channel-id'), RedactChannelMessage in the body; target SubChannelId, @length 1..128, pattern '^[-_a-zA-Z0-9]*$'), and each declares BadRequestException with httpError 400 (doc: 'The input parameters don't match the service's restrictions.'). SubChannelId is meaningful only for an elastic channel — one created with an ElasticChannelConfiguration. The recordings show AWS refusing SubChannelId on a channel that is NOT elastic with HTTP 400, Code 'BadRequest', and the message 'Non elastic channel must not specify SubChannelId.' That entire sentence is fixed text: AWS interpolates neither the supplied SubChannelId nor the channel ARN into it.

Shared emulator behavior: HTTP 400 with the message 'SubChannelId "<the supplied value>" is not supported: this emulator does not serve elastic channels'. That sentence appears nowhere in AWS's vocabulary. It (a) uses none of AWS's wording, (b) interpolates the submitted SubChannelId value, which AWS does not, and (c) states a property of the EMULATOR ('this emulator does not serve elastic channels') rather than of the request — so the same refusal is emitted whether the target channel is elastic or not, and a caller cannot tell a genuine non-elastic-channel refusal from an unimplemented feature.

Shared why: a client that string-matches the refusal (assertions, error-mapping tables, log alarms) never matches, and support tooling that reads the message concludes elastic channels are unsupported rather than that it addressed a sub-channel on a standard channel. The interpolated SubChannelId also puts a request value into a message AWS keeps constant, so a recorded-response comparison diverges on every distinct SubChannelId.

Shared fix: the shared SubChannelId admission check reached by these handlers (it is one path — all four operations emit the byte-identical sentence) must decide elasticity from the resolved channel's ElasticChannelConfiguration and raise BadRequestException(400)/Code 'BadRequest' with exactly 'Non elastic channel must not specify SubChannelId.', with nothing interpolated; a channel that IS elastic must not reach this refusal at all.

Cause 1 of 4: ListChannelMemberships(SubChannelId=...) on a standard channel.
AWS: 400 / 'Non elastic channel must not specify SubChannelId.' (Smithy: ListChannelMemberships.SubChannelId @httpQuery 'sub-channel-id'; BadRequestException 400).
Emulator: 400 / 'SubChannelId "<value>" is not supported: this emulator does not serve elastic channels'.
Fix: ListChannelMemberships' input validation / the shared SubChannelId check.
Traces: chime-sdk-messaging-channel-memberships-list-invalid-query-rejected#task#7.

Cause 2 of 4: DeleteChannelMessage(SubChannelId=...) on a standard channel.
AWS: 400 / 'Non elastic channel must not specify SubChannelId.' (Smithy: DeleteChannelMessage.SubChannelId @httpQuery 'sub-channel-id'; BadRequestException 400; success is 204).
Emulator: 400 / 'SubChannelId "<value>" is not supported: this emulator does not serve elastic channels'.
Fix: DeleteChannelMessage's input validation / the shared SubChannelId check.
Traces: chime-sdk-messaging-channel-message-delete-rejected#task#9.

Cause 3 of 4: RedactChannelMessage(SubChannelId=...) on a standard channel.
AWS: 400 / 'Non elastic channel must not specify SubChannelId.' (Smithy: RedactChannelMessage.SubChannelId is a body member; BadRequestException 400).
Emulator: 400 / 'SubChannelId "<value>" is not supported: this emulator does not serve elastic channels'.
Fix: RedactChannelMessage's input validation / the shared SubChannelId check.
Traces: chime-sdk-messaging-channel-message-redact-rejected#task#5.

Cause 4 of 4: GetChannelMessage(SubChannelId=...) on a standard channel, and the same refusal on the message-status read path.
AWS: 400 / 'Non elastic channel must not specify SubChannelId.' (Smithy: GetChannelMessage.SubChannelId @httpQuery 'sub-channel-id'; BadRequestException 400).
Emulator: 400 / 'SubChannelId "<value>" is not supported: this emulator does not serve elastic channels'.
Fix: GetChannelMessage's input validation / the shared SubChannelId check.
Traces: chime-sdk-messaging-get-channel-message-malformed-requests-rejected#task#6; the same cluster covers chime-sdk-messaging-message-status-sub-channel-id-on-standard-channel-rejected#task#7 (GetChannelMessageStatus).

## Evidence
Replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout. Status and error class matched (400); only $.Message diverged, identically, in all four clusters.

Cause 1 — ListChannelMemberships (chime-sdk-messaging-channel-memberships-list-invalid-query-rejected, task#7):
  $.Message: expected 'Non elastic channel must not specify SubChannelId.'
              got      'SubChannelId "sim2realsub" is not supported: this emulator does not serve elastic channels'

Cause 2 — DeleteChannelMessage (chime-sdk-messaging-channel-message-delete-rejected, task#9):
  $.Message: expected 'Non elastic channel must not specify SubChannelId.'
              got      'SubChannelId "sim2realsub" is not supported: this emulator does not serve elastic channels'

Cause 3 — RedactChannelMessage (chime-sdk-messaging-channel-message-redact-rejected, task#5):
  $.Message: expected 'Non elastic channel must not specify SubChannelId.'
              got      'SubChannelId "sim2realsub" is not supported: this emulator does not serve elastic channels'

Cause 4 — GetChannelMessage (chime-sdk-messaging-get-channel-message-malformed-requests-rejected, task#6; cluster also covers chime-sdk-messaging-message-status-sub-channel-id-on-standard-channel-rejected#task#7):
  $.Message: expected 'Non elastic channel must not specify SubChannelId.'
              got      'SubChannelId "sub1" is not supported: this emulator does not serve elastic channels'

Message anatomy: AWS's sentence is 100% fixed text. The emulator's sentence has no fixed text in common with it and additionally interpolates the submitted SubChannelId — 'sim2realsub' and 'sub1' above are the values the two fixtures happened to send and are NOT part of any requirement; the requirement is that no value is interpolated at all.

## Steps to Reproduce
Region us-east-1. Create an AppInstance and an AppInstanceUser (chime-sdk-identity), then create a STANDARD channel — CreateChannel with NO ElasticChannelConfiguration — and send one message to it so a MessageId exists. Use the AppInstanceUser ARN as x-amz-chime-bearer throughout. Pick any SubChannelId that satisfies the modelled pattern, e.g. 'subone'.

1. GET /channels/{ChannelArn}/memberships?sub-channel-id=subone (ListChannelMemberships).
2. DELETE /channels/{ChannelArn}/messages/{MessageId}?sub-channel-id=subone (DeleteChannelMessage).
3. POST /channels/{ChannelArn}/messages/{MessageId}?operation=redact with body {"SubChannelId":"subone"} (RedactChannelMessage).
4. GET /channels/{ChannelArn}/messages/{MessageId}?sub-channel-id=subone (GetChannelMessage), and the same query on the message-status read.

Each must answer HTTP 400 with Code 'BadRequest' and Message exactly 'Non elastic channel must not specify SubChannelId.' — no SubChannelId value echoed. The emulator instead answers 'SubChannelId "subone" is not supported: this emulator does not serve elastic channels'. Repeat step 1 with a different SubChannelId to confirm the emulator's message varies with the request value while AWS's does not.

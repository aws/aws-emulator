Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-associate-channel-flow-non-flow-arn-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-associate-channel-flow-non-flow-arn-rejected chime-sdk-messaging-associate-channel-flow-unknown-flow-not-found chime-sdk-messaging-channel-moderators-list-invalid-input-rejected chime-sdk-messaging-get-channel-message-malformed-requests-rejected chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected chime-sdk-messaging-list-channels-associated-with-channel-flow-rejected chime-sdk-messaging-search-channels-malformed-fields-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|AssociateChannelFlow|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|AssociateChannelFlow|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|AssociateChannelFlow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|AssociateChannelFlow|4xx->4xx|status
Issue-Key: chime-sdk-messaging|ListChannelModerators|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|ListChannelModerators|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|ListChannelModerators|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|ListChannelModerators|4xx->4xx|status
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->4xx|status
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->2xx|$.Code
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->2xx|$.Message
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|GetChannelMessage|4xx->2xx|status
Issue-Key: chime-sdk-messaging|ListChannelBans|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|ListChannelBans|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|ListChannelBans|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|ListChannelBans|4xx->4xx|status
Issue-Key: chime-sdk-messaging|ListChannelsAssociatedWithChannelFlow|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|ListChannelsAssociatedWithChannelFlow|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|ListChannelsAssociatedWithChannelFlow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|ListChannelsAssociatedWithChannelFlow|4xx->4xx|status
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->4xx|$.Code
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->4xx|$.Message
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->4xx|status
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->2xx|$.Code
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->2xx|$.Message
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|SearchChannels|4xx->2xx|status
Issue-Commit: 757e19016492578f68fb6ad3f54c4ad74c32c8c1

# chime-sdk-messaging input-ARN and ChimeBearer refusals carry the wrong error class, status and message

> _Note: this issue was identified against the code base at commit `757e19016492578f68fb6ad3f54c4ad74c32c8c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateChannelFlow

## Description
Ten net-new wire divergences, all in the Chime SDK messaging emulator's input-ARN / ChimeBearer admission path. Every handler that must distinguish "this ARN is syntactically fine but is not the KIND of resource this member names" from "the caller is not permitted" collapses both onto a single ForbiddenException (403) — and in two cases onto no refusal at all. All ten causes are numbered below and each carries its own fixture and step; all ten must be closed before this issue is submitted.

Cause 1 of 10: AssociateChannelFlow with a ChannelArn that satisfies the modelled ARN pattern but does not name a Chime channel.
AWS: Smithy models AssociateChannelFlow.ChannelArn as a required @httpLabel ChimeArn (@length 5..1600, pattern '^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:[^/].{0,1023}$') and declares BadRequestException(400), NotFoundException(404), ForbiddenException(403), UnauthorizedClientException(401); the error body members are Code (ErrorCode enum) and Message. The recording shows AWS answering HTTP 400 with Code 'BadRequest' and the fully fixed sentence 'An invalid Channel ARN was supplied for the input parameter.' — no value from the request is interpolated; the shape rendered in x-amzn-errortype is BadRequestException.
Emulator: HTTP 403, Code 'Forbidden', Message 'An invalid App Instance ARN was supplied for the input parameter.', x-amzn-errortype 'ForbiddenException'. The status, the error code, the errortype header and the message all diverge, and the message names the wrong ARN member (App Instance rather than Channel).
Why: a client that branches on 400-vs-403 to tell "I built the ARN wrong, fix my input" from "my principal lacks permission, escalate" takes the wrong branch and retries or alarms on an authorization failure that never happened; and the message points the operator at the AppInstanceArn it never supplied.
Fix: AssociateChannelFlow's input validation in its handler — resolve ChannelArn as a channel ARN and refuse a non-channel ARN with the modelled BadRequestException(400)/Code 'BadRequest' and the Channel-ARN sentence, before any permission decision.
Traces: chime-sdk-messaging-associate-channel-flow-non-flow-arn-rejected#task#4 (and #task#5).

Cause 2 of 10: AssociateChannelFlow naming a ChannelFlowArn that is well-formed but names no channel flow.
AWS: the operation models NotFoundException with httpError 404 (doc: 'One or more of the resources in the request does not exist in the system.'). The recording shows HTTP 404, Code 'NotFound', fixed message 'The specified channel flow was not found.'
Emulator: HTTP 403, Code 'Forbidden', Message 'An invalid App Instance ARN was supplied for the input parameter.', x-amzn-errortype 'ForbiddenException'.
Why: an absent dependency is reported as a permission denial, so a client's create-if-missing / wait-for-resource logic keyed on NotFoundException never fires and the caller instead investigates IAM.
Fix: AssociateChannelFlow's handler — resolve ChannelFlowArn against channel-flow state and raise NotFoundException(404) with that sentence when the flow is absent.
Traces: chime-sdk-messaging-associate-channel-flow-unknown-flow-not-found#task#4.

Cause 3 of 10: ListChannelModerators with a ChannelArn that is not a channel ARN.
AWS: Smithy ListChannelModerators.ChannelArn is a required @httpLabel ChimeArn with the same pattern; BadRequestException(400) is modelled. Recording: HTTP 400, Code 'BadRequest', fixed message 'An invalid Channel ARN was supplied for the input parameter.'
Emulator: HTTP 403, Code 'Forbidden', Message 'You do not have sufficient access to perform this action.', x-amzn-errortype 'ForbiddenException'.
Why: same misclassification as cause 1 — a malformed-input bug is surfaced as an access-control failure on a read path, so callers never correct the ARN.
Fix: ListChannelModerators' input validation in its handler.
Traces: chime-sdk-messaging-channel-moderators-list-invalid-input-rejected#task#8.

Cause 4 of 10: ListChannelModerators with an x-amz-chime-bearer header that is a well-formed ARN but does not name an AppInstanceUser.
AWS: Smithy models ChimeBearer as a required @httpHeader('x-amz-chime-bearer') ChimeArn and declares UnauthorizedClientException with httpError 401 (doc: 'The client is not currently authorized to make the request.'). Recording: HTTP 401, Code 'Unauthorized', fixed message 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.' — the header name is part of the fixed text; no request value is interpolated.
Emulator: HTTP 403, Code 'Forbidden', Message 'You do not have sufficient access to perform this action.', x-amzn-errortype 'ForbiddenException'.
Why: 401 tells an SDK/credential layer the caller identity itself is unusable (re-acquire the bearer); 403 tells it the identity is fine but the action is denied. The emulator sends every bad bearer down the second path, so bearer-refresh logic is never exercised.
Fix: the shared ChimeBearer resolution used by the handlers — refuse a bearer that is not an AppInstanceUser ARN with UnauthorizedClientException(401)/Code 'Unauthorized' and that sentence.
Traces: chime-sdk-messaging-channel-moderators-list-invalid-input-rejected#task#9.

Cause 5 of 10: GetChannelMessage with a ChannelArn that is not a channel ARN.
AWS: Smithy GetChannelMessage.ChannelArn is a required @httpLabel ChimeArn; BadRequestException(400) is modelled. Recording: HTTP 400, Code 'BadRequest', 'An invalid Channel ARN was supplied for the input parameter.'
Emulator: HTTP 403, Code 'Forbidden', 'You do not have sufficient access to perform this action.', x-amzn-errortype 'ForbiddenException'.
Why: as cause 1/3.
Fix: GetChannelMessage's input validation in its handler.
Traces: chime-sdk-messaging-get-channel-message-malformed-requests-rejected#task#9.

Cause 6 of 10: GetChannelMessage with an invalid x-amz-chime-bearer is SERVED, not refused.
AWS: ChimeBearer is required and UnauthorizedClientException(401) is modelled. Recording: HTTP 401, Code 'Unauthorized', 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.'
Emulator: HTTP 200 with the channel message body — no Code, no Message, no x-amzn-errortype.
Why: this is a read of message content on behalf of a bearer the service cannot resolve. A client (or a test of an authorization boundary) that expects the read to be refused sees it succeed against the emulator and fail in production, and any negative test of bearer handling silently passes.
Fix: GetChannelMessage's handler must run the shared ChimeBearer resolution before reading message state, and raise UnauthorizedClientException(401) when the bearer names no AppInstanceUser.
Traces: chime-sdk-messaging-get-channel-message-malformed-requests-rejected#task#10.

Cause 7 of 10: ListChannelBans with a ChannelArn that is not a channel ARN.
AWS: Smithy ListChannelBans.ChannelArn is a required @httpLabel ChimeArn; BadRequestException(400) modelled. Recording: HTTP 400, Code 'BadRequest', 'An invalid Channel ARN was supplied for the input parameter.'
Emulator: HTTP 403, Code 'Forbidden', 'You do not have sufficient access to perform this action.', x-amzn-errortype 'ForbiddenException'.
Why: as cause 1/3.
Fix: ListChannelBans' input validation in its handler.
Traces: chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected#task#5.

Cause 8 of 10: ListChannelsAssociatedWithChannelFlow with a ChannelFlowArn that is well-formed but is not a channel-flow ARN.
AWS: Smithy models ChannelFlowArn as a required @httpQuery('channel-flow-arn') ChimeArn and declares BadRequestException(400). Recording: HTTP 400, Code 'BadRequest', and the fixed message here is the GENERIC form 'An invalid ARN was supplied for the input parameter.' (this operation does not name the member in the sentence).
Emulator: HTTP 403, Code 'Forbidden', Message 'An invalid App Instance ARN was supplied for the input parameter.', x-amzn-errortype 'ForbiddenException' — wrong status, wrong code, and a sentence naming an App Instance ARN this operation has no member for.
Why: as cause 1, plus the operator is sent looking for an AppInstanceArn parameter that does not exist on this API.
Fix: ListChannelsAssociatedWithChannelFlow's input validation in its handler.
Traces: chime-sdk-messaging-list-channels-associated-with-channel-flow-rejected#task#4 (and #task#5).

Cause 9 of 10: SearchChannels with an absent/unusable x-amz-chime-bearer is refused as a bad request instead of an unauthorized client.
AWS: Smithy models SearchChannels.ChimeBearer as an OPTIONAL @httpHeader('x-amz-chime-bearer') ChimeArn, and declares UnauthorizedClientException(401). Recording: HTTP 401, Code 'Unauthorized', 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.' — i.e. AWS treats a bearer it cannot resolve as an identity problem, not a schema problem, and does not claim the header is 'required'.
Emulator: HTTP 400, Code 'BadRequest', Message 'the x-amz-chime-bearer header is required', x-amzn-errortype 'BadRequestException'. The message is emulator-authored prose (lower-case, no trailing period) that appears nowhere in AWS's vocabulary, and it contradicts the model, which does not mark the member @required.
Fix: SearchChannels' handler must route an unresolvable bearer through the shared ChimeBearer resolution to UnauthorizedClientException(401)/Code 'Unauthorized' with AWS's sentence, instead of synthesising a required-header BadRequest.
Traces: chime-sdk-messaging-search-channels-malformed-fields-rejected#task#7.

Cause 10 of 10: SearchChannels accepts a Fields[].Values entry that is not an AppInstanceUser ARN and answers 200.
AWS: Smithy models SearchChannels.Fields as a required SearchFields list (@length 1..20) of SearchField{Key: SearchFieldKey enum (MEMBERS), Values: SearchFieldValues (@length 1..20), Operator: SearchFieldOperator}; the Values member is documented 'The values must be AppInstanceUserArns specified as a list of strings.' BadRequestException(400) is modelled. Recording: HTTP 400, Code 'BadRequest', fixed message 'An invalid ARN was supplied for the input parameter.'
Emulator: HTTP 200 with a channel result body — no Code, no Message, no x-amzn-errortype — so a search keyed on a value that cannot be a member is answered as a successful search.
Why: a caller that mis-built or truncated the member ARN is told the search ran, and reads the (empty or unrelated) result as authoritative membership truth; reconciliation that deletes or recreates channels on 'no match' acts on a result AWS would never have returned.
Fix: SearchChannels' input validation in its handler — validate each Fields[].Values entry as an AppInstanceUser ARN before searching, and refuse with BadRequestException(400) and that sentence.
Traces: chime-sdk-messaging-search-channels-malformed-fields-rejected#task#4 (and #task#5, #task#8).

## Evidence
Replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout (score 81.2%, 45 mismatched steps). Expected values are the recorded AWS responses; actual values are the emulator's.

Cause 1 — AssociateChannelFlow (fixture chime-sdk-messaging-associate-channel-flow-non-flow-arn-rejected, task#4, cluster also covers task#5):
  status: expected 400, got 403
  $.Code: expected 'BadRequest', got 'Forbidden'
  $.Message: expected 'An invalid Channel ARN was supplied for the input parameter.', got 'An invalid App Instance ARN was supplied for the input parameter.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 2 — AssociateChannelFlow (chime-sdk-messaging-associate-channel-flow-unknown-flow-not-found, task#4):
  status: expected 404, got 403
  $.Code: expected 'NotFound', got 'Forbidden'
  $.Message: expected 'The specified channel flow was not found.', got 'An invalid App Instance ARN was supplied for the input parameter.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 3 — ListChannelModerators (chime-sdk-messaging-channel-moderators-list-invalid-input-rejected, task#8):
  status: expected 400, got 403
  $.Code: expected 'BadRequest', got 'Forbidden'
  $.Message: expected 'An invalid Channel ARN was supplied for the input parameter.', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 4 — ListChannelModerators (same fixture, task#9):
  status: expected 401, got 403
  $.Code: expected 'Unauthorized', got 'Forbidden'
  $.Message: expected 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 5 — GetChannelMessage (chime-sdk-messaging-get-channel-message-malformed-requests-rejected, task#9):
  status: expected 400, got 403
  $.Code: expected 'BadRequest', got 'Forbidden'
  $.Message: expected 'An invalid Channel ARN was supplied for the input parameter.', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 6 — GetChannelMessage (same fixture, task#10):
  status: expected 401, got 200
  $.Code: expected 'Unauthorized', got None (absent)
  $.Message: expected 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.', got None (absent)
  $headers.x-amzn-errortype: expected present, got None

Cause 7 — ListChannelBans (chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected, task#5):
  status: expected 400, got 403
  $.Code: expected 'BadRequest', got 'Forbidden'
  $.Message: expected 'An invalid Channel ARN was supplied for the input parameter.', got 'You do not have sufficient access to perform this action.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 8 — ListChannelsAssociatedWithChannelFlow (chime-sdk-messaging-list-channels-associated-with-channel-flow-rejected, task#4; cluster also covers task#5):
  status: expected 400, got 403
  $.Code: expected 'BadRequest', got 'Forbidden'
  $.Message: expected 'An invalid ARN was supplied for the input parameter.', got 'An invalid App Instance ARN was supplied for the input parameter.'
  $headers.x-amzn-errortype: got 'ForbiddenException'

Cause 9 — SearchChannels (chime-sdk-messaging-search-channels-malformed-fields-rejected, task#7):
  status: expected 401, got 400
  $.Code: expected 'Unauthorized', got 'BadRequest'
  $.Message: expected 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.', got 'the x-amz-chime-bearer header is required'
  $headers.x-amzn-errortype: got 'BadRequestException'

Cause 10 — SearchChannels (same fixture, task#4; cluster also covers task#5 and task#8):
  status: expected 400, got 200
  $.Code: expected 'BadRequest', got None (absent)
  $.Message: expected 'An invalid ARN was supplied for the input parameter.', got None (absent)
  $headers.x-amzn-errortype: expected present, got None

In each refusal message the entire sentence is fixed text — AWS interpolates no request value into any of these four sentences ('An invalid Channel ARN was supplied for the input parameter.', 'An invalid ARN was supplied for the input parameter.', 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.', 'The specified channel flow was not found.'), so there is nothing environment-specific to reproduce.

Related but NOT covering: the open behavioral issue 20261127T195731Z-chime-sdk-messaging-3-net-new-causes-pagination.md describes a sibling symptom (a pattern-valid AppInstanceArn served an empty page by ListChannels/ListChannelFlows) but carries no wire replay key and names none of the operations above.

## Steps to Reproduce
Region us-east-1 (Chime SDK messaging is served from us-east-1 / eu-central-1 only). Create an AppInstance and an AppInstanceUser via chime-sdk-identity, then a channel and a channel flow via chime-sdk-messaging; use the AppInstanceUser ARN as x-amz-chime-bearer unless a cause says otherwise.

1. AssociateChannelFlow(ChannelArn=<the AppInstance ARN, i.e. a well-formed ARN that is not a channel>, ChannelFlowArn=<the real flow>) -> expect 400 / Code 'BadRequest' / 'An invalid Channel ARN was supplied for the input parameter.'; emulator answers 403 'Forbidden'.
2. AssociateChannelFlow(ChannelArn=<the real channel>, ChannelFlowArn=<a well-formed channel-flow ARN with a random uuid that was never created>) -> expect 404 / 'NotFound' / 'The specified channel flow was not found.'; emulator answers 403 'Forbidden'.
3. ListChannelModerators(ChannelArn=<a well-formed non-channel ARN>) -> expect 400 / 'BadRequest' / 'An invalid Channel ARN was supplied for the input parameter.'
4. ListChannelModerators(ChannelArn=<the real channel>, x-amz-chime-bearer=<a well-formed ARN that is not an AppInstanceUser>) -> expect 401 / 'Unauthorized' / 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.'
5. GetChannelMessage(ChannelArn=<a well-formed non-channel ARN>, MessageId=<a sent message id>) -> expect 400 / 'BadRequest' / 'An invalid Channel ARN was supplied for the input parameter.'
6. GetChannelMessage(ChannelArn=<the real channel>, MessageId=<a sent message id>, x-amz-chime-bearer=<a well-formed ARN that is not an AppInstanceUser>) -> expect 401 / 'Unauthorized'; the emulator returns 200 and the message body.
7. ListChannelBans(ChannelArn=<a well-formed non-channel ARN>) -> expect 400 / 'BadRequest' / 'An invalid Channel ARN was supplied for the input parameter.'
8. ListChannelsAssociatedWithChannelFlow(ChannelFlowArn=<a well-formed ARN that is not a channel flow>) -> expect 400 / 'BadRequest' / 'An invalid ARN was supplied for the input parameter.'
9. SearchChannels(Fields=[{Key:'MEMBERS',Values:[<the AppInstanceUser ARN>],Operator:'INCLUDES'}]) with NO x-amz-chime-bearer header -> expect 401 / 'Unauthorized' / 'An invalid app instance user ARN was supplied for the x-amz-chime-bearer HTTP header.'; emulator answers 400 'the x-amz-chime-bearer header is required'.
10. SearchChannels(Fields=[{Key:'MEMBERS',Values:[<a well-formed ARN that is not an AppInstanceUser, e.g. the AppInstance ARN>],Operator:'INCLUDES'}], x-amz-chime-bearer=<the AppInstanceUser>) -> expect 400 / 'BadRequest' / 'An invalid ARN was supplied for the input parameter.'; emulator answers 200 with a channel list.

Or replay the bundled fixtures directly: chime-sdk-messaging-associate-channel-flow-non-flow-arn-rejected, -associate-channel-flow-unknown-flow-not-found, -channel-moderators-list-invalid-input-rejected, -get-channel-message-malformed-requests-rejected, -list-channel-bans-malformed-channel-arn-rejected, -list-channels-associated-with-channel-flow-rejected, -search-channels-malformed-fields-rejected.

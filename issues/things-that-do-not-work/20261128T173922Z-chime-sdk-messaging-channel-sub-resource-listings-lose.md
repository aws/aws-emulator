Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-batch-channel-membership-duplicate-and-sub-channel-rejected
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-batch-channel-membership-duplicate-and-sub-channel-rejected chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|ListChannelMemberships|2xx->2xx|$.ChannelMemberships[*]
Issue-Key: chime-sdk-messaging|ListChannelBans|4xx->4xx|$.message
Issue-Related: 20261128T173922Z-chime-sdk-messaging-input-arn-and-chimebearer-refusals.md
Issue-Commit: 757e19016492578f68fb6ad3f54c4ad74c32c8c1

# chime-sdk-messaging channel sub-resource listings lose batch-added memberships and misreport a bad pagination token

> _Note: this issue was identified against the code base at commit `757e19016492578f68fb6ad3f54c4ad74c32c8c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannelMemberships

## Description
Two net-new wire divergences on the channel sub-resource listing paths: one loses state that a preceding batch write should have created, the other refuses a bad cursor with the wrong error class and sentence. Both causes are numbered below and each carries its own fixture and step; both must be closed before this issue is submitted.

Cause 1 of 2: ListChannelMemberships reports only the channel creator after a batch membership add — the batch-added members are missing from the page.
AWS: Smithy models ListChannelMemberships output as {ChannelArn, ChannelMemberships: ChannelMembershipSummaryList, NextToken}, each summary projecting Member (Identity{Arn, Name}) and ReadMarkerTimestamp. The recording shows the listing for a channel whose memberships were added as a batch returning the creator PLUS one summary per successfully added member, each with Member.Arn = that AppInstanceUser's ARN, Member.Name = that user's name, and ReadMarkerTimestamp absent — i.e. a successful batch add is immediately visible to this read.
Emulator: HTTP 200 but with only the first membership present; $.ChannelMemberships[1] and $.ChannelMemberships[2] are absent entirely, so the two batch-added members (named 'alpha' and 'beta' in the recording) never appear.
Why: membership is the authorization and fan-out basis for a channel. A client that adds members as a batch and then lists to confirm sees them missing, so it re-adds them, reports a failed provision, or concludes the users are not in the channel — and message delivery / moderation logic driven by the membership list operates on an under-populated set.
Fix: the channel-membership state write reached by the batch membership-add handler, and/or ListChannelMemberships' projection of that state — the memberships a successful batch add reports must be committed to the same store this listing reads. Note the open issue 20261127T195731Z-chime-sdk-messaging-batchcreatechannelmembership-stub.md ('Implement chime-sdk-messaging BatchCreateChannelMembership: unimplemented scaffold stub') is a plausible upstream, but it does NOT cover this divergence: the replay attributes this failure to ListChannelMemberships and carries no stub or owner marker for it, so the read-back must be verified against this fixture once the write path exists.
Traces: chime-sdk-messaging-batch-channel-membership-duplicate-and-sub-channel-rejected#task#10.

Cause 2 of 2: ListChannelBans refuses a NextToken it never issued with a synthetic BadRequestException sentence instead of AWS's ValidationException pagination-token refusal.
AWS: this refusal is framework-level and NOT in the Smithy model — ListChannelBans.NextToken is modelled only as a @sensitive @httpQuery('next-token') string (@length 0..2048, pattern '.*'), and the operation declares only BadRequestException(400)/ForbiddenException(403)/UnauthorizedClientException(401)/ThrottledClientException(429)/ServiceFailureException(500)/ServiceUnavailableException(503); so this is recorded behavior only. The recording shows AWS answering error code 'ValidationException' with the framework 'message' member of the fixed form "Pagination token exception in operation '<OperationName>': <reason>". Fixed text: "Pagination token exception in operation '", then the operation name AS INVOKED ('ListChannelBans' here), then "': ", then a reason determined by HOW the token fails — a token that decodes but whose leading two bytes read as an unknown envelope id yields 'invalid payload wrapping schema <n>', where <n> is that big-endian two-byte number taken from the SUPPLIED token (an internal schema number, never a fixed literal).
Emulator: error code 'BadRequestException' (x-amzn-errortype 'BadRequestException') with the single emulator-authored sentence 'NextToken is not a token this service issued' — no operation name and no reason clause, and the wrong error class.
Why: a client that branches on the error code to tell a stale/forged cursor from a bad request never takes the ValidationException branch, so resume-from-scratch and retry logic never fires; log scraping or alarms keyed on the "Pagination token exception in operation ..." sentence never match; and an operator cannot tell a tampered token from an undecryptable one.
Fix: the shared pagination-token decode/validate path the listing handlers reach — classify the decode failure and raise the service's ValidationException with the invoked operation name and the matching reason clause, rather than one BadRequestException sentence for every operation and every failure mode. A genuine token the service issued must still page normally.
Related but NOT covering: the open behavioral issue 20261127T195731Z-chime-sdk-messaging-3-net-new-causes-pagination.md describes this same pagination-token cause from the behavioral suite, but it carries only behavioral check ids and no wire replay key, so this fixture step is not gated by it and must be re-replayed here.
Traces: chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected#task#7.

## Evidence
Replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout.

Cause 1 — ListChannelMemberships (fixture chime-sdk-messaging-batch-channel-membership-duplicate-and-sub-channel-rejected, task#10), HTTP 200 on both sides:
  $.ChannelMemberships[1]: expected {'Member': {'Arn': '<app-instance>/user/<alpha user>', 'Name': 'alpha'}, 'ReadMarkerTimestamp': None}, got None (element absent)
  $.ChannelMemberships[2]: expected {'Member': {'Arn': '<app-instance>/user/<beta user>', 'Name': 'beta'}, 'ReadMarkerTimestamp': None}, got None (element absent)
  (element [0] matched). The ARNs in the recording embed that run's generated app-instance and user ids and are not themselves requirements; the requirement is that one summary per successfully batch-added member is present, carrying that member's ARN and Name.

Cause 2 — ListChannelBans (fixture chime-sdk-messaging-list-channel-bans-malformed-channel-arn-rejected, task#7), HTTP 400 on both sides:
  $.message: expected "Pagination token exception in operation 'ListChannelBans': invalid payload wrapping schema -24949"
              got      'NextToken is not a token this service issued'
  $headers.x-amzn-errortype: got 'BadRequestException' (AWS renders the ValidationException class here)
  The number -24949 is the two-byte big-endian envelope id decoded from the token that fixture supplied — it is a function of the submitted token, not a literal to hard-code. Fixed text is "Pagination token exception in operation '" / "': " / 'invalid payload wrapping schema '.

## Steps to Reproduce
Region us-east-1. Create an AppInstance and three AppInstanceUsers (chime-sdk-identity): a creator and two members named 'alpha' and 'beta'. Create a standard channel as the creator and use the creator's ARN as x-amz-chime-bearer.

Cause 1: add 'alpha' and 'beta' to the channel in one batch membership call (BatchCreateChannelMembership with both ARNs, Type DEFAULT), confirming the response reports both as created and none in Errors. Then GET /channels/{ChannelArn}/memberships (ListChannelMemberships). The page must contain three summaries — the creator plus one each for 'alpha' and 'beta', each with Member.Arn = that user's ARN and Member.Name = 'alpha'/'beta' and no ReadMarkerTimestamp. The emulator returns only the first summary.

Cause 2: ban a member so the listing is non-empty, then GET /channels/{ChannelArn}/bans?max-results=1&next-token=<a base64 string this service never issued, e.g. base64 of 'page-cursor-never-minted'>. AWS answers HTTP 400 with error code 'ValidationException' and message "Pagination token exception in operation 'ListChannelBans': invalid payload wrapping schema <n>", where <n> is the two-byte big-endian value decoded from the supplied token; the emulator answers 'BadRequestException' / 'NextToken is not a token this service issued'. Then repeat with the genuine NextToken the first page returned: it must still page normally.

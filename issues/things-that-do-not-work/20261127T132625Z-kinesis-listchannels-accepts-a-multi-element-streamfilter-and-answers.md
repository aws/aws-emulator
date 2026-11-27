Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-list-channels-stream-filter-size-bounds
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-list-channels-stream-filter-size-bounds
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|ListChannels|4xx->2xx|$.__type
Issue-Key: kinesis|ListChannels|4xx->2xx|$.message
Issue-Key: kinesis|ListChannels|4xx->2xx|status
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# ListChannels accepts a multi-element StreamFilter and answers 200 instead of ValidationException

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
=== Cause 1 of 1: ListChannels does not enforce the one-element limit on StreamFilter ===
AWS: Smithy: ListChannels.StreamFilter targets StreamFilterList with @length min 1 max 10000, and StreamFilter is a structure whose StreamARN is @required; ListChannels binds ValidationException, InvalidArgumentException, AccessDeniedException, ExpiredNextTokenException and LimitExceededException (all client, HTTP 400), and is @paginated/@readonly. The modelled list maximum is therefore NOT the operative limit — AWS applies a stricter runtime rule on top of it. Recorded behavior: a ListChannels request whose StreamFilter carries more than one element is refused HTTP 400, __type "ValidationException", message "StreamFilter must contain at most one element." — entirely fixed text, with no count, member value or stream ARN interpolated. (The same fixture's neighbouring step shows the lower bound being enforced, so both ends of this member's rule are checked by the service.)
Emulator: HTTP 200 with a normal ListChannels result body and no error at all — the request is accepted and the surplus filter elements are silently absorbed, so the response is a listing computed under filter semantics AWS never offers.
Why: this is a wrong SUCCESS, the worst shape of divergence for a client: code that passes two filters gets back a channel list the emulator chose how to filter (first element only, union, or unfiltered) and treats it as authoritative, so a caller validates a query against the emulator that AWS will refuse outright in production, and any decision made from the returned ChannelSummaries is based on a filter the service would not have applied.
Fix: ListChannels' input validation — reject a StreamFilter with more than one element with ValidationException "StreamFilter must contain at most one element." before the listing is computed, alongside the existing min-length check.
Traces: kinesis-list-channels-stream-filter-size-bounds#task#4.

## Evidence
kinesis-list-channels-stream-filter-size-bounds#task#4, ListChannels: status expected 400, got 200; $.__type missing — expected 'ValidationException', got None; $.message missing — expected 'StreamFilter must contain at most one element.', got None. The request carried a StreamFilter of more than one element; the emulator returned a successful ListChannels body instead of refusing. The adjacent step #task#5 of the same fixture shows the empty-list (min length) case is refused, so only the upper bound is unenforced.

## Steps to Reproduce
1. Create two streams and (optionally) a channel on each so the listing is non-empty.
2. kinesis.list_channels(StreamFilter=[{'StreamARN': <arn of stream A>}, {'StreamARN': <arn of stream B>}]) — expect HTTP 400 with __type 'ValidationException' and message 'StreamFilter must contain at most one element.'; observe HTTP 200 with a ChannelSummaries body.
3. For contrast, kinesis.list_channels(StreamFilter=[{'StreamARN': <arn of stream A>}]) must still succeed, and kinesis.list_channels(StreamFilter=[]) must still be refused for the min-length rule.
4. Re-replay kinesis-list-channels-stream-filter-size-bounds#task#4.

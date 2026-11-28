Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.list-retained-messages-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.list-retained-messages-unissued-next-token-rejected|run#10|AssertionError|assert got == (EXPECTED_CODE, EXPECTED_MESSAGE), (
Issue-Commit: 4866adc0993f8fa67cc0ae020b08b6d4f21694d6

# iot-data ListRetainedMessages refuses an unissued nextToken with an emulator-authored message instead of AWS's "unknown"

> _Note: this issue was identified against the code base at commit `4866adc0993f8fa67cc0ae020b08b6d4f21694d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRetainedMessages

## Description
1 net-new cause in iot-data's ListRetainedMessages pagination-token guard. Related but NOT covered by open issue 20261128T002112Z-iot-data-request-validation-refusals-carry-generic-or.md: that issue's ListRetainedMessages causes are all about the `maxResults` query parameter (out-of-range and non-numeric values yielding "Bad Request" or a leaked Rust deserializer string). This cause is the `nextToken` parameter, whose refusal already carries the correct error class and status and fails only on the message text, so it is a distinct defect in a distinct guard.

Cause 1 of 1: the nextToken refusal message is emulator-authored and echoes the rejected token.
AWS: Smithy: ListRetainedMessages.nextToken targets NextToken as an @httpQuery, and the operation declares InvalidRequestException with errorKind client / httpError 400 (GET /retainedMessage). A nextToken the service never issued is refused 400 InvalidRequestException with the message `unknown` — recorded behavior: that whole string is the fixed text of this error class, it has no variable part, and in particular AWS does NOT echo the rejected token back. AWS deliberately says nothing about the opaque cursor's contents.
Emulator: refuses with the right code and status (InvalidRequestException) but the message `the nextToken '<token>' is not valid` — emulator-authored prose that also interpolates the raw token value from the request. Replay detail: expected ('InvalidRequestException', 'unknown'), got ('InvalidRequestException', "the nextToken 'c2ltMnJlYWwtZm9yZ2VkLXBhZ2UtY3Vyc29y' is not valid") — the base64-looking string there is just the forged token the check happened to send, it is not part of the requirement.
Why: a client that branches on, logs or asserts the refusal message for a stale/forged page cursor sees text AWS never produces, so message-matching retry and paging logic (and any test pinned to AWS's wording) breaks even though the status and error code line up. Echoing the token back also leaks the caller's opaque cursor into an error body AWS keeps contentless.
Fix: the nextToken validation in the ListRetainedMessages handler — raise the modeled InvalidRequestException carrying exactly the fixed message `unknown`, instead of formatting a message from the submitted token. The positive control in the same check passes, so the cursor-issuing and cursor-accepting paths are correct and the listing stays usable after the refusal; only the refusal's message body diverges.
Traces: iot-data.list-retained-messages-unissued-next-token-rejected.

## Evidence
Failing check iot-data.list-retained-messages-unissued-next-token-rejected (mismatch):

  AssertionError: an unissued nextToken should be refused with ('InvalidRequestException', 'unknown'), got ('InvalidRequestException', "the nextToken 'c2ltMnJlYWwtZm9yZ2VkLXBhZ2UtY3Vyc29y' is not valid")

Expected (AWS): HTTP 400, x-amzn-errortype InvalidRequestException, $.message == "unknown" (entirely fixed text; no request- or state-derived part).
Actual (emulator): HTTP 400, x-amzn-errortype InvalidRequestException, $.message == "the nextToken '<the token from the request>' is not valid" (fixed text "the nextToken '" + "' is not valid" with the request's nextToken interpolated).

The check's positive control passed: a nextToken the emulator itself issued on a maxResults=1 page is accepted and returns one entry, and the first page still answers after the refusal. Only the refusal message diverges.

Smithy (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/smithy-model/com-amazonaws-iotdataplane-iotmoonrakerservice.json): ListRetainedMessages http GET /retainedMessage code 200; input nextToken -> NextToken @httpQuery("nextToken"), maxResults -> MaxResults @httpQuery @range(min 1, max 200); errors include InvalidRequestException (client, httpError 400).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an iot-data client at the data-plane endpoint.
2. Retain two messages so the listing paginates: Publish with retain=true to two distinct topics under one prefix.
3. GET /retainedMessage?maxResults=1 repeatedly, following nextToken, until both retained topics have been listed; keep the nextToken from the first page.
4. GET /retainedMessage?maxResults=1&nextToken=<that issued token> -> succeeds with one entry (positive control; passes today).
5. GET /retainedMessage?maxResults=1&nextToken=<any token the service never issued> -> AWS answers 400 InvalidRequestException with $.message exactly "unknown"; the emulator answers 400 InvalidRequestException with $.message "the nextToken '<submitted token>' is not valid".
6. GET /retainedMessage?maxResults=1 -> still answers one entry (passes today).

Replay check: iot-data.list-retained-messages-unissued-next-token-rejected

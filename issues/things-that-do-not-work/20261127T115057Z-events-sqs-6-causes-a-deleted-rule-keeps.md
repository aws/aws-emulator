Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.deleted-rule-stops-delivering-while-a-sibling-rule-on-the-same-pattern-keeps-delivering events__sqs.fifo-target-empty-message-group-id-rejected events__sqs.input-paths-map-key-over-256-characters-rejected events__sqs.put-targets-input-paths-map-key-outside-pattern-rejected events__sqs.remove-targets-oversized-id-rejected-and-target-keeps-delivering events__sqs.put-events-resource-over-2048-characters-rejected events__sqs.input-paths-map-value-over-256-rejected-and-256-delivers events__sqs.input-paths-map-over-one-hundred-entries-rejected-while-one-hundred-render-into-the-message events__sqs.put-targets-input-paths-map-over-100-entries-rejected events__sqs.input-template-placeholder-is-never-rendered-as-an-object-key
Issue-Key: events__sqs.deleted-rule-stops-delivering-while-a-sibling-rule-on-the-same-pattern-keeps-delivering|run#35|TimeoutError|raise TimeoutError(
Issue-Key: events__sqs.fifo-target-empty-message-group-id-rejected|_refused#15|AssertionError|raise AssertionError(f"PutTargets accepted the target: {out!r}")
Issue-Key: events__sqs.input-paths-map-key-over-256-characters-rejected|_refusal#17|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: events__sqs.put-targets-input-paths-map-key-outside-pattern-rejected|_refusal#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events__sqs.remove-targets-oversized-id-rejected-and-target-keeps-delivering|run#23|AssertionError|assert got == (
Issue-Key: events__sqs.put-events-resource-over-2048-characters-rejected|run#22|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: events__sqs.input-paths-map-value-over-256-rejected-and-256-delivers|run#19|AssertionError|assert (code, message) == expected, f"257-character path gave {(code, message)!r}"
Issue-Key: events__sqs.input-paths-map-over-one-hundred-entries-rejected-while-one-hundred-render-into-the-message|run#21|AssertionError|assert sorted(echoed) == sorted(sent_paths), (
Issue-Key: events__sqs.put-targets-input-paths-map-over-100-entries-rejected|run#17|AssertionError|assert message.startswith(_HEAD) and message.endswith(_TAIL), (
Issue-Key: events__sqs|PutTargets|ValidationException
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__sqs: 6 causes — a deleted rule keeps delivering, four PutTargets constraint violations are accepted, and the collection-constraint and InputTemplate refusal messages are malformed

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 6: a deleted rule keeps delivering. AWS: once DeleteRule returns, that rule matches nothing further — fresh events reach only the surviving sibling rule on the same pattern. Smithy: DeleteRule's only resource error is ResourceNotFoundException; a successful delete is final. Emulator: `no 30 s window passed in which the deleted rule's queue received none of the fresh events` — the deleted rule's target keeps receiving. Checks: events__sqs.deleted-rule-stops-delivering-while-a-sibling-rule-on-the-same-pattern-keeps-delivering.

Cause 2 of 6: PutTargets accepts an SQS FIFO target with an EMPTY MessageGroupId. AWS: refuses with a 400 ValidationException — a FIFO queue target must carry a non-empty SqsParameters.MessageGroupId (the model allows min length 0, so this is a handler rule; recorded behavior for the refusal). Emulator: `PutTargets accepted the target: {'FailedEntryCount': 0, 'FailedEntries': []}`. Checks: events__sqs.fifo-target-empty-message-group-id-rejected.

Cause 3 of 6: PutTargets accepts an InputPathsMap KEY over 256 characters, and one outside the allowed pattern. AWS: refuses with a 400 ValidationException quoting the member and constraint — Smithy: TransformerPaths key targets InputTransformerPathKey @length 1..256 pattern ^[A-Za-z0-9\_\-]+$. Emulator: returns 200 with FailedEntryCount 0 in both cases. Checks: events__sqs.input-paths-map-key-over-256-characters-rejected, events__sqs.put-targets-input-paths-map-key-outside-pattern-rejected.

Cause 4 of 6: constraint-violation messages for COLLECTION members are rendered against the individual element, with the value elided and the compound constraint reduced to one clause. AWS reports the violation at the COLLECTION path, echoing the offending collection and listing both bounds, e.g. RemoveTargets: `1 validation error detected: Value '[<id>]' at 'ids' failed to satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1]`; PutEvents resources: `... at 'entries.1.member.resources' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 2048, Member must have length greater than or equal to 0]`; InputPathsMap value: `Value '{amount=<path>}' at 'targets.1.member.inputTransformer.inputPathsMap' failed to satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0]`; and an over-100-entry map echoes the MAP ITSELF (`Value '{<entries>}'`), not a count. Smithy backs the limits: TargetId @length 1..64, EventResourceList member @length ≤2048, TransformerPaths @length 0..100 with value @length 0..256. The fixed text is AWS's template; the bracketed value, the member path and the limits are taken from the request and the model. Emulator: reports `at 'ids.1.member'` / `at 'entries.1.member.resources.1.member'` / `at '...inputPathsMap.amount'` with a single max clause, and renders the oversized map as `Value '101 entries'`. Checks: events__sqs.remove-targets-oversized-id-rejected-and-target-keeps-delivering, events__sqs.put-events-resource-over-2048-characters-rejected, events__sqs.input-paths-map-value-over-256-rejected-and-256-delivers, events__sqs.input-paths-map-over-one-hundred-entries-rejected-while-one-hundred-render-into-the-message, events__sqs.put-targets-input-paths-map-over-100-entries-rejected.

Cause 5 of 6: the InputTemplate refusal for a placeholder used as an object KEY is emulator-authored. AWS: ValidationException `Invalid InputTemplate for target <target-id> : [Source: (String)"<the template>"; line: <n>, column: <n>].` — the documented restriction is "the placeholder cannot be used as an object key" (Smithy InputTransformer.InputTemplate docs), and the message quotes the template with the parser's line/column. Emulator: `InputTemplate for target key is not valid JSON.`. Checks: events__sqs.input-template-placeholder-is-never-rendered-as-an-object-key.

Why: cause 1 is a correctness disaster — a client that deletes a rule to stop a flow (for cost, privacy or incident reasons) keeps receiving and paying for deliveries, and no API call reveals the rule; causes 2-3 store target configurations real EventBridge refuses, so the failure moves to delivery time; causes 4-5 break every client that matches or displays AWS's validation text and hide which member and which bound was violated.
Fix: the events DeleteRule handler/rule-index removal (the rule must leave the matching set), the PutTargets/PutEvents validation layer (FIFO MessageGroupId, InputPathsMap key length/pattern), the shared constraint-message renderer for list/map members (collection path, echoed collection value, compound constraint), and the InputTemplate validator's message.
Traces: the ten check ids listed per cause above.

## Evidence
Cause 1: `TimeoutError: no 30 s window passed in which the deleted rule's queue received none of the fresh events`
Causes 2-3: `PutTargets accepted the target: {'FailedEntryCount': 0, 'FailedEntries': []}` / `the call was expected to be refused but succeeded: {'FailedEntryCount': 0, ...}`
Cause 4: `RemoveTargets with a 65-character id answered ('ValidationException', "...Value '<id>' at 'ids.1.member' failed to satisfy constraint: Member must have length less than or equal to 64")` vs AWS `Value '[<id>]' at 'ids' ... constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1]`; `the refusal does not read "...Value '{"<entries>"}' at '...inputPathsMap'..."; it reads "...Value '101 entries' at '...inputPathsMap'..."`; the 2049-char resource message ends `at 'entries.1.member.resources.1.member'` instead of `at 'entries.1.member.resources'` with the compound constraint.
Cause 5: expected ('ValidationException', 'Invalid InputTemplate for target key : [Source: (String)"{null: "v"}"; line: 1, column: 2].'), got ('ValidationException', 'InputTemplate for target key is not valid JSON.')

## Steps to Reproduce
1. Two rules on one bus with the same pattern, each with its own queue target; DeleteRule one of them; PutEvents and watch the deleted rule's queue keep receiving.
2. PutTargets naming a FIFO queue with SqsParameters.MessageGroupId="" — observe 200.
3. PutTargets with an InputPathsMap key of 257 chars, and with a key containing a character outside ^[A-Za-z0-9_\-]+$ — observe 200.
4. RemoveTargets with a 65-character id; PutEvents with a 2049-character resources entry; PutTargets with a 257-character InputPathsMap value and with 101 entries — compare each message against AWS's collection-path form.
5. PutTargets with InputTemplate `{<k>: "v"}` (placeholder as object key) — compare the message.

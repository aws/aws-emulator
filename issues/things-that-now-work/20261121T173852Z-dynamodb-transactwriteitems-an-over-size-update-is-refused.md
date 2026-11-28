Issue-Source: trace-sweep-divergence
Issue-Trace: transact-write-items-update-over-size-limit-cancellation-reason
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transact-write-items-update-over-size-limit-cancellation-reason
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.CancellationReasons
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.Message
Issue-Related: 20261111T221143Z-dynamodb-condition-key-and-filter-expression-defects-are.md
Issue-Commit: c00b6f0fe4a56f65b06b651f01dc2848e60dcf79

# dynamodb TransactWriteItems: an over-size Update is refused as ValidationException instead of a TransactionCanceledException carrying CancellationReasons

> _Note: this issue was identified against the code base at commit `c00b6f0fe4a56f65b06b651f01dc2848e60dcf79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
One cause.

Cause 1 of 1: a TransactWriteItems whose Update member drives the target item past the 400 KB item-size limit is answered as a request-level ValidationException instead of the transaction-cancellation envelope.

AWS: when a change made BY the transaction pushes an item over the item-size limit, DynamoDB cancels the transaction rather than refusing the request. Smithy: TransactWriteItems errors include TransactionCanceledException (errorKind client, httpError 400), whose documentation lists as a cancel circumstance "An item size becomes too large (larger than 400 KB), or a local secondary index (LSI) becomes too large, or a similar validation error occurs because of changes made by the transaction". TransactionCanceledException models Message plus CancellationReasons (CancellationReasonList, @length min 1 max 100); CancellationReason models Code, Message and Item, and is documented as "An ordered list of errors for each item in the request ... ordered according to the ordering of the TransactWriteItems request parameter. If no error occurred for the associated item an error with a Null code and Null message will be present." The recorded response is HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#TransactionCanceledException', top-level Message 'Transaction cancelled, please refer cancellation reasons for specific reasons [<comma-separated per-slot codes>]' — the sentence 'Transaction cancelled, please refer cancellation reasons for specific reasons' is fixed text and the bracketed list is a value derived from the request, one code per TransactItems slot in request order (here the single slot yields '[ValidationError]') — and CancellationReasons with that slot's entry {Code: 'ValidationError', Message: 'Item size to update has exceeded the maximum allowed size'}. In that per-slot Message the whole sentence is fixed text (the error class for an over-size update); the per-slot Code is the class of that slot's defect, and the slots that did not fail carry Code 'None' with no Message. Error class and texts are recorded behavior; the status (400) and the cancel circumstance are modeled.

Emulator: the same request is answered HTTP 400 with __type 'com.amazon.coral.validate#ValidationException'. The reply carries no Message member and no CancellationReasons member at all (both are absent, not empty), so the per-slot verdict AWS returns is unavailable and the failure is reported as a flat request-validation refusal under a different error class and a different member name for the text.

Why: a client that branches on the error class gets the wrong verdict for an over-size transactional update. TransactionCanceledException means the transaction as submitted was rejected as a unit and the per-slot CancellationReasons say which action to fix (and which siblings are blameless, Code 'None'); ValidationException means the request itself is malformed. An SDK's transaction helper, which unpacks CancellationReasons to attribute the failure to a specific TransactItems slot and to decide retry versus shrink, sees no CancellationReasons list and cannot identify the offending action — in a multi-action transaction it cannot tell which item to shrink. Code that catches TransactionCanceledException to inspect reasons never enters that branch at all, and the generic handler it falls into sees an error with no message member.

Fix: the TransactWriteItems handler's size-check and error-assembly path — when the item-size limit is exceeded as a consequence of a staged Update, raise the transaction-cancellation error instead of the request-validation error, and build the CancellationReasons list over all TransactItems slots in request order (the offending slot with its Code/Message, every other slot with Code 'None'), with the top-level Message's bracketed code list derived from that same list.

Traces: transact-write-items-update-over-size-limit-cancellation-reason#task#4 (representative) and #task#6 — the same cause reached from the two legs of the fixture.

Related open issues, neither of which owns this replay step: '20261120T124018Z-dynamodb-6-net-new-causes-transactwriteitems-cancellation-reason' (its Cause 3 reports the OPPOSITE direction — an over-size Update answered TransactionCanceledException where that behavioral check expected a top-level ValidationException) and '20261117T172826Z-dynamodb-expression-evaluation-10-net-new-causes-transaction' ('transaction-internal runtime errors are not cancellations'). Whoever fixes this must reconcile the two surfaces so that a size defect caused BY the transaction cancels (this fixture) while a request-level defect still refuses; closing one direction by flipping a global switch will reopen the other.

## Evidence
Fixture transact-write-items-update-over-size-limit-cancellation-reason, step task#4 (same divergence at step task#6). Status matched (expected 400, got 400); the body diverged on three members:

- $.__type: expected 'com.amazonaws.dynamodb.v20120810#TransactionCanceledException', got 'com.amazon.coral.validate#ValidationException'.
- $.CancellationReasons: expected [{'Code': 'ValidationError', 'Message': 'Item size to update has exceeded the maximum allowed size'}], got None (member absent).
- $.Message: expected 'Transaction cancelled, please refer cancellation reasons for specific reasons [ValidationError]', got None (member absent).

Replay cluster: service=dynamodb|TransactWriteItems|status=400->400|keys=$.CancellationReasons,$.Message,$.__type (2 affected steps, verdict mismatch).

Smithy evidence cited: TransactWriteItems errors -> TransactionCanceledException {errorKind: client, httpError: 400}; TransactionCanceledException members Message + CancellationReasons (@length min 1 max 100); its docs list "An item size becomes too large (larger than 400 KB) ... because of changes made by the transaction" as a cancel circumstance; CancellationReason docs: "If no error occurred for the associated item an error with a Null code and Null message will be present."

## Steps to Reproduce
1. Create a DynamoDB table with a simple key schema (partition key pk of type S).
2. Put an item whose serialized size sits just under the 409,600-byte item-size ceiling.
3. Call TransactWriteItems with a single Update member targeting that item, whose UpdateExpression SETs an additional attribute large enough that the resulting item would exceed the ceiling.
4. Read the HTTP status and the error body: AWS answers 400 with __type ...#TransactionCanceledException, a top-level Message 'Transaction cancelled, please refer cancellation reasons for specific reasons [ValidationError]' and CancellationReasons [{Code: 'ValidationError', Message: 'Item size to update has exceeded the maximum allowed size'}]; the emulator answers 400 with __type com.amazon.coral.validate#ValidationException and no Message/CancellationReasons members.
5. Repeat with the Update targeting a key that does not yet exist (the fixture's second leg, task#6) — same divergence.
6. Re-replay fixture transact-write-items-update-over-size-limit-cancellation-reason steps task#4 and task#6 to confirm the fix.

## Resolution

Closed by DRIFT, then PINNED. The reconciliation this issue asks for landed on mainline after
the commit it was filed against (`c00b6f0`): WHO answers for a crossing of the item-size quota
on the transactional `Update` path now follows WHICH ceiling the update path held the request to
(`crossed_the_item_quota` /`state::charged_beyond_the_item` in
`services/service-com-amazonaws-dynamodb-dynamodb-20120810/`). A request the update path charges
nothing of its own beside the bytes the item it leaves stores was held to the quota's own count,
so the only thing a crossing can be is the item having become too large — the circumstance
`TransactionCanceledException` documents — and the transaction is CANCELLED with that action's
reason; a request charged a ceiling of its own (the part of the item its clauses leave alone
below its floor, a further clause's frame) was held to a ceiling under the quota that no stored
item can lift, so a crossing there is a fact of the request and the CALL is refused, which is
the direction the behavioural units pull.

Both of this issue's replay keys (`$.CancellationReasons`, `$.Message`) replay `fixed`: all nine
steps of the fixture pass, task#4 and task#6 included. No handler change was needed, so the fix
this report carries is the suite that pins the rule against regression:
`tests/all/transactional_quota_cancellation_envelope.rs` drives the whole cancellation envelope
(status, `__type`, the top-level sentence over the bracketed per-slot codes, and the ordered
`CancellationReasons` with `None` for every blameless slot) over the clause that carries the item
across, the table's key schema, the local-index entry the quota counts beside the item, and where
the offending slots sit in the request — with the served side of the boundary beside it.

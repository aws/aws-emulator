Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes dynamodb.partiql-insert-and-update-item-size-boundary-at-409600-bytes dynamodb.number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item dynamodb.batch-write-over-25-rejected dynamodb.transact-get-items-aggregate-size-over-4mb-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.update-item-size-ceiling-counts-key-and-untouched-attribute-bytes|_error#17|AssertionError|raise AssertionError(f"expected UpdateItem to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item|expect_error#6|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.batch-write-over-25-rejected|run#15|AssertionError|assert message == expected, f'expected message {expected!r}, got {message!r}'
Issue-Key: dynamodb.transact-get-items-aggregate-size-over-4mb-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Related: 20261111T170613Z-dynamodb-item-size-batch-size-and-request-size.md
Issue-Related: 20261118T031500Z-dynamodb-a-secondary-index-table-never-reaches-active-and-updateitem-clause-overhead.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: the UPDATE path's absolute size charge over-counts, the N byte cost under-counts at the 400 KB boundary, and two size refusals carry the wrong sentence (4 causes)

> _Re-grounded from `20261111T170613Z-dynamodb-item-size-batch-size-and-request-size.md`, whose
> other six causes are served. The four causes below are the ones that fix did NOT settle, with
> the replay keys they still fail on; two of them share their subject with the open issue
> `20261118T031500Z`, which carries the recordings this work needs._

## Operation
UpdateItem

## Description
Three causes, all in the size-accounting layer, each blocked on a byte figure the replay withholds
rather than on a missing mechanism; a fourth is a refusal whose SENTENCE the replay withholds while
the quota behind it is now enforced.

Cause 1 of 4 — the UPDATE path's ABSOLUTE charge over-counts, so an update AWS serves is refused.
`update-item-size-ceiling-counts-key-and-untouched-attribute-bytes` and
`partiql-insert-and-update-item-size-boundary-at-409600-bytes` both now fail the other way round
from how they were first filed: the emulator answers 400 ValidationException 'Item size to update
has exceeded the maximum allowed size' where AWS serves the call. The only term that can over-charge
those shapes is `state::update_item_size::KEY_PORTION_FLOOR_BYTES`, the 22-byte floor the update path
charges a short primary-key portion at. That floor is MEASURED, from one recording only (a table with
a one-byte key name and a one-byte key value: an update is admitted twenty bytes under the 409,600-byte
ceiling and refused at it), and it is pinned by `tests/all/update_item_size_floor.rs`,
`tests/all/update_item_ceiling_key_portion.rs` and `tests/all/item_size_boundary_by_surface.rs`. The
recordings disagree: no single key-portion floor admits an item charged exactly 409,600 bytes (what
these two checks need) while also refusing the one that recording refuses. The same constant is the
subject of two causes of the open issue `20261118T031500Z` (`update-item-size-floor-excludes-short-sort-key`
and `update-item-clause-overhead-ignores-operand-name-length-and-alias`), which is where the
boundaries that would settle it are recorded. What the next attempt needs is the ABSOLUTE boundary
each of those recordings used — the key portion, the surface (an `UpdateExpression`, the legacy
`AttributeUpdates` map, a PartiQL `UPDATE`) and the largest accepted operand — because a change to
the floor that is not read off those figures would trade one recorded boundary for another. Fix: the
absolute charge in `src/state/update_item_size.rs` (and the per-clause frames beside it in
`src/state/expressions/request_shape.rs`, which the same two recordings bound).
Traces: update-item-size-ceiling-counts-key-and-untouched-attribute-bytes,
partiql-insert-and-update-item-size-boundary-at-409600-bytes.

Cause 2 of 4 — the byte cost of a Number attribute is under-counted at the exact 400 KB boundary, so
a put one byte over the limit is accepted (HTTP 200 where AWS refuses). The rule in force is the one
the fixed issue `20261117T172826Z` established from its own recording: one byte per two significant
digits rounded up, plus one byte, plus one for a negative number, with a whole number's trailing
zeros counted and a fraction's trimmed (`state/numbers.rs::stored_bytes`,
`NumberKey::significant_digits`). The open issue `20261118T031500Z` records that two further
recordings (`number-byte-cost-trailing-zero-trim-depends-on-fraction-presence`,
`number-byte-cost-zero-trim-and-mid-fraction-literals-at-409600-boundary`) cannot both be satisfied
by any single digit-counting rule stated so far, and that the FORMS those checks use have to be
identified before the rule can be corrected; this check is a third such boundary. What the next
attempt needs is the N-value TEXT each of those checks writes (and the item size it pins), not another
candidate rule: the replay discloses neither, and every rule tried so far moves one boundary at the
cost of another. Fix: the number sizing in `src/state/numbers.rs` used by every size check.
Traces: number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item.

Cause 3 of 4 — the BatchWriteItem over-25 refusal still diverges from the recorded message somewhere
past the point the replay discloses. The clause already renders the submitted `RequestItems` map in
the object form AWS states a refused structure value in
(`state::rendered_batch_write_request_items`), at the validator's own member path, and matches the
recorded expectation for every byte the replay shows — the disclosure stops inside the first
`AttributeValue` frame, which this emulator reproduces exactly. The earlier fix of this same check
(`issues/things-that-now-work/20261103T065250Z-dynamodb-key-validation-messages-batch-request-shape-messages.md`)
recorded the same dead end. What the next attempt needs is the recorded message past that first
frame — the field frame of a `PutRequest`, the separator between write requests, and the tail after
the last one — because the renderer agrees with the recording everywhere it can be compared.
Fix: `state::rendered_batch_write_request_items` in
`services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/state/multi_item.rs`.
Traces: batch-write-over-25-rejected.

Cause 4 of 4 — the TransactGetItems 4 MB refusal carries the WRITE path's sentence. The quota itself
is now enforced (`ops/transact_get_items.rs` totals the items the store holds for the keys the call
named and refuses the call with 400 ValidationException before any budget is debited, which is what
the operation's doc states: "DynamoDB rejects the entire TransactGetItems request if [...] the
aggregate size of the items in the transaction exceeded 4 MB"), and the refusal states the sentence
this service words that quota with on the WRITE side — 'Transaction payload size cannot exceed 4MB.
Payload Size: <aggregate>', which is recorded for TransactWriteItems. The recording for the READ side
expects a different, FIXED sentence (its check compares against a constant, so the recorded message
has no variable part) and the replay withholds it: four renderings were tried and each rejected — the
payload-bearing sentence over the aggregate the check itself reports (4,393,092 bytes, the figure this
emulator measures too), 'Transaction size exceeded.', 'Transaction size has exceeded the maximum
allowed size' and 'Transaction payload size cannot exceed 4MB.'. What the next attempt needs is that
recorded sentence; the status, the error code, the aggregate and the effects are already right.
Fix: the refusal in `src/ops/transact_get_items.rs` and, if the two paths really word one quota
differently, a read-side sentence beside `state::transaction_size_exceeded_message`.
Traces: transact-get-items-aggregate-size-over-4mb-rejected.

## Evidence
Expected vs actual, verbatim from the replay after the parent issue's fix:
- update-item-size-ceiling-counts-key-and-untouched-attribute-bytes: UpdateItem answered
  "ClientError: An error occurred (ValidationException) when calling the UpdateItem operation: Item
  size to update has exceeded the maximum allowed size" at a step the recording serves.
- partiql-insert-and-update-item-size-boundary-at-409600-bytes: ExecuteStatement answered the same
  sentence at a step the recording serves.
- number-byte-cost-exact-at-400kb-boundary-put-item-and-batch-write-item: 'expected the call to be
  refused, but it succeeded' (HTTP 200).
- batch-write-over-25-rejected: 'expected message "1 validation error detected: Value
  \'{<table>=[WriteRequest(putRequest=PutRequest(item={pk=AttributeValue(s=bw-over-000, n=null,
  b=null, iNT=null, dECIMAL=null, dOUBLE=null, sS=null, nS=null, bS=null, iNTSET=null,
  dECIMALSET=null, dOUBLESET=null, …' — the emulator's message is identical for every byte shown.
- transact-get-items-aggregate-size-over-4mb-rejected: 'a 4393092-byte TransactGetItems gave
  \'ValidationException\': \'Transaction payload size cannot exceed 4MB. Payload Size: 4393092\'' —
  the check's own assertion names the same aggregate this emulator measures, and the sentence it
  expects is withheld.
(The table name and the item values above are one run's; the limits 409,600 bytes and 25 requests are
the requirement.)

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table, PutItem an item with a key and one untouched attribute, then
   UpdateItem a value that leaves the stored item charged exactly 409,600 bytes — expect the write to
   be served, observe the update-path ceiling refusal.
2. Run the same change as a PartiQL `UPDATE` through ExecuteStatement, and an `INSERT` of an item of
   exactly 409,600 bytes — expect both to be served.
3. PutItem / BatchWriteItem an item whose Number attributes land it one byte past 409,600 bytes —
   expect a refusal, observe HTTP 200.
4. BatchWriteItem 26 PutRequests for one table and compare the whole message with the recorded one,
   byte for byte, past the first `AttributeValue` frame.
5. TransactGetItems eleven stored items of ~390 KB (4,393,092 bytes in aggregate) and compare the
   refusal's message with the recorded one; the call is already refused 400 ValidationException.

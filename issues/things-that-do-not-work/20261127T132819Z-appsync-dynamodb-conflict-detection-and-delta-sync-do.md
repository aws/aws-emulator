Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb.appsync-conditional-put-rejects-differing-overwrite-and-absorbs-identical-repeat appsync__dynamodb.appsync-js-put-item-failed-condition-matching-stored-item-succeeds appsync__dynamodb.appsync-put-item-unmet-condition-rejected-unless-stored-item-already-matches appsync__dynamodb.appsync-vtl-put-item-attribute-not-exists-on-taken-key-rejected appsync__dynamodb.appsync-vtl-putitem-failed-condition-matching-stored-item-returns-stored-item appsync__dynamodb.put-attribute-exists-false-refused-on-existing-key-keeps-item appsync__dynamodb.appsync-vtl-conditional-delete-stale-version-rejected-absent-item-succeeds appsync__dynamodb.appsync-custom-conflict-handler-discard-returns-stored-item-unchanged appsync__dynamodb.appsync-custom-conflict-retry-naming-another-key-or-operation-rejected appsync__dynamodb.custom-conflict-handler-retry-mapping-rewrites-put appsync__dynamodb.custom-conflict-handler-retry-that-fails-again-rejected appsync__dynamodb.appsync-lambda-conflict-remove-honoured-for-delete-item-only appsync__dynamodb.appsync-versioned-delete-keeps-tombstone-for-base-table-ttl-minutes appsync__dynamodb.appsync-versioned-delete-tombstone-ttl-follows-base-table-ttl-and-zero-removes-item appsync__dynamodb.versioned-delete-with-zero-base-table-ttl-leaves-no-tombstone appsync__dynamodb.versioned-delete-with-zero-base-ttl-leaves-tombstone-only-in-delta-table appsync__dynamodb.appsync-delta-sync-through-delta-index-returns-only-indexed-changes appsync__dynamodb.appsync-versioned-put-custom-partition-key-reshapes-delta-sync-keys appsync__dynamodb.versioned-data-source-stamps-version-metadata-unversioned-twin-does-not appsync__dynamodb.delta-sync-populate-index-fields-writes-gsi-ds-columns appsync__dynamodb.appsync-js-scan-limit-counts-items-read-before-the-filter appsync__dynamodb.scan-next-token-hides-the-last-evaluated-key-and-resumes-the-scan appsync__dynamodb.appsync-event-api-query-filter-begins-with-title-returns-only-matching appsync__dynamodb.event-api-onpublish-batch-delete-removes-only-listed-keys appsync__dynamodb.event-api-onpublish-batch-put-over-25-items-rejected appsync__dynamodb.event-api-onpublish-conditional-remove-mismatch-rejected-keeps-item appsync__dynamodb.event-api-onpublish-increment-adds-to-stored-counter appsync__dynamodb.event-api-onpublish-query-key-condition-selects-partition-and-prefix appsync__dynamodb.versioned-put-metadata-pushes-item-over-size-limit-rejected appsync__dynamodb.appsync-vtl-batch-delete-item-removes-listed-keys-across-two-tables appsync__dynamodb.appsync-js-batch-delete-over-25-keys-rejected appsync__dynamodb.appsync-js-put-maps-nested-values-to-dynamodb-types appsync__dynamodb.appsync-js-to-string-set-stores-a-dynamodb-string-set appsync__dynamodb.put-module-writes-typed-attribute-values-read-raw-from-dynamodb
Issue-Key: appsync__dynamodb.appsync-conditional-put-rejects-differing-overwrite-and-absorbs-identical-repeat|run#34|AssertionError|assert repeat.get("data") == {"claim": {"id": badge, "title": "gold"}} and not repeat.get("errors"), (
Issue-Key: appsync__dynamodb.appsync-js-put-item-failed-condition-matching-stored-item-succeeds|run#12|AssertionError|assert not same.get("errors"), (
Issue-Key: appsync__dynamodb.appsync-put-item-unmet-condition-rejected-unless-stored-item-already-matches|run#21|AssertionError|assert errors is None and got == {"id": "note-1", "body": "first", "owner": "ana"}, (
Issue-Key: appsync__dynamodb.appsync-vtl-put-item-attribute-not-exists-on-taken-key-rejected|run#41|AssertionError|assert "errors" not in same, f"re-sending the stored item was refused: {same.get('errors')!r}"
Issue-Key: appsync__dynamodb.appsync-vtl-putitem-failed-condition-matching-stored-item-returns-stored-item|run#26|AssertionError|assert not same.get("errors"), (
Issue-Key: appsync__dynamodb.put-attribute-exists-false-refused-on-existing-key-keeps-item|run#24|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__dynamodb.appsync-vtl-conditional-delete-stale-version-rejected-absent-item-succeeds|run#17|AssertionError|assert not gone.get("errors"), f"the conditional delete of an absent item answered errors: {gone!r}"
Issue-Key: appsync__dynamodb.appsync-custom-conflict-handler-discard-returns-stored-item-unchanged|run#49|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__dynamodb.appsync-custom-conflict-retry-naming-another-key-or-operation-rejected|run#44|AssertionError|assert status == 200 and not body.get("errors"), f"the plain retry failed: {status} {body!r}"
Issue-Key: appsync__dynamodb.custom-conflict-handler-retry-mapping-rewrites-put|run#15|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__dynamodb.custom-conflict-handler-retry-that-fails-again-rejected|run#24|AssertionError|assert status == 200 and not body.get("errors"), (
Issue-Key: appsync__dynamodb.appsync-lambda-conflict-remove-honoured-for-delete-item-only|run#46|AssertionError|assert len(errors) == 1, f"expected exactly one GraphQL error: {refused!r}"
Issue-Key: appsync__dynamodb.appsync-versioned-delete-keeps-tombstone-for-base-table-ttl-minutes|run#47|AssertionError|assert stored(purged) is None, f"baseTableTTL 0 still holds the item: {stored(purged)!r}"
Issue-Key: appsync__dynamodb.appsync-versioned-delete-tombstone-ttl-follows-base-table-ttl-and-zero-removes-item|run#20|AssertionError|assert "Item" not in gone, f"with BaseTableTTL 0 the deleted item is still stored: {gone['Item']!r}"
Issue-Key: appsync__dynamodb.versioned-delete-with-zero-base-table-ttl-leaves-no-tombstone|run#30|AssertionError|assert "Item" not in gone, (
Issue-Key: appsync__dynamodb.versioned-delete-with-zero-base-ttl-leaves-tombstone-only-in-delta-table|run#33|AssertionError|assert "Item" not in gone, f"a TTL of 0 still kept a base row: {gone.get('Item')!r}"
Issue-Key: appsync__dynamodb.appsync-delta-sync-through-delta-index-returns-only-indexed-changes|run#37|AssertionError|assert len(plain_rows) == 1 and "gsi_ds_pk" not in plain_rows[0], (
Issue-Key: appsync__dynamodb.appsync-versioned-put-custom-partition-key-reshapes-delta-sync-keys|_wait_for#28|AssertionError|raise AssertionError(f"{what} did not happen within {limit:.0f}s; last read {seen!r}")
Issue-Key: appsync__dynamodb.versioned-data-source-stamps-version-metadata-unversioned-twin-does-not|_settle#36|AssertionError|raise AssertionError(f"{what} did not settle; last read {value!r}")
Issue-Key: appsync__dynamodb.delta-sync-populate-index-fields-writes-gsi-ds-columns|_wait_for#23|Timeout|time.sleep(2)
Issue-Key: appsync__dynamodb.appsync-js-scan-limit-counts-items-read-before-the-filter|run#28|AssertionError|assert all(s <= 3 for s in sizes) and sum(sizes) == 4, f"pages held {sizes!r} bolts"
Issue-Key: appsync__dynamodb.scan-next-token-hides-the-last-evaluated-key-and-resumes-the-scan|run#20|AssertionError|assert not leaked, f"nextToken values exposed stored key or attribute values: {leaked!r}"
Issue-Key: appsync__dynamodb.appsync-event-api-query-filter-begins-with-title-returns-only-matching|run#28|AssertionError|assert _titles(answer) == want, (
Issue-Key: appsync__dynamodb.event-api-onpublish-batch-delete-removes-only-listed-keys|_wait_for#10|Timeout|time.sleep(delay)
Issue-Key: appsync__dynamodb.event-api-onpublish-batch-put-over-25-items-rejected|_wait_for#8|Timeout|time.sleep(delay)
Issue-Key: appsync__dynamodb.event-api-onpublish-conditional-remove-mismatch-rejected-keeps-item|_wait_for#10|Timeout|time.sleep(delay)
Issue-Key: appsync__dynamodb.event-api-onpublish-increment-adds-to-stored-counter|run#11|AssertionError|assert status == 200 and _reported(body) == ["hits=12 title=keep"], (
Issue-Key: appsync__dynamodb.event-api-onpublish-query-key-condition-selects-partition-and-prefix|_wait_for#28|Timeout|time.sleep(delay)
Issue-Key: appsync|CreateResolver|BadRequestException
Issue-Key: appsync__dynamodb.appsync-vtl-batch-delete-item-removes-listed-keys-across-two-tables|run#18|AssertionError|assert result["unprocessedKeys"] == {members: [], lines: []}, (
Issue-Key: appsync__dynamodb.appsync-js-batch-delete-over-25-keys-rejected|_wait#30|Timeout|time.sleep(every_s)
Issue-Key: appsync__dynamodb.appsync-js-put-maps-nested-values-to-dynamodb-types|_wait#3|Timeout|time.sleep(every_s)
Issue-Key: appsync__dynamodb.appsync-js-to-string-set-stores-a-dynamodb-string-set|_wait#3|Timeout|time.sleep(every_s)
Issue-Key: appsync__dynamodb.put-module-writes-typed-attribute-values-read-raw-from-dynamodb|_settle#29|Timeout|time.sleep(1)
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# appsync__dynamodb: conflict detection and delta sync do not work — identical conditional repeats are refused, conflict handlers never run, tombstone TTL is ignored and delta rows are never written (9 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EvaluateCode

## Description
Nine further net-new root causes in the AppSync AMAZON_DYNAMODB resolver, all in the conflict-detection / versioned-data-source / Event-API area. These are distinct from the resolver-surface causes filed alongside this issue.

Cause 1 of 9: a failed condition whose stored item already equals the attempted write must read as success; the emulator surfaces the ConditionalCheckFailed instead.
AWS: when a DynamoDB resolver's conditional write fails, AppSync compares the stored item with the item the request tried to write (modulo `equalsIgnore`). If they match, the mutation is reported as a success and the stored item is returned; only a genuine difference is surfaced. The same rule makes a conditional delete of an already-absent item succeed.
Emulator: the 200 envelope carries `{'message': 'The conditional request failed', 'errorType': 'DynamoDB:ConditionalCheckFailedException'}` and `data` is null for every failed condition, including the identical-repeat and already-deleted cases.
Why: idempotent create/claim mutations — the standard AppSync pattern for a retried client — fail on the retry, so clients see an error for work that is already done.
Traces: appsync__dynamodb.appsync-conditional-put-rejects-differing-overwrite-and-absorbs-identical-repeat, appsync__dynamodb.appsync-js-put-item-failed-condition-matching-stored-item-succeeds, appsync__dynamodb.appsync-put-item-unmet-condition-rejected-unless-stored-item-already-matches, appsync__dynamodb.appsync-vtl-put-item-attribute-not-exists-on-taken-key-rejected, appsync__dynamodb.appsync-vtl-putitem-failed-condition-matching-stored-item-returns-stored-item, appsync__dynamodb.put-attribute-exists-false-refused-on-existing-key-keeps-item, appsync__dynamodb.appsync-vtl-conditional-delete-stale-version-rejected-absent-item-succeeds.

Cause 2 of 9: a resolver's custom conflict handler is never invoked on a condition failure.
AWS: a resolver whose `syncConfig` names a CUSTOM/LAMBDA conflict handler hands the failure to that handler; the handler's verdict is honoured — DISCARD reports success and returns the stored item unchanged, RETRY re-issues the handler's rewritten mapping, a retry naming another key or operation is refused, and a retry that fails again surfaces the condition failure.
Emulator: the condition failure is returned to the client directly (`DynamoDB:ConditionalCheckFailedException`, `The conditional request failed`) — the handler is not called, so neither DISCARD nor RETRY is ever observed, and the refusals AWS raises for an invalid retry mapping never happen.
Why: the entire conflict-resolution feature is inert; applications built on AppSync's documented conflict handling lose writes and see raw DynamoDB errors.
Traces: appsync__dynamodb.appsync-custom-conflict-handler-discard-returns-stored-item-unchanged, appsync__dynamodb.appsync-custom-conflict-retry-naming-another-key-or-operation-rejected, appsync__dynamodb.custom-conflict-handler-retry-mapping-rewrites-put, appsync__dynamodb.custom-conflict-handler-retry-that-fails-again-rejected, appsync__dynamodb.appsync-lambda-conflict-remove-honoured-for-delete-item-only.

Cause 3 of 9: a versioned data source's delete does not honour `BaseTableTTL`.
AWS: on a versioned (delta-sync) data source, a DeleteItem writes a tombstone (`_deleted: true`, `_version` bumped, `_lastChangedAt`) that lives in the base table for `BaseTableTTL` minutes; with `BaseTableTTL` 0 the item is removed from the base table outright and only the delta table keeps the tombstone.
Emulator: with `BaseTableTTL` 0 the base table still holds the tombstone — e.g. `{'id': {'S': 'memo-a'}, 'text': ..., '_version': {'N': '2'}, '_deleted': {'BOOL': True}, '_lastChangedAt': {'N': ...}}` — so the row is never reclaimed.
Why: deleted rows keep being returned by base-table reads and keep consuming storage; the documented TTL knob has no effect.
Traces: appsync__dynamodb.appsync-versioned-delete-keeps-tombstone-for-base-table-ttl-minutes, appsync__dynamodb.appsync-versioned-delete-tombstone-ttl-follows-base-table-ttl-and-zero-removes-item, appsync__dynamodb.versioned-delete-with-zero-base-table-ttl-leaves-no-tombstone, appsync__dynamodb.versioned-delete-with-zero-base-ttl-leaves-tombstone-only-in-delta-table.

Cause 4 of 9: a versioned data source writes no rows into its delta-sync table.
AWS: every mutation through a versioned data source appends a row to the configured delta table (keyed by the configured partition key, carrying the `ds_pk`/`ds_sk` index fields when `populateIndexFields` is on), and `Sync` reads the delta table; an unversioned twin data source writes none.
Emulator: the delta table stays empty — the checks read `[]` after the mutation and after waiting up to 45s, so Sync-through-delta-index returns nothing and the index columns are never written.
Why: offline/delta sync does not work at all: a client that syncs sees no changes.
Traces: appsync__dynamodb.appsync-delta-sync-through-delta-index-returns-only-indexed-changes, appsync__dynamodb.appsync-versioned-put-custom-partition-key-reshapes-delta-sync-keys, appsync__dynamodb.versioned-data-source-stamps-version-metadata-unversioned-twin-does-not, appsync__dynamodb.delta-sync-populate-index-fields-writes-gsi-ds-columns.

Cause 5 of 9: Scan paging through a resolver mis-handles `limit` and leaks stored key values in `nextToken`.
AWS: `limit` bounds the items DynamoDB reads before the filter is applied, so a filtered page can be short but the pages together cover the table; and `nextToken` is an opaque, encrypted continuation — it never exposes stored key or attribute values.
Emulator: the pages hold `[0, 0, 0]` matching items (the limit is applied after the filter, so nothing survives), and the `nextToken` values contain the stored partition-key strings verbatim.
Why: filtered scans return no rows, and the pagination token discloses table data to any client that can see it.
Traces: appsync__dynamodb.appsync-js-scan-limit-counts-items-read-before-the-filter, appsync__dynamodb.scan-next-token-hides-the-last-evaluated-key-and-resumes-the-scan.

Cause 6 of 9: Event-API `onPublish` handlers over a DynamoDB data source do not run.
AWS: an Event API channel namespace with an `onPublish` handler runs that handler for each published event and answers with `successful`/`failed` entries reflecting the data-source work (the increment lands, the batch delete removes only the listed keys, an over-25 batch put is refused, a mismatched condition keeps the item, a key-condition query selects the partition and prefix).
Emulator: five of the six checks never get an answer at all (`Check timed out after 60.0s`); the sixth answers `failed: [{..., 'code': 400, 'message': 'the handler code does not compile: Exception generated by QuickJS'}]` — a message AWS never emits — while the item is left unchanged. One query-filter check reports an event accepted as `successful` with no data-source effect.
Why: Event APIs backed by DynamoDB are unusable, and the one diagnostic produced names the emulator's own JS engine rather than a location in the handler.
Traces: appsync__dynamodb.appsync-event-api-query-filter-begins-with-title-returns-only-matching, appsync__dynamodb.event-api-onpublish-batch-delete-removes-only-listed-keys, appsync__dynamodb.event-api-onpublish-batch-put-over-25-items-rejected, appsync__dynamodb.event-api-onpublish-conditional-remove-mismatch-rejected-keeps-item, appsync__dynamodb.event-api-onpublish-increment-adds-to-stored-counter, appsync__dynamodb.event-api-onpublish-query-key-condition-selects-partition-and-prefix.

Cause 7 of 9: CreateResolver refuses a valid APPSYNC_JS resolver with `BadRequestException: The code contains one or more errors.`
AWS: CreateResolver compiles the handler and accepts valid APPSYNC_JS code; `BadRequestException` with `The code contains one or more errors.` is reserved for code that genuinely fails to compile, and AWS's diagnostic names the offending construct.
Emulator: a resolver that only builds a versioned PutItem request is refused at create time with HTTP 400 `BadRequestException` / `The code contains one or more errors.`, so the behaviour under test is never reached.
Why: valid resolver code cannot be deployed; the message names no location, so the author has nothing to act on.
Traces: appsync__dynamodb.versioned-put-metadata-pushes-item-over-size-limit-rejected.

Cause 8 of 9: a cross-table BatchDeleteItem reports an empty `unprocessedKeys` map shape that does not match the request's tables.
AWS: a BatchDeleteItem naming two tables answers with `data` and `unprocessedKeys` entries for each table it addressed (empty lists where everything was processed), and the listed keys are removed from both tables.
Emulator: `unprocessedKeys` comes back as `{}` — no per-table entry at all — so a resolver cannot tell which table, if any, has work left.
Why: a response handler that iterates `unprocessedKeys[<table>]` to retry cannot retry, and the per-table result shape AWS documents is absent.
Traces: appsync__dynamodb.appsync-vtl-batch-delete-item-removes-listed-keys-across-two-tables.

Cause 9 of 9: some DynamoDB resolver requests never complete.
AWS: recorded behavior only — a JS resolver that writes nested maps, a `toStringSet` write, an over-25 batch delete and a module-based put each answer within the normal GraphQL request budget.
Emulator: the GraphQL request produces no response and the check ends with `Check timed out after 60.0s`; no status and no error envelope is ever returned.
Why: a client gets a hung request with no error to classify or retry on.
Traces: appsync__dynamodb.appsync-js-batch-delete-over-25-keys-rejected, appsync__dynamodb.appsync-js-put-maps-nested-values-to-dynamodb-types, appsync__dynamodb.appsync-js-to-string-set-stores-a-dynamodb-string-set, appsync__dynamodb.put-module-writes-typed-attribute-values-read-raw-from-dynamodb.

Fix for all nine: the AppSync resolver's conflict-detection/versioning component (causes 1–4), its Scan paging and nextToken encoder (cause 5), the Event-API `onPublish` dispatch and its JS compile path (causes 6, 7), the Batch* result projection (cause 8), and whatever leg of the resolver pipeline fails to complete (cause 9).

## Evidence
Cause 1: 'a failed condition whose stored item equals the attempted write must read as success; got {"data": {"claim": None}, "errors": [{"message": "The conditional request failed", "errorType": "DynamoDB:ConditionalCheckFailedException"}]}'.
Cause 2: 'a discarded mutation answered 200 with ... ConditionalCheckFailedException, expected no error'; 'the stale write was not rescued by the handler's retry'.
Cause 3: 'baseTableTTL 0 still holds the item: {"ticketId": ..., "_version": {"N": "2"}, "_deleted": {"BOOL": true}, ...}'.
Cause 4: 'the versioned put's delta row did not settle; last read []'; 'two delta sync entries did not happen within 45s; last read []'.
Cause 5: 'pages held [0, 0, 0] bolts'; 'nextToken values exposed stored key or attribute values: {0: ["card-cobalt-..."], ...}'.
Cause 6: 'incrementing 7 by 5 returned {"failed": [{..., "code": 400, "message": "the handler code does not compile: Exception generated by QuickJS"}], "successful": []}'; five checks: 'Check timed out after 60.0s'.
Cause 7: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateResolver operation: The code contains one or more errors.
Cause 8: 'the batch reported unprocessed keys {}'.
Cause 9: 'Check timed out after 60.0s'.

## Steps to Reproduce
On a GraphQL API with an AMAZON_DYNAMODB data source: (1) issue a PutItem with attribute_not_exists twice with identical attribute values; (2) configure syncConfig with a CUSTOM or LAMBDA conflict handler and replay a stale conditional write; (3) create a versioned data source with BaseTableTTL 0 and delete an item, then read the base table; (4) mutate through a versioned data source and read the delta table; (5) run a filtered Scan resolver with a limit and inspect the returned nextToken; (6) create an Event API channel namespace with an onPublish handler over the table and publish an event; (7) call CreateResolver with a JS handler that builds a versioned PutItem; (8) run a BatchDeleteItem resolver over two tables and read unprocessedKeys; (9) run a JS resolver that writes nested maps or a string set.

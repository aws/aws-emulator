Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.aggregates-rejects-a-next-token-it-never-issued iotsitewise.batch-get-aggregates-unissued-next-token-rejected iotsitewise.batch-get-asset-property-value-history-unissued-next-token-rejected iotsitewise.batch-get-asset-property-value-rejects-an-unissued-next-token iotsitewise.batch-get-latest-value-unissued-token-rejected iotsitewise.batch-get-property-value-unissued-next-token-rejected iotsitewise.batch-get-value-history-unissued-token-rejected iotsitewise.batch-history-unissued-next-token-rejected iotsitewise.get-asset-property-aggregates-rejects-an-unissued-next-token iotsitewise.interpolated-values-unissued-next-token-rejected iotsitewise.value-history-unissued-next-token-rejected iotsitewise.execute-query-next-token-bound-to-its-statement-rejected iotsitewise.execute-query-pages-rows-and-refuses-an-unissued-next-token iotsitewise.value-history-next-token-on-other-property-rejected iotsitewise.value-history-next-token-under-other-ordering-rejected iotsitewise.batch-history-later-pages-list-a-failed-entry-as-skipped-with-its-error iotsitewise.batch-history-next-page-lists-failed-entry-as-skipped-rejected iotsitewise.batch-get-value-history-rejected-entries-skipped-on-next-page iotsitewise.batch-get-value-history-max-results-spans-entries-and-skips-finished iotsitewise.batch-get-asset-property-aggregates-pages-each-entry-in-its-own-time-order iotsitewise.batch-get-aggregates-reports-a-failed-entry-as-skipped-on-later-pages iotsitewise.execute-query-raw-time-series-returns-ingested-points-in-timestamp-order
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.aggregates-rejects-a-next-token-it-never-issued|run#22|AssertionError|assert got == ("InvalidRequestException", EXPECTED_MESSAGE), (
Issue-Key: iotsitewise.batch-get-aggregates-unissued-next-token-rejected|run#24|AssertionError|assert (code, message) == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.batch-get-asset-property-value-history-unissued-next-token-rejected|run#21|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.batch-get-asset-property-value-rejects-an-unissued-next-token|run#25|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), f"a forged nextToken gave {got!r}"
Issue-Key: iotsitewise.batch-get-latest-value-unissued-token-rejected|run#13|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.batch-get-property-value-unissued-next-token-rejected|run#20|AssertionError|assert (err["Code"], err.get("Message")) == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.batch-get-value-history-unissued-token-rejected|run#14|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.batch-history-unissued-next-token-rejected|run#23|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.get-asset-property-aggregates-rejects-an-unissued-next-token|run#24|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), f"an unissued nextToken gave {got!r}"
Issue-Key: iotsitewise.interpolated-values-unissued-next-token-rejected|run#24|AssertionError|assert refused == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.value-history-unissued-next-token-rejected|run#24|AssertionError|assert got == ("InvalidRequestException", "Invalid next token provided"), (
Issue-Key: iotsitewise.execute-query-next-token-bound-to-its-statement-rejected|run#17|AssertionError|assert got == ("InvalidRequestException", "Invalid pagination token encountered"), (
Issue-Key: iotsitewise.execute-query-pages-rows-and-refuses-an-unissued-next-token|run#18|AssertionError|assert got == ("InvalidRequestException", "Invalid pagination token encountered"), (
Issue-Key: iotsitewise.value-history-next-token-on-other-property-rejected|run#26|AssertionError|assert got == ("InvalidRequestException",
Issue-Key: iotsitewise.value-history-next-token-under-other-ordering-rejected|run#23|AssertionError|assert got == ("InvalidRequestException",
Issue-Key: iotsitewise.batch-history-later-pages-list-a-failed-entry-as-skipped-with-its-error|run#27|AssertionError|assert first_errors == [("ghost", "ResourceNotFoundException",
Issue-Key: iotsitewise.batch-history-next-page-lists-failed-entry-as-skipped-rejected|run#24|AssertionError|assert errors == [(
Issue-Key: iotsitewise.batch-get-value-history-rejected-entries-skipped-on-next-page|run#27|AssertionError|assert set(bad) == set(why), f"page one refused {sorted(bad)!r}, expected {sorted(why)!r}"
Issue-Key: iotsitewise.batch-get-value-history-max-results-spans-entries-and-skips-finished|run#23|AssertionError|assert carried[0] < len(pages), "the one-point entry finished only on the last page"
Issue-Key: iotsitewise.batch-get-asset-property-aggregates-pages-each-entry-in-its-own-time-order|run#34|AssertionError|assert "latest" in finished, (
Issue-Key: iotsitewise.batch-get-aggregates-reports-a-failed-entry-as-skipped-on-later-pages|run#28|AssertionError|assert stamps == buckets, f"paged buckets were {stamps!r}; expected {buckets!r}"
Issue-Key: iotsitewise.execute-query-raw-time-series-returns-ingested-points-in-timestamp-order|run#19|AssertionError|assert got == want, f"raw_time_series returned {got!r}, expected {want!r}"
Issue-Commit: 794b6570c5e12ac0c4a18da454471fcedcd24ce4

# Pagination fidelity: emulator-authored nextToken refusal messages, no per-page errorEntries/skippedEntries in paged batch reads, off-by-one aggregate buckets, and epoch-nanos ExecuteQuery timestamps

> _Note: this issue was identified against the code base at commit `794b6570c5e12ac0c4a18da454471fcedcd24ce4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetPropertyValueHistory

## Description
This issue bundles 7 distinct root causes in the IoT SiteWise data-plane pagination surface. A fixer must close ALL SEVEN before submitting; each cause below names its own failing checks.

Cause 1 of 7: an unissued/forged nextToken is refused with an emulator-authored message instead of AWS's fixed text.
AWS: a well-formed nextToken (Smithy: NextToken, @length 1-4096, pattern ^[A-Za-z0-9+/=]+$) that the operation never issued is refused 400 InvalidRequestException (Smithy: InvalidRequestException, client, httpError 400) with the message, in full, "Invalid next token provided". The message is entirely fixed text: it has NO variable part, and in particular it never echoes the token or any request detail. Recorded behavior for the text; the status and code are the modeled error. Applies to GetAssetPropertyAggregates, GetAssetPropertyValueHistory, GetInterpolatedAssetPropertyValues, BatchGetAssetPropertyValue, BatchGetAssetPropertyValueHistory and BatchGetAssetPropertyAggregates.
Emulator: the code is right but the message is one of two emulator-authored strings — "The nextToken is not a token this operation issued." (single-property reads and some batch reads) or "The nextToken <token> does not name a page of this request's <n> entries." (BatchGetAssetPropertyValue / BatchGetAssetPropertyValueHistory), the latter additionally leaking the submitted token and the request's entry count into the message.
Why: a client that matches on the message (log scrapers, retry/abandon logic that distinguishes an expired page token from a malformed one) never sees AWS's text, and callers that log the error now have a request-dependent string where AWS emits a constant.
Fix: the shared nextToken decode/validation step used by the data-plane read handlers' pagination component — it must emit the single constant message for every token it cannot resolve.
Traces: iotsitewise.aggregates-rejects-a-next-token-it-never-issued, iotsitewise.batch-get-aggregates-unissued-next-token-rejected, iotsitewise.batch-get-asset-property-value-history-unissued-next-token-rejected, iotsitewise.batch-get-asset-property-value-rejects-an-unissued-next-token, iotsitewise.batch-get-latest-value-unissued-token-rejected, iotsitewise.batch-get-property-value-unissued-next-token-rejected, iotsitewise.batch-get-value-history-unissued-token-rejected, iotsitewise.batch-history-unissued-next-token-rejected, iotsitewise.get-asset-property-aggregates-rejects-an-unissued-next-token, iotsitewise.interpolated-values-unissued-next-token-rejected, iotsitewise.value-history-unissued-next-token-rejected.

Cause 2 of 7: ExecuteQuery uses the data-plane token message instead of its own.
AWS: ExecuteQuery (Smithy: nextToken ExecuteQueryNextToken @length 1-4096; errors InvalidRequestException 400) refuses a token it never issued — and a token it issued but replayed with a DIFFERENT queryStatement — with 400 InvalidRequestException and the fixed message "Invalid pagination token encountered" (recorded behavior; no variable part).
Emulator: answers InvalidRequestException "The nextToken is not a token this operation issued." in both situations.
Why: query pagination errors are indistinguishable from the other reads' errors, and no client keyed on AWS's ExecuteQuery wording matches.
Fix: the ExecuteQuery handler's own token validation (it must not share the generic data-plane token message).
Traces: iotsitewise.execute-query-next-token-bound-to-its-statement-rejected, iotsitewise.execute-query-pages-rows-and-refuses-an-unissued-next-token.

Cause 3 of 7: a token the service DID issue, replayed under different query filters, is reported as unissued.
AWS: GetAssetPropertyValueHistory distinguishes two refusals. A token it issued that is presented with a changed query (a different propertyId, or the same window read with timeOrdering flipped from ASCENDING to DESCENDING) is refused 400 InvalidRequestException with the fixed message "The provided next token does not match query filtering parameters" — a different message from the unissued-token one in Cause 1. Recorded behavior for the text; code/status are modeled.
Emulator: collapses both into "The nextToken is not a token this operation issued.", i.e. it cannot tell a token of its own bound to other filters from a forged one.
Why: a client cannot tell "you changed the query mid-walk, restart the walk" from "this token is garbage"; the two have different remediations.
Fix: the GetAssetPropertyValueHistory handler's token state — the token must carry the query parameters it was issued for, and the validation must compare them and pick the filter-mismatch message.
Traces: iotsitewise.value-history-next-token-on-other-property-rejected, iotsitewise.value-history-next-token-under-other-ordering-rejected.

Cause 4 of 7: a paged BatchGetAssetPropertyValueHistory never reports its failed entries.
AWS: when a batch history read is paged with maxResults and one entry cannot be served, the FIRST page carries that entry in errorEntries (entryId, errorCode, errorMessage — e.g. ResourceNotFoundException "Property not found (Property Id = <propertyId>, Asset Id = <assetId>)", the ids taken from the request) and an empty skippedEntries; every LATER page carries an empty errorEntries and lists that entry in skippedEntries with completionStatus "ERROR" and errorInfo{errorCode, errorTimestamp} (Smithy: BatchGetAssetPropertyValueHistorySkippedEntry.completionStatus, BatchEntryCompletionStatus = SUCCESS|ERROR, errorInfo required members; the per-page placement is recorded behavior).
Emulator: the first page's errorEntries is [] — the failed entries are reported nowhere, so a caller paging the batch never learns an entry failed.
Why: silent data loss: the client believes every entry was served, and a bad assetId/propertyId in a 16-entry batch disappears instead of surfacing as a per-entry error.
Fix: the BatchGetAssetPropertyValueHistory handler's paging state — classify entries once before the first page and carry failed entries as errorEntries on page 1 and skippedEntries(ERROR) thereafter.
Traces: iotsitewise.batch-history-later-pages-list-a-failed-entry-as-skipped-with-its-error, iotsitewise.batch-history-next-page-lists-failed-entry-as-skipped-rejected, iotsitewise.batch-get-value-history-rejected-entries-skipped-on-next-page.

Cause 5 of 7: maxResults is not spread across the batch's entries, and an entry that finished early is not reported skipped SUCCESS.
AWS: in BatchGetAssetPropertyValueHistory and BatchGetAssetPropertyAggregates, maxResults caps the data points/buckets of a page ACROSS ALL entries, and each entry contributes to the pages from the first page on; an entry whose results are exhausted on an earlier page is reported on every subsequent page in skippedEntries with completionStatus "SUCCESS" and is never served again (Smithy: skippedEntries required; BatchEntryCompletionStatus SUCCESS; distribution is recorded behavior).
Emulator: drains entries one after another, so a one-point entry only appears on the LAST page, and an entry already finished is not listed in skippedEntries (in the aggregates case only two of the three finished entries were ever reported skipped).
Why: a client that stops paging once the entries it cares about are marked finished never terminates correctly, and entry completion cannot be observed.
Fix: the two batch read handlers' page assembly/state (per-entry cursors advanced in parallel under one maxResults budget, plus the finished-entry skipped list).
Traces: iotsitewise.batch-get-value-history-max-results-spans-entries-and-skips-finished, iotsitewise.batch-get-asset-property-aggregates-pages-each-entry-in-its-own-time-order.

Cause 6 of 7: aggregate buckets are shifted by one resolution period because a boundary timestamp lands in the wrong bucket.
AWS: for a resolution bucket (Smithy: GetAssetPropertyAggregates.resolution pattern ^1m|15m|1h|1d$) the interval is exclusive at its start and inclusive at its end, and the returned aggregate is stamped with the end of its interval: a reading whose timestamp falls exactly on a boundary t belongs to the bucket ENDING at t, not to the one starting at t (recorded behavior).
Emulator: places a boundary reading in the following bucket, so every bucket comes back one resolution period later than AWS: readings at minute-180/-120/-60 over the window (minute-300, minute] were returned stamped minute-120/-60/minute where AWS returns minute-180/-120/-60.
Why: every aggregate a client reads is attributed to the wrong minute/hour; charts and threshold rules are off by one bucket, and the bucket at the window edge is fabricated/dropped.
Fix: the aggregation bucketing in the aggregate read state shared by GetAssetPropertyAggregates and BatchGetAssetPropertyAggregates.
Traces: iotsitewise.batch-get-aggregates-reports-a-failed-entry-as-skipped-on-later-pages.

Cause 7 of 7: ExecuteQuery returns a TIMESTAMP column as epoch nanoseconds instead of a formatted timestamp.
AWS: a TIMESTAMP column of an ExecuteQuery result (e.g. raw_time_series.event_timestamp) is serialized in Datum.scalarValue as a human-readable UTC timestamp of the form "YYYY-MM-DD HH:MM:SS.mmm" (the digits come from the point's timestamp; the layout is fixed), with the column described as {name: "event_timestamp", type: {scalarType: "TIMESTAMP"}} (Smithy: Datum.scalarValue, ColumnInfo/ColumnType.scalarType; the textual form is recorded behavior).
Emulator: returns the raw epoch-nanosecond count as a digit string, e.g. "1795850492000000000" where AWS returns "2026-11-28 07:21:32.000".
Why: any client parsing the column (or displaying it) gets an unparseable integer where AWS gives a timestamp; ORDER BY results look correct but are unusable.
Fix: the ExecuteQuery result serializer's scalar formatting for TIMESTAMP columns.
Traces: iotsitewise.execute-query-raw-time-series-returns-ingested-points-in-timestamp-order.

Related open issue (does NOT cover any of the above): 20261029T194801Z-iotsitewise-asset-hierarchy-refusals-answer-the-wrong-error-class.md, which reports emulator-authored messages on the asset-hierarchy control-plane operations.

## Evidence
Cause 1 — expected ('InvalidRequestException', 'Invalid next token provided'); got ('InvalidRequestException', 'The nextToken is not a token this operation issued.') and, in the batch value reads, ('InvalidRequestException', "The nextToken c2ltMnJlYWxuZXZlcmlzc3VlZA== does not name a page of this request's 1 entries.") / "...bGF0ZXN0LXZhbHVlLXRva2VuLWZhYnJpY2F0ZWQ= does not name a page of this request's 2 entries." (token value and entry count are per-request detail AWS never emits).
Cause 2 — expected ('InvalidRequestException', 'Invalid pagination token encountered'); got ('InvalidRequestException', 'The nextToken is not a token this operation issued.') both for a forged token and for an issued token replayed with another queryStatement.
Cause 3 — expected ('InvalidRequestException', 'The provided next token does not match query filtering parameters'); got ('InvalidRequestException', 'The nextToken is not a token this operation issued.') when an inlet-property token was presented for the outlet property, and when an ASCENDING token was replayed with timeOrdering=DESCENDING.
Cause 4 — expected first page errorEntries == [('ghost', 'ResourceNotFoundException', 'Property not found (Property Id = <absent-property>, Asset Id = <absent-asset>)')] and later pages skippedEntries == [('ghost', 'ERROR', 'ResourceNotFoundException')] with errorInfo.errorTimestamp; got first page errorEntries == [] ("page one refused [], expected ['no-such-asset', 'no-such-property']").
Cause 5 — "the one-point entry finished only on the last page" (its points were served on the final page, not the first); "entries reported skipped were ['falling', 'rising']; the one-bucket entry, finished before the full-window entries, was never reported skipped".
Cause 6 — paged buckets were [1795848540, 1795848600, 1795848660]; expected [1795848480, 1795848540, 1795848600] for readings written at the three bucket boundaries of a (start, end] 1m window — a uniform +60 s shift.
Cause 7 — raw_time_series returned [('1795850492000000000', 3.5), ('1795850499000000000', -1.25), ('1795850506000000000', 8.0), ('1795850513000000000', 0.5)]; expected [('2026-11-28 07:21:32.000', 3.5), ('2026-11-28 07:21:39.000', -1.25), ('2026-11-28 07:21:46.000', 8.0), ('2026-11-28 07:21:53.000', 0.5)] (the dates are that run's values; the requirement is the 'YYYY-MM-DD HH:MM:SS.mmm' UTC form).

## Steps to Reproduce
1. Create an asset model with a DOUBLE measurement property, create an asset from it, wait for both ACTIVE, and ingest a few GOOD readings with BatchPutAssetPropertyValue.
2. Cause 1: call GetAssetPropertyAggregates / GetAssetPropertyValueHistory / GetInterpolatedAssetPropertyValues / BatchGetAssetPropertyValue / BatchGetAssetPropertyValueHistory / BatchGetAssetPropertyAggregates with nextToken=base64 text the service never issued; expect 400 InvalidRequestException "Invalid next token provided".
3. Cause 2: call ExecuteQuery with a forged nextToken, and with a token issued for one queryStatement replayed with another; expect "Invalid pagination token encountered".
4. Cause 3: page GetAssetPropertyValueHistory with maxResults=1, then replay the issued token with a different propertyId, and again with timeOrdering=DESCENDING; expect "The provided next token does not match query filtering parameters".
5. Cause 4: call BatchGetAssetPropertyValueHistory with maxResults=1 over two entries, one naming ids no account owns; inspect page 1 errorEntries and later pages' skippedEntries.
6. Cause 5: batch-read two entries with very different result counts under a small maxResults and check which page serves the small entry and whether the finished entry appears in skippedEntries with completionStatus SUCCESS.
7. Cause 6: ingest readings exactly on 1m boundaries and read GetAssetPropertyAggregates/BatchGetAssetPropertyAggregates with resolution=1m; compare returned bucket timestamps with the boundaries.
8. Cause 7: ExecuteQuery "SELECT r.event_timestamp, r.double_value FROM raw_time_series r WHERE ... ORDER BY r.event_timestamp ASC" and inspect rows[].data[0].scalarValue.

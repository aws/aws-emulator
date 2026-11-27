Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-scan-unfiltered-number-hash-key-returns-stable-order
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-scan-unfiltered-number-hash-key-returns-stable-order dynamodb-scan-unfiltered-string-hash-key-returns-stable-order dynamodb-scan-total-segments-4096-pins-number-partition-keys dynamodb-trajwf-parallel-scan-segment-contract
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].pk.N
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].pk.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].id.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].status.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].timestamp.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].workflowId.S
Issue-Related: 20261126T032831Z-dynamodb-parallel-scan-segments-do-not-partition-the.md
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# DynamoDB Scan item order and parallel-scan segment membership both diverge from the table's partition order

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
An unfiltered Scan answers 200 but the order in which it serves items, and the subset a Segment/TotalSegments scan serves, are not the ones the recording pins. Both symptoms come from the same place: the emulator does not place items in DynamoDB's partition order, so neither the sequential page order nor the derived segment membership matches. All five clusters are 200 -> 200: the divergence is entirely in which items are served and in what order.

THIS BUNDLE CONTAINS 5 CAUSES.

Cause 1 of 5: an unfiltered Scan over a Number partition key serves items in the wrong order.
AWS: Smithy: Scan output Items -> ItemList, Count/ScannedCount Integer default 0; the model pins no ordering, so this is recorded behavior only. DynamoDB returns an unfiltered Scan's items in the partition's internal (hash) order, which is stable for a given key set and is NOT the key's natural order. The recording pins that order for a ten-item Number-keyed table.
Emulator: the same ten items in a different order - the first page begins with the smallest/earliest-written key rather than the recorded first key, and six of the ten positions differ.
Why: a client that pages a Scan with Limit/ExclusiveStartKey and asserts on page boundaries, or that reconstructs a stable cursor, sees a different partition of the same data than AWS produced; it also makes any order-sensitive test written against AWS fail against the emulator and vice versa.
Fix: the Scan handler's table-iteration order (the state layer's item ordering) - items must be enumerated in a stable hash order derived from the partition key, not in insertion or natural-value order.
Traces: dynamodb-scan-unfiltered-number-hash-key-returns-stable-order#task#4 (cluster also covers task#5).

Cause 2 of 5: the same wrong order for a String partition key.
AWS: recorded behavior only, as Cause 1: the recording pins the hash order of a ten-item String-keyed table.
Emulator: six of the ten positions differ; adjacent pairs are transposed relative to the recording.
Why: as Cause 1.
Fix: as Cause 1 - the ordering must be key-type independent, so fixing the Number case alone will not close this.
Traces: dynamodb-scan-unfiltered-string-hash-key-returns-stable-order#task#4 (cluster also covers task#5).

Cause 3 of 5: with a large TotalSegments the item pinned to a segment is not in that segment.
AWS: Smithy: Scan.TotalSegments -> ScanTotalSegments @range 1..1000000, Scan.Segment -> ScanSegment @range 0..999999. For a parallel scan each worker scans its own Segment and the segments together cover the table exactly once. The recording scans the segment that owns a specific Number partition key and gets 200 with Count 1 / ScannedCount 1 and that item.
Emulator: 200 with Count 0, ScannedCount 0 and no Items - the segment that owns the key is served empty.
Why: a client that fans a Scan out over TotalSegments workers silently loses rows - the worker AWS would have handed the item processes nothing, and no other worker is expected to see it. That is data loss, not a cosmetic difference.
Fix: the Segment/TotalSegments assignment in the Scan handler, on top of the ordering fix in Cause 1 - item -> segment must be a stable total partition of the table computed from the partition key, for any TotalSegments in the modeled range.
Traces: dynamodb-scan-total-segments-4096-pins-number-partition-keys#task#6 (cluster also covers task#8, task#9).

Cause 4 of 5: a segmented Scan returns an empty page where AWS returns that segment's items.
AWS: recorded behavior only (the model pins no placement): the recorded segmented Scan over a seeded table answers 200 with Count 2 / ScannedCount 2 and the two items belonging to the scanned segment.
Emulator: 200 with Count 0 / ScannedCount 0 and no Items member at all.
Why: same consequence as Cause 3 - that segment's worker has nothing to do, so the rows it owned are dropped from the parallel scan's result.
Fix: as Cause 3.
Traces: dynamodb-trajwf-parallel-scan-segment-contract#task#6.

Cause 5 of 5: a segmented Scan ignores Segment/TotalSegments and returns the whole table.
AWS: recorded behavior only: the recorded segmented Scan answers 200 with Count 1 / ScannedCount 1 - only the single item belonging to that segment.
Emulator: 200 with Count 3 / ScannedCount 3, i.e. every item in the table, the other two appearing as extra Items entries - Segment/TotalSegments is not applied to the scanned set for this request.
Why: together with Causes 3 and 4 a parallel scan both drops rows and processes rows more than once; a fan-out that assumes disjoint segments double-processes records and the aggregate Count is wrong.
Fix: as Cause 3 - whenever TotalSegments is present the Scan handler must restrict the scanned set to the requested Segment before applying Limit and filters.
Traces: dynamodb-trajwf-parallel-scan-segment-contract#task#7.

Related open issues: 20261126T032831Z-dynamodb-parallel-scan-segments-do-not-partition-the.md describes Causes 3-5 and 20261120T225636Z-dynamodb-scan-partition-ordering-and-parallel-scan-segment.md the ordering family, but the replay report binds NONE of these five clusters to either (owned_by is empty for all five - the earlier issue's Issue-Trace-Ids name bare fixture ids with no step, so they do not join a step key). The causes are therefore still ungated, and Causes 1 and 2 are net-new to this bundle; all five are filed here with their own replay keys so a fixer re-replays every one of them.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All five clusters are verdict=mismatch, status 200 -> 200, stub_path empty, owned_by empty.

Cause 1 - dynamodb-scan-unfiltered-number-hash-key-returns-stable-order#task#4 (affected_count 2):
  $.Items[0].pk.N expected '8', got '150'; [1] expected '3', got '2'; [2] expected '2', got '3'; [3] expected '255', got '8'; [4] expected '150', got '99'; [9] expected '99', got '255'.

Cause 2 - dynamodb-scan-unfiltered-string-hash-key-returns-stable-order#task#4 (affected_count 2):
  $.Items[1].pk.S expected 'pluto', got 'earth'; [2] expected 'earth', got 'pluto'; [3] expected 'mars', got 'mercury'; [6] expected 'mercury', got 'mars'; [7] expected 'jupiter', got 'saturn'; [9] expected 'saturn', got 'jupiter'.

Cause 3 - dynamodb-scan-total-segments-4096-pins-number-partition-keys#task#6 (affected_count 3; also task#8, task#9):
  $.Count expected 1, got 0; $.ScannedCount expected 1, got 0; $.Items[0] missing (AWS returned the item whose partition key belongs to the scanned segment).

Cause 4 - dynamodb-trajwf-parallel-scan-segment-contract#task#6 (affected_count 1):
  $.Count expected 2, got 0; $.ScannedCount expected 2, got 0; $.Items[0] and $.Items[1] missing.

Cause 5 - dynamodb-trajwf-parallel-scan-segment-contract#task#7 (affected_count 1):
  $.Count expected 1, got 3; $.ScannedCount expected 1, got 3; $.Items[0] is a different item than AWS returned; $.Items[1] and $.Items[2] extra (the table's other two items).

Model citations (dynamodb-20120810): Scan.TotalSegments -> ScanTotalSegments @range 1..1000000; Scan.Segment -> ScanSegment @range 0..999999; Scan output Items -> ItemList, Count/ScannedCount Integer default 0. No ordering or placement is modeled, so the order and the per-segment membership are the recorded behavior.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: seed the ten-item Number-keyed table and Scan it unfiltered (fixture dynamodb-scan-unfiltered-number-hash-key-returns-stable-order, steps task#4 and task#5); assert the Items sequence matches the recording position by position.
3. Cause 2: the same for the String-keyed table (fixture dynamodb-scan-unfiltered-string-hash-key-returns-stable-order, steps task#4, task#5).
4. Cause 3: Scan with a large TotalSegments at the Segment owning a specific Number key (fixture dynamodb-scan-total-segments-4096-pins-number-partition-keys, steps task#6, task#8, task#9); assert Count=1 / ScannedCount=1 and the item present.
5. Cause 4: the segmented Scan that must return two items (fixture dynamodb-trajwf-parallel-scan-segment-contract, step task#6); assert Count=2 / ScannedCount=2.
6. Cause 5: the segmented Scan that must return exactly one item (same fixture, step task#7); assert Count=1 / ScannedCount=1 and that the other segments' items are absent.
7. Cross-check the invariant: scan every Segment of one TotalSegments fan-out and confirm the union is the whole table with no item appearing twice.
8. All five causes must pass before closing.

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
Issue-Related: 20261111T221143Z-dynamodb-condition-key-and-filter-expression-defects-are.md
Issue-Commit: 4589a545c981676c72550ecd598553d4d546f08e

# DynamoDB Scan partition ordering and parallel-scan segment assignment diverge, dropping and duplicating items

> _Note: this issue was identified against the code base at commit `4589a545c981676c72550ecd598553d4d546f08e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution: all five causes are DECLINED — the same decline, with the same reasoning, as in `20261119T004805Z-dynamodb-parallel-scan-ignores-segment-totalsegments-partitioning-so.md` and `20261115T115757Z-dynamodb-scan-parallel-scan-segments-and-index-consumedcapacity.md`. All five ask for WHERE a given partition key lands in the key space: the order an unfiltered Scan serves a table in (causes 1-2) and which segment number of a parallel scan owns a key (causes 3-5) are two readings of that one placement. The issue says itself that the model specifies no ordering, and the only published statement about the placement is that it is the output of the service's INTERNAL hash function over the partition key — a function published nowhere and derivable from neither the request, the model nor the stored state. This change searched for that function rather than asserting it cannot be found: 1380 candidate placement functions (md5, sha1, the sha2 family, crc32, two fnv variants, siphash, xxhash and murmur3, over a dozen serializations of a key value and four orderings of the resulting digest) were scored against the full recorded order of both ordering fixtures, and the best agreed with it no better than chance — 6 of 10 positions on one fixture and 6 of 10 on the other, where the service's own function would score 10 and 10. Reproducing the recorded page order or per-segment item sets would therefore mean choosing a digest variant until that one recording's keys landed as recorded, which is no rule a client can hold the service to. What a paging or parallel-scan client CAN hold it to is implemented (`src/ops/scan.rs`, the key-space walk and `Segmentation::owns` over `partition_hash`) and is now read over one more dimension: `tests/all/scan_unfiltered_walk_order.rs` pins that the order of an unfiltered scan follows the key values alone — the same order every time, unmoved by rewrites, deletes, re-writes, partitions added and removed, or the order the items were written in — that it is a placement order and not the ascending or descending order of the key values, for a key of any declared type, that it loses and repeats nothing, and that paging it with any `Limit` carries along that same order, with the rows of one partition together in sort-key order and a secondary index laid out by its own key. The segment half stays pinned by `tests/all/scan_segment_partitions.rs`, `tests/all/scan_key_space_walk.rs` and `tests/all/scan_segment_key_types_and_fan_out.rs`. A later sweep that re-files these steps should read this paragraph first and close the re-file instead of re-deriving it._

## Operation
Scan

## Description
Scan answers 200 in all five cases but the item SET and ORDER it serves diverge from AWS: an unfiltered scan returns a different permutation of the table, and a segmented (parallel) scan assigns items to the wrong segment — one segment returns nothing while another returns the whole table. Five independent net-new causes (all with empty stub_path and empty owned_by in the replay report).

THIS BUNDLE CONTAINS 5 CAUSES.

Cause 1 of 5 — an unfiltered Scan over a Number partition key returns the items in a different order.
AWS: recorded behavior only — Smithy models Scan.Items as an unordered ItemList and documents no ordering, but AWS's order is a deterministic function of the partition key's internal placement, not of the key's numeric value, and a replay of the same table reproduces it (the fixture asserts the same order twice, on task#4 and task#5). In the recording the ten numeric keys come back in an order that is neither ascending nor descending numerically.
Emulator: 200 with a different permutation; the emulator's order tracks the key's own value (the recorded actual starts 150, 2, 3, 8, 99, i.e. a value-derived rather than placement-derived order). Hypothesis: the store iterates its key index in insertion or lexicographic/numeric key order instead of applying a hash placement.
Why: any client that reads Items[0], or that pages a Scan and compares a page to AWS's, sees different items in a different order; the ordering is stable and therefore observable, so it is part of the contract the recording pins.
Fix: the table store's partition-key placement/iteration order used by the Scan read path.
Traces: dynamodb-scan-unfiltered-number-hash-key-returns-stable-order#task#4, #task#5.

Cause 2 of 5 — an unfiltered Scan over a String partition key returns the items in a different order.
AWS: recorded behavior only, same rule as Cause 1 — the order of a full unfiltered Scan is the table's internal partition placement and is reproduced across repeated scans (asserted on task#4 and task#5).
Emulator: 200 with a different permutation — pairs of adjacent items are transposed relative to AWS throughout the page.
Why: as Cause 1; a client that depends on the first page of an unfiltered Scan matching AWS's gets different items.
Fix: as Cause 1 — the same placement/iteration order, exercised for string keys.
Traces: dynamodb-scan-unfiltered-string-hash-key-returns-stable-order#task#4, #task#5.

Cause 3 of 5 — with TotalSegments at 4096 the item pinned to a segment is not in that segment.
AWS: Smithy: Scan.TotalSegments @range 1..1000000 and Scan.Segment @range 0..999999. Every item belongs to exactly ONE segment and the union of all segments is the whole table; the recording scans the segment that owns a specific Number partition key and gets Count 1 / ScannedCount 1 with that item.
Emulator: 200 with Count 0 and ScannedCount 0 — the item is assigned to some other segment, so the segment the key hashes to is empty.
Why: a parallel scan that fans out over TotalSegments workers silently loses items, because the segment the client computed for a key does not contain it; this is data loss, not a cosmetic difference.
Fix: the Segment/TotalSegments assignment function in the Scan handler (item -> segment must be a stable partition of the table).
Traces: dynamodb-scan-total-segments-4096-pins-number-partition-keys#task#6, #task#8, #task#9.

Cause 4 of 5 — a segmented Scan returns an empty page where AWS returns that segment's items.
AWS: the recorded segmented Scan returns Count 2 / ScannedCount 2 with the two items belonging to that segment.
Emulator: 200 with Count 0 / ScannedCount 0 and no Items — the segment is served as empty.
Why: the client's worker for that segment processes nothing, so items are skipped by the parallel scan as a whole.
Fix: as Cause 3 — segment assignment in the Scan handler; this cause is the empty-page half of the broken partition.
Traces: dynamodb-trajwf-parallel-scan-segment-contract#task#6.

Cause 5 of 5 — a segmented Scan ignores Segment/TotalSegments and returns the whole table.
AWS: the recorded segmented Scan returns Count 1 / ScannedCount 1 — only the single item belonging to that segment.
Emulator: 200 with Count 3 / ScannedCount 3 — every item in the table, i.e. the Segment/TotalSegments pair is not applied to this request at all.
Why: combined with Cause 4, a parallel scan both drops items (empty segments) and processes items more than once (segments that return everything); any fan-out that assumes disjoint segments double-processes records.
Fix: as Cause 3/4 — the Scan handler must restrict the scanned set to the requested Segment whenever TotalSegments is present.
Traces: dynamodb-trajwf-parallel-scan-segment-contract#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; all five are `verdict: mismatch` clusters with empty stub_path and empty owned_by. Status is 200 -> 200 in every case; the divergence is in the served item set/order.

Cause 1 (dynamodb-scan-unfiltered-number-hash-key-returns-stable-order#task#4, 2 affected):
  $.Items[0].pk.N expected '8', got '150'; $.Items[1].pk.N expected '3', got '2'; $.Items[2].pk.N expected '2', got '3'; $.Items[3].pk.N expected '255', got '8'; $.Items[4].pk.N expected '150', got '99'; $.Items[9].pk.N expected '99', got '255'

Cause 2 (dynamodb-scan-unfiltered-string-hash-key-returns-stable-order#task#4, 2 affected):
  $.Items[1].pk.S expected 'pluto', got 'earth'; $.Items[2].pk.S expected 'earth', got 'pluto'; $.Items[3].pk.S expected 'mars', got 'mercury'; $.Items[6].pk.S expected 'mercury', got 'mars'; $.Items[7].pk.S expected 'jupiter', got 'saturn'; $.Items[9].pk.S expected 'saturn', got 'jupiter'

Cause 3 (dynamodb-scan-total-segments-4096-pins-number-partition-keys#task#6, 3 affected):
  $.Count expected 1, got 0; $.ScannedCount expected 1, got 0; $.Items[0] missing (expected the item whose partition key hashes to the scanned segment)

Cause 4 (dynamodb-trajwf-parallel-scan-segment-contract#task#6, 1 affected):
  $.Count expected 2, got 0; $.ScannedCount expected 2, got 0; $.Items[0] and $.Items[1] missing

Cause 5 (dynamodb-trajwf-parallel-scan-segment-contract#task#7, 1 affected):
  $.Count expected 1, got 3; $.ScannedCount expected 1, got 3; $.Items[1] and $.Items[2] extra (the other two table items are also returned), and $.Items[0] is a different item than AWS returned

Model citations: Smithy Scan.TotalSegments -> ScanTotalSegments @range 1..1000000; Scan.Segment -> ScanSegment @range 0..999999; Scan output Items -> ItemList, Count/ScannedCount Integer default 0. The model specifies no ordering, so the ordering requirement is the recorded behavior (asserted twice per fixture, hence stable).

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay dynamodb-scan-unfiltered-number-hash-key-returns-stable-order steps task#4 and task#5 — seed ten Number partition keys, then Scan with no filter — and compare the order of $.Items.
3. Cause 2: replay dynamodb-scan-unfiltered-string-hash-key-returns-stable-order steps task#4 and task#5 — same with String partition keys.
4. Cause 3: replay dynamodb-scan-total-segments-4096-pins-number-partition-keys step task#6 — Scan with TotalSegments 4096 at the Segment that owns the seeded key — and check Count/ScannedCount/Items.
5. Cause 4: replay dynamodb-trajwf-parallel-scan-segment-contract step task#6 — the segmented Scan that must return two items.
6. Cause 5: replay dynamodb-trajwf-parallel-scan-segment-contract step task#7 — the segmented Scan that must return exactly one item; confirm the other segments' items are not returned.
7. All five must pass before closing; in particular the segments of one parallel scan must partition the table (no item missing, no item in two segments).

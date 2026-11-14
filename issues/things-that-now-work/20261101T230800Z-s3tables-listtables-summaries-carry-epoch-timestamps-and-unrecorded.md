Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-list-tables-pagination-and-prefix
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-list-tables-pagination-and-prefix s3tables-rename-table-conflict-matrix s3tables-table-create-and-inspect s3tables-table-lifecycle-iceberg
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[*]
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl0_297c7750"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl0_297c7750"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl1_297c7750"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl1_297c7750"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl2_297c7750"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl2_297c7750"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_44edf3d0"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_44edf3d0"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_one_44edf3d0"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_44edf3d0"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_44edf3d0"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_tbl_two_44edf3d0"].tableARN
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl_686a1f32"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl_686a1f32"].modifiedAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl_ec01b3e3"].createdAt
Issue-Key: s3tables|ListTables|2xx->2xx|$.tables[name="sim2real_probe_s3tables_tbl_ec01b3e3"].modifiedAt
Issue-Commit: 8715ba3f180c27028dee3267eb478eb6947c4fd2

# s3tables ListTables summaries carry epoch timestamps and unrecorded id members

> _Note: this issue was identified against the code base at commit `8715ba3f180c27028dee3267eb478eb6947c4fd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Seven causes, all in how ListTables renders each TableSummary. Every cause must be closed before this issue is submitted.

Cause 1 of 7: ListTables element shape (fixture s3tables-list-tables-pagination-and-prefix, step task#6).
AWS: Smithy TableSummary has required createdAt and modifiedAt Timestamp members with @timestampFormat("date-time"), serialized as ISO-8601 instant strings on this rest-json service; namespaceId and tableBucketId are OPTIONAL members and the recorded AWS response for ListTables omits them entirely (recorded behavior only for the omission — each recorded element carries exactly namespace, name, type, tableARN, createdAt, modifiedAt).
Emulator: 200, but every element renders createdAt/modifiedAt as epoch NUMBERS (1793574060.1699364) and adds namespaceId and tableBucketId members, so no element matches the recorded element at all.
Why: an SDK deserializing TableSummary.createdAt against a number fails, so paging a namespace's tables is impossible; the extra members also make a snapshot-comparing client see every table as changed.
Fix: the ListTables output serializer (timestamp rendering and which summary members are emitted).
Traces: s3tables-list-tables-pagination-and-prefix#task#6.

Cause 2 of 7: ListTables page 2 timestamps (same fixture, step task#7).
AWS: as cause 1 — createdAt/modifiedAt are ISO-8601 date-time strings.
Emulator: elements tbl0/tbl1/tbl2 carry createdAt/modifiedAt as epoch numbers (1793574060.1699364, 1793574060.1745248, 1793574060.178799).
Why/Fix: as cause 1.
Traces: s3tables-list-tables-pagination-and-prefix#task#7.

Cause 3 of 7: ListTables prefix-filtered page (same fixture, step task#8).
AWS: as cause 1.
Emulator: the single matching element carries createdAt/modifiedAt as epoch numbers.
Why/Fix: as cause 1.
Traces: s3tables-list-tables-pagination-and-prefix#task#8.

Cause 4 of 7: ListTables after rename-conflict probes (fixture s3tables-rename-table-conflict-matrix, step task#14).
AWS: as cause 1, plus TableSummary.tableARN is modeled ^arn:...:bucket/<name>/table/[a-zA-Z0-9-_]{1,255}$ and the recorded element's shape assertion on its identifier segment rejects the emulator's value (recorded behavior only; same generated-identifier form reported in the companion issue on epoch timestamps and non-UUID server ids).
Emulator: elements sim2real_tbl_one/sim2real_tbl_two carry epoch createdAt/modifiedAt and tableARNs ending in 32-hex digests.
Why/Fix: as cause 1, plus the id minter in services/service-com-amazonaws-s3tables-s3tablebuckets/src/state.rs:351-362.
Traces: s3tables-rename-table-conflict-matrix#task#14.

Cause 5 of 7: ListTables after a successful rename in the conflict matrix (same fixture, step task#15).
AWS: as cause 4.
Emulator: the remaining element carries epoch createdAt/modifiedAt and a tableARN of the same rejected form.
Why/Fix: as cause 4.
Traces: s3tables-rename-table-conflict-matrix#task#15.

Cause 6 of 7: ListTables after create-and-inspect (fixture s3tables-table-create-and-inspect, step task#6).
AWS: as cause 1; the recorded element's createdAt/modifiedAt are '2026-09-05T10:31:19.321964313Z'.
Emulator: 1793574081.4236898 for both.
Why/Fix: as cause 1.
Traces: s3tables-table-create-and-inspect#task#6.

Cause 7 of 7: ListTables in the Iceberg lifecycle (fixture s3tables-table-lifecycle-iceberg, step task#6).
AWS: as cause 1.
Emulator: createdAt/modifiedAt = 1793574082.1143086.
Why/Fix: as cause 1.
Traces: s3tables-table-lifecycle-iceberg#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (7 ListTables clusters, status 200 expected and returned in all of them):
- s3tables-list-tables-pagination-and-prefix#task#6: $.tables[0]/$.tables[1] reported missing-then-extra; recorded element = {namespace, name, type, tableARN, createdAt, modifiedAt}, emulator element = the same plus 'namespaceId': '322e5e076329818b3f526630fec31ca3', 'tableBucketId': '9d0813b79b3bdac393292f913f0b18bc', with 'createdAt': 1793574060.1699364 where the recording has a date-time string.
- #task#7: $.tables[name="...tbl0..."].createdAt/.modifiedAt expected a date-time string, got 1793574060.1699364 (likewise tbl1, tbl2).
- #task#8: same on tbl0.
- s3tables-rename-table-conflict-matrix#task#14: createdAt/modifiedAt epoch numbers and $.tables[...].tableARN shape assertion failed, got 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-probe-s3t-rename-44edf3d0/table/cab0c784d0914d61e56e71726c720321'.
- #task#15: same on the surviving element.
- s3tables-table-create-and-inspect#task#6: expected '2026-09-05T10:31:19.321964313Z', got 1793574081.4236898.
- s3tables-table-lifecycle-iceberg#task#6: expected a date-time string, got 1793574082.1143086.
The literal instants and digests are per-run; the requirement is the serialized FORM and the member set.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Fixture s3tables-list-tables-pagination-and-prefix, steps task#6, task#7, task#8 — inspect each element of $.tables: createdAt/modifiedAt type, and whether namespaceId/tableBucketId are present.
3. Repeat for s3tables-rename-table-conflict-matrix#task#14 and #task#15, s3tables-table-create-and-inspect#task#6, s3tables-table-lifecycle-iceberg#task#6.

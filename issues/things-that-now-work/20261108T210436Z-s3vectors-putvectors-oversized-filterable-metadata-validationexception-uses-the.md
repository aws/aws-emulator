Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit|run#34|AssertionError|assert message == expected_message, (
Issue-Commit: 622f38417b8d75b0cfd881842d16d49c14a6c801

# s3vectors PutVectors' oversized-filterable-metadata ValidationException uses the emulator's own clause instead of AWS's "Filterable metadata must have at most <n> bytes"

> _Note: this issue was identified against the code base at commit `622f38417b8d75b0cfd881842d16d49c14a6c801`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutVectors

## Description
ONE net-new root cause in the s3vectors emulator's PutVectors refusal contract, grounded in a failing behavioral check the replay report left unmarked (neither ALREADY FILED nor OWNED). The refusal happens at the right moment with the right HTTP status and the right error code; what diverges is the `message` member the client reads.

Neighbouring issues, named so a fixer does not mistake them for coverage:
* FIXED 20261029T195812Z-s3vectors-s3-vectors-refusal-messages-diverge-on-8-further.md asked (Causes 5 and 7) for PutVectors' per-record ValidationException to be worded "Invalid record for key '<key>': <clause>" for the dimension-mismatch and zero-norm rules. That fix LANDED: the emulator now emits exactly that prefix with the offending record's key, and this check confirms it. Progression note, NOT a reopen and NOT a refusal the fix left in place (AWS refuses this request too, with the same status and code): the cause below is the RESIDUAL divergence in the <clause> for a DIFFERENT per-record rule — the filterable-metadata byte budget — which that issue does not mention at all.
* OPEN 20261102T105452Z-s3vectors-3-net-new-causes-putvectors-accepts-a.md covers PutVectors duplicate keys and the QueryVectors/ListVectors sentences; it says nothing about metadata budgets.
* OPEN 20261103T033818Z-s3vectors-6-net-new-causes-listvectors-queryvectors-refusal.md is confined to ListVectors/QueryVectors refusal wording and fieldList paths.

=== Cause 1 of 1: the filterable-metadata-over-budget clause is emulator-worded ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models PutVectors (POST /PutVectors, http 200, @idempotent) with required `vectors` (PutVectorsInputList, @length min 1 max 500) and ValidationException (errorKind client, httpError 400 [defaulted], required `message`, optional `fieldList`); each entry carries a `key` and an optional `metadata` document, and the bound index's MetadataConfiguration.nonFilterableMetadataKeys declares which metadata keys are exempt from the filterable budget. The model does not state the byte budgets, so recorded behavior and the service's published limits supply them: filterable metadata is capped per vector (2048 bytes) and total metadata at a much larger cap (40 KiB). A PutVectors record whose FILTERABLE metadata (any key the index did not declare non-filterable) exceeds the filterable cap is refused with HTTP 400 ValidationException and `message` exactly "Invalid record for key '<vectorKey>': Filterable metadata must have at most <filterableCapBytes> bytes". Fixed text: "Invalid record for key '", "': Filterable metadata must have at most ", " bytes". Variable parts: the offending record's `key`, taken from the request, and the service's filterable-metadata cap in bytes (2048) — the SUBMITTED size is NOT interpolated.
Emulator: HTTP 400 ValidationException (correct status and code, and the "Invalid record for key '<key>': " prefix is correct) with the clause replaced by the emulator's own sentence — observed message "Invalid record for key 'vec-big-filterable': filterable metadata size exceeds the maximum allowed size of 2048 bytes". Lower-cased "filterable", "size exceeds the maximum allowed size of" in place of "must have at most". ('vec-big-filterable' is this run's record key filling the template, not part of it.)
Why: PutVectors answers ValidationException for every per-record defect it can report — wrong vector width, degenerate vector, bad metadata document, over-budget metadata — so the `message` clause is the only discriminator a client has. An embedding/ingestion pipeline branches on it to decide whether to RESHAPE the vector (dimension defect), DROP the record (degenerate vector) or MOVE an oversized attribute under a non-filterable metadata key (this defect). A client that matches or surfaces AWS's wording takes its fallback path here and the intended path against AWS, and no assertion on the service's sentence can hold.
Fix: PutVectors' per-record metadata-budget validation — the clause built by `crate::state::metadata_budget::metadata_budget_defect` for the filterable-budget branch, as lifted by PutVectors' handler through `LookupError::invalid_record`. Only the clause text changes; the record-key prefix, the 400 and the ValidationException class are already right.
Traces: s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 96/136 passed, 37 mismatched, 3 errored, 0 skipped, bootstrap_signal false; the s3vectors service itself scored 81/84. Of the three s3vectors failures, two are OWNED by 20261102T105452Z-s3vectors-3-net-new-causes-putvectors-accepts-a.md and were skipped; the one below carried no stub_path and no owner.

Cause 1 — check s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit. Setup: CreateVectorBucket(<fresh name>), CreateIndex(dataType=float32, dimension=4, distanceMetric=euclidean, metadataConfiguration={nonFilterableMetadataKeys:['chunk']}); GetIndex confirms the declaration round-trips. The check then submits ONE 8192-byte payload (strictly between the 2048-byte filterable cap and the 40 KiB total cap) twice, changing only the metadata KEY:
  * under the DECLARED non-filterable key 'chunk' -> accepted and read back verbatim through GetVectors(returnMetadata=true) — this leg PASSES, so nonFilterableMetadataKeys is honoured;
  * under the UNDECLARED filterable key 'body' (record key 'vec-big-filterable') -> expected HTTP 400 ValidationException with message "Invalid record for key 'vec-big-filterable': Filterable metadata must have at most 2048 bytes"; ACTUAL HTTP 400 ValidationException with message "Invalid record for key 'vec-big-filterable': filterable metadata size exceeds the maximum allowed size of 2048 bytes".
AssertionError: unexpected ValidationException message for 8192 B of filterable metadata: "Invalid record for key 'vec-big-filterable': filterable metadata size exceeds the maximum allowed size of 2048 bytes".

The HTTP status (400), the error code (ValidationException), the "Invalid record for key '<key>': " prefix and the interpolated record key all MATCH; only the clause after the colon diverges. The small-filterable positive control, the effect-absent reads (GetVectors returns [] and ListVectors omits the refused key while retaining the accepted vectors) and the restore leg (the same key with an under-cap value is accepted and readable) all pass, so the refusal is made at the right moment and writes nothing — the wording is the whole divergence.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the '### s3vectors' section of the failing-check list. The grounding check is s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit; re-replay exactly that check to confirm the fix.
1. CreateVectorBucket(vectorBucketName=<fresh lower-case name>).
2. CreateIndex(vectorBucketName=<that bucket>, indexName=<fresh name>, dataType=float32, dimension=4, distanceMetric=euclidean, metadataConfiguration={'nonFilterableMetadataKeys': ['chunk']}); GetIndex to confirm nonFilterableMetadataKeys == ['chunk'].
3. Control: PutVectors(vectors=[{key:'vec-small-filterable', data:{float32:[0.125,-0.25,0.5,-0.75]}, metadata:{'body': 's'*64}}]) -> 200, and GetVectors(returnMetadata=true) reads the value back.
4. Exemption control: PutVectors with the SAME 8192-character payload under the DECLARED key -> [{key:'vec-big-non-filterable', data:{float32:[...]}, metadata:{'chunk': 'x'*8192}}] -> 200, read back verbatim.
5. The cause: PutVectors with that identical 8192-character payload under an UNDECLARED key -> [{key:'vec-big-filterable', data:{float32:[...]}, metadata:{'body': 'x'*8192}}] -> read $.message of the 400 and compare it to "Invalid record for key 'vec-big-filterable': Filterable metadata must have at most 2048 bytes".
6. Confirm nothing was written: GetVectors(keys=['vec-big-filterable'], returnMetadata=true) returns an empty vectors list and ListVectors does not list that key while still listing the two accepted ones.

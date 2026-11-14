Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog
Issue-Key: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count|run#20|AssertionError|assert copied["ResultRows"] == 0 and copied["ResultSize"] == 0, (
Issue-Key: redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty|run#25|AssertionError|assert denied["Status"] == "FAILED", (
Issue-Key: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog|_run_sql#20|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Commit: 04af5b44e533c3de8f990e2c6a103d8fa8770847

# redshift cluster SQL engine: 3 net-new causes — a COPY's DescribeStatement reports loaded rows in ResultRows and -1 in ResultSize, COPY ignores MAXERROR so a breaching load FINISHES, and ANALYZE COMPRESSION <table> is a parser error

> _Note: this issue was identified against the code base at commit `04af5b44e533c3de8f990e2c6a103d8fa8770847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Replay of simparity-behavioral-aws-v1 against the emulated Redshift cluster data plane (redshift-data ExecuteStatement/DescribeStatement reaching a provisioned cluster) left three failing checks that no filed issue owns. This bundle carries THREE causes, all in the cluster SQL engine / statement-record layer behind ExecuteStatement; all three must be closed before submitting.

Related but NOT covering them: the open issue 20261029T193810Z-redshift-cluster-sql-engine-multi-statement-sql-refused.md (its Cause 4 is COPY failing to PARSE at all, its Cause 5 is the ENCODE column option failing to parse) and the resolved 20261024T024248Z-redshift-cluster-sql-engine-rejects-core-redshift-statements.md (COPY/UNLOAD/SET/SHOW unparsed). COPY and ENCODE now parse, so the replay has advanced PAST those faces to the three below; the suite marked none of these three checks OWNED and none of these three faces is described in either issue.

=== Cause 1 of 3: a COPY's statement record reports the loaded row count in ResultRows and a null ResultSize, where AWS reports 0/0 ===
AWS: Smithy (com.amazonaws.redshiftdata) DescribeStatementResponse.ResultRows = "Either the number of rows returned from the SQL statement or the number of rows affected... A -1 indicates the value is null", ResultSize = "The size in bytes of the returned results. A -1 indicates the value is null.", HasResultSet = "A value that indicates whether the statement has a result set." Recorded behavior: a COPY ... FROM 's3://...' IAM_ROLE '...' CSV that FINISHES reports HasResultSet false, ResultRows 0 and ResultSize 0 — the number of rows the load ingested is NOT surfaced on the statement record (it is read back in-session with pg_last_copy_count(), or from the table itself).
Emulator: the same COPY reports ResultRows = 3 (the number of rows in the S3 object) and ResultSize = -1. So both members diverge at once: a count AWS keeps at 0, and the model's explicit null sentinel -1 where AWS reports the real value 0.
Why: a client that reads ResultSize to decide whether a result set is worth fetching sees -1 ("null") on a statement AWS reports as a definite 0-byte result, and a client that reads ResultRows to count AFFECTED rows of a DML statement now cannot distinguish a COPY (which AWS leaves at 0) from an INSERT/UPDATE/DELETE. Load-accounting code calibrated on AWS double-counts or mis-reports every COPY.
Fix: the statement-record projection for COPY behind DescribeStatement (the redshift-data statement store that fills ResultRows/ResultSize when a statement goes terminal) — leave ResultRows at 0 for a COPY and report ResultSize 0 rather than the -1 null sentinel; keep the ingested count only in the engine's session state that pg_last_copy_count() reads.
Traces: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count.

=== Cause 2 of 3: COPY ignores MAXERROR — a load whose bad-row count reaches the threshold FINISHES and commits instead of failing and rolling back ===
AWS: recorded behavior only (the Smithy model describes no SQL surface). A Redshift COPY is one transaction and MAXERROR n is its tolerance: when the number of rows the loader cannot convert REACHES n, the load fails, nothing is committed, and the statement record goes Status FAILED with Redshift's own load error — fixed text "ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system table for details." (two spaces after the period, as AWS words it), with <table> taken from the COPY statement. The rejected rows are individually accounted for in stl_load_errors (line_number, colname, raw_field_value), and the identical COPY under MAXERROR n+1 FINISHES and loads the well-formed rows.
Emulator: a COPY of ten CSV lines, two of which carry a non-integer value for an INT column, run with MAXERROR 2, goes Status 'FINISHED' with Error None. The threshold has no effect, so the load is neither failed nor rolled back and the load-failure error above is never produced.
Why: MAXERROR is the contract that keeps a dirty file out of a table. A pipeline that relies on COPY failing and rolling back on a breaching file gets a silent partial/complete load in the emulator, then diverges from AWS on both the table's contents and the error-path branch (catch the failure, read stl_load_errors) that never runs.
Fix: the cluster SQL engine's COPY executor and the state it touches — count per-row conversion failures during the load, record each in stl_load_errors, and when the count reaches MAXERROR abort the COPY's single transaction (committing no row) and store Redshift's load-failure message as the statement record's Error with Status FAILED.
Traces: redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty.

=== Cause 3 of 3: ANALYZE COMPRESSION <table> is rejected as a syntax error ===
AWS: recorded behavior only. Redshift accepts ANALYZE COMPRESSION <table>: the statement FINISHES, HasResultSet is true, and GetStatementResult returns the recommendation as columns named exactly ["Table", "Column", "Encoding", "Est_reduction_pct"], one record per column of the analysed table, with a non-RAW encoding and a positive estimated reduction for a low-cardinality column; the recommended encoding is then usable as a CREATE TABLE ... ENCODE <encoding> option and visible in pg_table_def (an ENCODE RAW column reads back as 'none').
Emulator: the statement goes terminal Status='FAILED' with Error='near "<table>": syntax error in ANALYZE COMPRESSION <table> at offset <position of the table name>' — i.e. the parser recognises ANALYZE but not the COMPRESSION form, so it fails on the table name. Fixed text of that message is the emulator's own ('near "...": syntax error in ... at offset N'), not Redshift wording, and no result set is ever produced.
Why: ANALYZE COMPRESSION is the documented way to choose column encodings; every schema-tuning client gets a syntax error real Redshift never returns, and the whole compression surface (the recommendation result set, the catalog-visible encoding) is unreachable.
Fix: the cluster SQL engine's statement parser plus an ANALYZE COMPRESSION executor and its result-set projection — parse ANALYZE COMPRESSION <table> [ (<columns>) ], sample the stored rows, and return the four-column recommendation result set so HasResultSet is true and GetStatementResult serves it.
Traces: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog.

## Evidence
Suite: simparity-behavioral-aws-v1 — service heading `### redshift`: 113/187 (60%), 74 failed, of which 73 were marked OWNED/ALREADY FILED and skipped. These three are the net-new remainder.

Cause 1 — check_redshift_rdapi_copy_from_s3_loads_object_rows_and_session_reports_count.py:67
  statement: COPY <table> (id, name, amt) FROM 's3://<bucket>/<key>.csv' IAM_ROLE '<role arn>' CSV
  expected: Status FINISHED, HasResultSet false, ResultRows == 0 and ResultSize == 0
  actual:   AssertionError: a COPY reports ResultRows/ResultSize 0 -- the loaded count is NOT surfaced there; got 3/-1
  (3 is the number of CSV rows in the object; the requirement is 0 and 0 regardless of the file.)

Cause 2 — check_redshift_redshift_copy_exceeding_maxerror_fails_and_leaves_the_table_empty.py:398
  statement: COPY <table> (id, name, qty) FROM 's3://<bucket>/<key>.csv' IAM_ROLE '<role arn>' CSV MAXERROR 2
            over ten CSV lines, two of which hold a non-integer qty
  expected: Status FAILED with Error "ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system table for details."; SELECT count(*) -> 0; stl_load_errors accounting for both bad lines by line_number/colname/raw_field_value; the identical COPY under MAXERROR 3 FINISHED with the eight well-formed rows
  actual:   AssertionError: two malformed rows must breach MAXERROR 2 and fail the load, got 'FINISHED' error=None

Cause 3 — check_redshift_redshift_analyze_compression_picks_a_non_raw_encoding_visible_in_the_catalog.py:320 (via _run_sql:240)
  statement: ANALYZE COMPRESSION <table>  (table holding 20000 rows over 5 distinct VARCHAR values)
  expected: Status FINISHED, HasResultSet true, result columns ["Table", "Column", "Encoding", "Est_reduction_pct"], a non-RAW encoding with Est_reduction_pct > 0
  actual:   AssertionError: DATAPLANE_UNIMPLEMENTED: SQL 'ANALYZE COMPRESSION <table>' did not FINISH: status='FAILED' error='near "<table>": syntax error in ANALYZE COMPRESSION <table> at offset 20'

(bucket names, role ARNs, cluster ids, table names, row counts and parser offsets are per-run examples; the requirements are the statuses, the 0/0 statement-record values, Redshift's load-failure message template and the ANALYZE COMPRESSION result-set shape.)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): DescribeStatementResponse.ResultRows (Long, default 0) 'Either the number of rows returned from the SQL statement or the number of rows affected... A -1 indicates the value is null'; DescribeStatementResponse.ResultSize (Long, default 0) 'The size in bytes of the returned results. A -1 indicates the value is null.'; DescribeStatementResponse.Error 'The error message from the cluster if the SQL statement encountered an error while running.'

## Steps to Reproduce
1. Clone the repo, boot the emulator. CreateCluster a single-node Redshift cluster with an attached IAM role granting s3:GetObject/s3:ListBucket on a test bucket, wait for 'available', and point a botocore redshift-data client at the emulator with ClusterIdentifier + DbUser.
2. Create a bucket and PutObject a 3-line CSV ('1,alpha,10.50' ...). ExecuteStatement 'CREATE TABLE t (id INT, name VARCHAR(32), amt DECIMAL(9,2))'.
3. Cause 1: ExecuteStatement "COPY t (id, name, amt) FROM 's3://<bucket>/<key>' IAM_ROLE '<arn>' CSV", poll DescribeStatement to FINISHED -> expect HasResultSet false, ResultRows 0, ResultSize 0; observe ResultRows 3 and ResultSize -1.
4. Cause 2: PutObject a 10-line CSV whose lines 3 and 7 hold a non-integer in the INT column; CREATE TABLE t2 (id INT, name VARCHAR(32), qty INT); ExecuteStatement "COPY t2 (id, name, qty) FROM 's3://<bucket>/<key2>' IAM_ROLE '<arn>' CSV MAXERROR 2" -> expect Status FAILED with Error "ERROR: Load into table 't2' failed.  Check 'stl_load_errors' system table for details.", SELECT count(*) FROM t2 == 0, and both bad lines in stl_load_errors; observe Status FINISHED with Error None. Re-run the identical COPY with MAXERROR 3 -> expect FINISHED with the eight good rows.
5. Cause 3: CREATE TABLE t3 (v VARCHAR(64) ENCODE RAW), insert 20000 rows over 5 distinct values, then ExecuteStatement 'ANALYZE COMPRESSION t3' -> expect FINISHED with HasResultSet true and GetStatementResult columns [Table, Column, Encoding, Est_reduction_pct]; observe FAILED with 'near "t3": syntax error in ANALYZE COMPRESSION t3 at offset 20'.
6. Re-replay every check id in trace_ids to confirm the fix.

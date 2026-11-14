Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it redshift.redshift-wlm-queue-assignment-is-observable-in-the-cluster-system-tables redshift.static-parameter-change-needs-a-reboot-before-the-engine-sees-it
Issue-Key: redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors|run#17|AssertionError|assert failed.get("Error") == _EXPECTED_ERROR, (
Issue-Key: redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty|run#30|AssertionError|assert "S3ServiceException" in error, (
Issue-Key: redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them|_run_sql#26|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it|run#23|AssertionError|assert unload_status["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-wlm-queue-assignment-is-observable-in-the-cluster-system-tables|run#18|AssertionError|assert tagged["Status"] == "FINISHED", (
Issue-Key: redshift.static-parameter-change-needs-a-reboot-before-the-engine-sees-it|_run_sql#39|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Redshift cluster SQL engine rejects core Redshift statements as syntax errors: COPY, UNLOAD, CREATE EXTERNAL SCHEMA, SET and SHOW all fail to parse

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One root cause with five observable faces: the SQL engine behind the emulator's Redshift clusters implements a generic SQL dialect and does not parse Redshift's own statement vocabulary. Every statement below comes back with status FAILED and a parser error of the form `near "<KEYWORD>": syntax error in <the whole statement> at offset N`, which means the keyword was never recognised at all.

AWS: a Redshift cluster parses and executes COPY (S3 ingest with IAM_ROLE / CSV), UNLOAD (S3 export with MANIFEST / DELIMITER / ALLOWOVERWRITE), CREATE EXTERNAL SCHEMA ... FROM DATA CATALOG (Spectrum), SET <parameter> TO <value> (session parameters such as query_group, observable in the WLM system tables) and SHOW <parameter>. Recorded behavior: a COPY whose source rows are malformed does NOT fail with a parser error — it fails with the engine's load-failure text and records the rejected rows in stl_load_errors; recorded example message: "ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system table for details." A COPY whose IAM role cannot read the S3 object fails with S3's access denial raised inside the cluster's own request, not a parse error. These are statements of the engine, not of the Smithy model: the Redshift Smithy model describes only the control plane and says nothing about SQL, so this is recorded behavior only.
Emulator: each statement is rejected by the parser before any execution. Observed statuses are FAILED with:
  - COPY -> error 'near "COPY": syntax error in COPY <table> (...) FROM \'s3://...\' IAM_ROLE \'arn:aws:iam::...\' CSV at offset 0'
  - UNLOAD -> error 'near "UNLOAD": syntax error in UNLOAD (...) TO \'s3://...\' IAM_ROLE ... MANIFEST DELIMITER \'|\' ALLOWOVERWRITE at offset 0'
  - CREATE EXTERNAL SCHEMA -> error 'near "EXTERNAL": syntax error in CREATE EXTERNAL SCHEMA ... FROM DATA CATALOG DATABASE ... CREATE EXTERNAL DATABASE IF NOT EXISTS at offset 7'
  - SET -> error 'near "SET": syntax error in SET query_group TO \'sim2real\' at offset 0'
  - SHOW -> error 'near "SHOW": syntax error in SHOW enable_case_sensitive_identifier at offset 0'
Because the parse fails, the downstream behavior the checks grade can never appear: no rows are loaded or exported, stl_load_errors is never populated, the S3 permission failure is never surfaced as S3's own error, no Spectrum external table returns rows, no query_group label shows up in the WLM system tables, and a static parameter change can never be observed as pending-then-applied across a reboot.
Why: every data-loading, data-export, Spectrum and session-tuning workload against an emulated Redshift cluster fails with a parser error that real Redshift never returns, and clients cannot distinguish "my SQL is wrong" from "this statement is unsupported here". A client that handles the documented load-failure path (catch the failure, read stl_load_errors) gets an unrelated syntax error instead.
Fix: the cluster SQL engine's statement parser and the executors behind it — add the Redshift statement forms (COPY, UNLOAD, CREATE EXTERNAL SCHEMA/DATABASE, SET/RESET, SHOW) and the state they touch (loaded table rows, stl_load_errors, S3 object read/write through the S3 service, external catalog bindings, session parameters and the WLM system tables). Related but NOT the same cause: the open issue 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md covers redshift-data-side dialect gaps (absent database silently FINISHES, CREATE SCHEMA / LOCK TABLE / :: casts); these five statement families are not named there.
Traces: the five check ids in trace_ids.

## Evidence
redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors:
  AssertionError: the async record must carry the engine's load-failure text naming the table and stl_load_errors; got 'near "COPY": syntax error in COPY sim2real_rdapi_copy_bad (id, name, amt) FROM \'s3://<bucket>/<key>.csv\' IAM_ROLE \'arn:aws:iam::000000000000:role/<role>\' CSV at offset 0' want "ERROR: Load into table 'sim2real_rdapi_copy_bad' failed.  Check 'stl_load_errors' system table for details."

redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty:
  AssertionError: the failure must be S3's, raised inside the cluster's own request, got 'near "COPY": syntax error in COPY sim2real_behav_copy_denied (id, v) FROM \'s3://<bucket>/rows.csv\' IAM_ROLE \'arn:aws:iam::000000000000:role/<role>\' CSV at offset 0'

redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them:
  AssertionError: DATAPLANE_UNIMPLEMENTED: SQL "CREATE EXTERNAL SCHEMA <s> FROM DATA CATALOG DATABASE '<db>' IAM_ROLE '<arn>' CREATE EXTERNAL DATABASE IF NOT EXISTS" did not FINISH: status='FAILED' error='near "EXTERNAL": syntax error ... at offset 7'

redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it:
  AssertionError: DATAPLANE_UNIMPLEMENTED: UNLOAD must FINISH, got 'FAILED' error='near "UNLOAD": syntax error in UNLOAD (\'SELECT id, v FROM sim2real_behav_unload\') TO \'s3://<bucket>/out/\' IAM_ROLE \'<arn>\' MANIFEST DELIMITER \'|\' ALLOWOVERWRITE at offset 0'

redshift.redshift-wlm-queue-assignment-is-observable-in-the-cluster-system-tables:
  AssertionError: DATAPLANE_UNIMPLEMENTED: the labelled batch must FINISH, got 'FAILED' error='near "SET": syntax error in SET query_group TO \'sim2real\' at offset 0'

redshift.static-parameter-change-needs-a-reboot-before-the-engine-sees-it:
  AssertionError: DATAPLANE_UNIMPLEMENTED: SQL 'SHOW enable_case_sensitive_identifier' did not FINISH: status='FAILED' error='near "SHOW": syntax error in SHOW enable_case_sensitive_identifier at offset 0'

(bucket names, role ARNs, table names and generated ids are per-run examples; the requirement is that each statement parses and executes.)

## Steps to Reproduce
1. Boot the emulator, provision a Redshift cluster and reach it through the Data API / SQL path the checks use.
2. Create a table and run `COPY <table> (cols) FROM 's3://<bucket>/<key>.csv' IAM_ROLE '<arn>' CSV`. Expect it to parse and execute; with malformed rows expect the load-failure error naming the table and stl_load_errors, and rows recorded in stl_load_errors. Observe a `near "COPY": syntax error` instead.
3. Repeat the COPY with an IAM role that cannot read the object. Expect S3's access denial raised inside the cluster request and the table left empty.
4. Run `UNLOAD ('SELECT ...') TO 's3://<bucket>/out/' IAM_ROLE '<arn>' MANIFEST DELIMITER '|' ALLOWOVERWRITE`. Expect FINISHED plus manifest/part objects in S3.
5. Run `CREATE EXTERNAL SCHEMA <s> FROM DATA CATALOG DATABASE '<db>' IAM_ROLE '<arn>' CREATE EXTERNAL DATABASE IF NOT EXISTS`, then select from an external table. Expect rows from S3 without loading them.
6. Run `SET query_group TO 'sim2real'` followed by a query, then read the WLM system tables for the label.
7. Run `SHOW enable_case_sensitive_identifier`. Expect the parameter value.
8. Replay the trace_ids checks to confirm.

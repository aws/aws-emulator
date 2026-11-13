Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error
Issue-Key: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error|run#19|AssertionError|assert bad_marker.encode("utf-8") not in delivered, (
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose__glue__s3: a record that does not match the Glue schema is written into the Parquet object instead of being diverted to the ErrorOutputPrefix, so the malformed-data vs parse-error distinction is unobservable

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One net-new root cause.

=== Cause 1 of 1: DataFormatConversionConfiguration does not reject schema-mismatched records to the ErrorOutputPrefix — they are concatenated into the delivered object ===
AWS: recorded behavior only (DataFormatConversionConfiguration, SchemaConfiguration and ExtendedS3DestinationConfiguration.ErrorOutputPrefix are modelled, but the per-record conversion verdict is not). AWS behavior: on a stream with DataFormatConversionConfiguration Enabled and a Glue SchemaConfiguration, each inbound JSON record is validated against the table schema. Records that convert are written to the main prefix as a Parquet file (magic `PAR1` at both head and tail); a record whose values do not match the declared column types is NOT converted and is instead written under the ErrorOutputPrefix as a JSON error record carrying a conversion error code, which distinguishes malformed data from a JSON parse error. The malformed record's bytes never appear in the Parquet object.
Emulator: the schema-mismatched record is written straight into the delivered main-prefix object. The check finds its marker inside the delivered bytes, so no error-prefix diversion happened and the two conversion error classes cannot be told apart.
Why: a client that reads the main prefix with a Parquet reader gets an object containing raw unconverted JSON (and loses the type guarantees it configured conversion for), while its error-prefix reconciliation job sees nothing — bad records are silently mixed into good data instead of being quarantined with a code that says why.
Fix: the Firehose S3 delivery worker's format-conversion stage — the per-record schema validation and the ErrorOutputPrefix writer that must receive the rejected record plus its conversion error code, rather than appending it to the main-prefix buffer.
Traces: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error.

Related open issue 20261025T065545Z-firehose-glue-s3-a-delivery-stream-with-dataformatconversionconfiguration.md owns the sibling check firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix (which still shows the delivered object is not Parquet at all: head b'{"ki'); it does not carry this check's replay key, so this cause is filed here.

## Evidence
firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error:
  assert bad_marker.encode("utf-8") not in delivered, (...)
  AssertionError: a record that should have been diverted to the error prefix was written into the Parquet object instead: 'pqmismatch-<id>'

Expected: the main-prefix object is a Parquet file (bytes begin and end with PAR1) containing only the records that matched the Glue schema; the schema-mismatched record appears only under ErrorOutputPrefix, in a JSON error record whose conversion error code identifies it as malformed data rather than a parse error.
Actual: the schema-mismatched record's marker is present in the main-prefix object's bytes, and no error-prefix object carries it. (The sibling check in this service shows the main object is not Parquet at all — head b'{"ki', tail b'eb5\n'.)

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Create an S3 bucket and a Glue database + table with typed columns (e.g. marker string, n int).
3. CreateDeliveryStream (DirectPut) with ExtendedS3DestinationConfiguration: Prefix for the main output, ErrorOutputPrefix set, and DataFormatConversionConfiguration {Enabled: true, SchemaConfiguration: <that database/table>, InputFormatConfiguration: JSON, OutputFormatConfiguration: Parquet}. Wait ACTIVE.
4. PutRecordBatch two records: one matching the schema, and one whose value for a typed column does not match it (carrying a unique marker).
5. Read the main prefix object: the mismatched record's marker is inside it, and the object is not Parquet. Read the ErrorOutputPrefix: no error record for it, so no conversion error code is available.

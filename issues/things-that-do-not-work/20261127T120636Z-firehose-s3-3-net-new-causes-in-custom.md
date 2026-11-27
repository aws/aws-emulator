Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__s3.firehose-random-string-prefix-expression-evaluates-to-eleven-characters firehose__s3.firehose-timestamp-prefix-day-of-year-letter-expands firehose__s3.firehose-timestamp-prefix-single-quoted-text-is-literal
Issue-Key: firehose__s3.firehose-random-string-prefix-expression-evaluates-to-eleven-characters|run#6|AssertionError|assert len(segment) == 11, (
Issue-Key: firehose__s3.firehose-timestamp-prefix-day-of-year-letter-expands|run#5|AssertionError|assert segment in days, (
Issue-Key: firehose__s3.firehose-timestamp-prefix-single-quoted-text-is-literal|run#6|AssertionError|assert segment in expected, (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose__s3: 3 net-new causes in custom S3 prefix expression evaluation — !{firehose:random-string} is 32 characters, !{timestamp:DDD} is not expanded at all, and single-quoted literal text in a timestamp pattern is mangled

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Three independent defects in the evaluator that expands a delivery stream's custom S3 object-key Prefix. ALL THREE must be closed before this issue is resubmitted.

AWS reference for all three: Smithy ExtendedS3DestinationConfiguration$Prefix — "The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3 files. You can also specify a custom prefix, as described in Custom Prefixes for Amazon S3 Objects" (https://docs.aws.amazon.com/firehose/latest/dev/s3-prefixes.html). Custom prefixes support the !{firehose:...} expressions and !{timestamp:<pattern>}, whose pattern is a Java DateTimeFormatter pattern evaluated against the record's approximate arrival timestamp in the configured time zone. The delivered object's key is the expanded prefix followed by the stream's generated object-name suffix.

Cause 1 of 3: !{firehose:random-string} expands to a 32-character value instead of the documented 11-character random string.
  AWS: the random-string expression evaluates to exactly 11 characters (documented in Custom Prefixes for Amazon S3 Objects). The particular characters are per-run; the LENGTH of 11 is the requirement.
  Emulator: the segment evaluated to a 32-character lower-case hex value, e.g. '5b5a23c046c64290b85299ba1b57a5d4' (32 chars) — a UUID-shaped token, not an 11-character random string.
  Why: a consumer that partitions, parses or length-validates delivered keys built from !{firehose:random-string} sees a different key layout than in AWS; key-prefix parsers and fixed-width key schemas break, and keys grow by 21 characters per object.
  Traces: firehose__s3.firehose-random-string-prefix-expression-evaluates-to-eleven-characters.

Cause 2 of 3: the day-of-year pattern letter is not supported — !{timestamp:DDD} is emitted literally as 'DDD'.
  AWS: 'D' is the day-of-year field of the timestamp pattern, so !{timestamp:DDD} expands to the zero-padded day of year of the record's arrival time (3 digits, e.g. '331' for 27 November 2026). The digits are a function of the clock, not a fixed value; the requirement is that the field is EXPANDED to the day of year rather than passed through.
  Emulator: the prefix segment is the literal text 'DDD' — the key delivered was 'doy/DDD/<stream>-1-<...>' where AWS produces 'doy/<day-of-year>/...'. The pattern letter is simply not implemented, so an unsupported letter falls through to the output.
  Why: every object of such a stream lands in one unpartitioned literal 'DDD' folder instead of a per-day partition. Date-partitioned consumers (Athena/Glue partition projection, lifecycle rules, per-day batch jobs) read nothing for any day, and the un-expanded pattern text silently becomes part of the key namespace.
  Traces: firehose__s3.firehose-timestamp-prefix-day-of-year-letter-expands.

Cause 3 of 3: single-quoted literal text inside a timestamp pattern is not treated as a literal, and characters inside it are corrupted.
  AWS: in a timestamp pattern, text enclosed in single quotes is literal and is emitted verbatim with the quotes removed. So the prefix segment !{timestamp:'year='yyyy} expands to "year=" followed by the 4-digit year — 'year=2026' for a record arriving in 2026.
  Emulator: the segment evaluated to "'6ear='2026" — the surrounding single quotes are kept in the output, and a character inside the quoted literal is substituted as though it were a pattern letter ('y' of "year=" became '6'). Only the trailing yyyy was expanded correctly. The evaluator has no quoting mode: it rewrites pattern letters everywhere, including inside quoted literals, and does not strip the quotes.
  Why: the documented way to write Hive-style partition keys (key='value') into a Firehose prefix produces corrupted keys. Object keys contain stray quote characters and mangled literal text, so Hive/Glue partition discovery and any key parser fail, and the corruption is silent — the stream reports healthy delivery.
  Traces: firehose__s3.firehose-timestamp-prefix-single-quoted-text-is-literal.

Fix (all three): the custom-prefix expression evaluator in the firehose service crate, used by the S3/ExtendedS3 destination writer when it composes a delivered object's key — give !{firehose:random-string} an 11-character random value; implement the timestamp pattern as a full DateTimeFormatter-compatible pattern including the day-of-year field 'D' (zero-padded to the number of pattern letters); and add a quoting mode so single-quoted runs are copied verbatim with the quotes stripped and no pattern-letter substitution applied inside them. An unsupported pattern letter must never fall through into the key as literal text. Note: open issue 20261029T175617Z-firehose-s3-a-gzip-compressed-delivered-object-is.md concerns the delivered object's ContentType, not key composition, and does not cover any cause above.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

Cause 1:
[mismatch] firehose__s3.firehose-random-string-prefix-expression-evaluates-to-eleven-characters
  File "check_firehose__s3_firehose_random_string_prefix_expression_evaluates_to_eleven_characters.py", line 227, in run
    assert len(segment) == 11, (
  AssertionError: random-string evaluated to '5b5a23c046c64290b85299ba1b57a5d4' (32 chars); expected 11 characters
Expected (AWS): the prefix segment produced by !{firehose:random-string} is 11 characters long.
Actual (emulator): a 32-character hex value. The specific characters are per-run; the length is the requirement.

Cause 2:
[mismatch] firehose__s3.firehose-timestamp-prefix-day-of-year-letter-expands
  File "check_firehose__s3_firehose_timestamp_prefix_day_of_year_letter_expands.py", line 227, in run
    assert segment in days, (
  AssertionError: !{timestamp:DDD} evaluated to 'DDD' in key 'doy/DDD/sim2real-fh-pfx-<id>-1-2026-11-27-11-55-22-<uuid>'; expected one of ['331']
Expected (AWS): the segment is the zero-padded day of year of the record's arrival time (3 digits).
Actual (emulator): the literal text 'DDD'. The value '331' shown is the day of year at replay time, a clock value, not a fixed requirement; the requirement is that the field is expanded.

Cause 3:
[mismatch] firehose__s3.firehose-timestamp-prefix-single-quoted-text-is-literal
  File "check_firehose__s3_firehose_timestamp_prefix_single_quoted_text_is_literal.py", line 228, in run
    assert segment in expected, (
  AssertionError: 'year='yyyy evaluated to "'6ear='2026" in key "lit/'6ear='2026/sim2real-fh-pfx-<id>-1-2026-11-27-11-54-38-<uuid>"; expected one of ['year=2026']
Expected (AWS): the quoted run is literal and the quotes are stripped, giving 'year=' + the 4-digit year.
Actual (emulator): "'6ear='2026" — quotes retained and the literal 'y' substituted. The year 2026 is a clock value; the fixed part of the requirement is the literal text 'year=' and the removal of the quotes.

The generated stream names, object-name suffixes and uuids in the keys are per-run values.

## Steps to Reproduce
1. Boot the emulator from this checkout. Create an S3 bucket and a firehose.amazonaws.com-trusted role with the S3 write set on it.
2. CreateDeliveryStream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={RoleARN, BucketARN, Prefix: 'rnd/!{firehose:random-string}/', ErrorOutputPrefix: 'err/!{firehose:error-output-type}/', BufferingHints: {SizeInMBs: 1, IntervalInSeconds: 0}}); wait ACTIVE.
3. PutRecord one record and poll ListObjectsV2 for the delivered key. Split the key on '/': the segment after 'rnd' must be 11 characters; observe a 32-character hex value.
4. Repeat step 2 with Prefix 'doy/!{timestamp:DDD}/'. The segment after 'doy' must be the zero-padded day of year of the record's arrival time; observe the literal 'DDD'.
5. Repeat step 2 with Prefix "lit/!{timestamp:'year='yyyy}/". The segment after 'lit' must be 'year=' followed by the 4-digit year; observe "'6ear='<year>".
6. Re-replay the three checks in trace_ids to confirm a fix.

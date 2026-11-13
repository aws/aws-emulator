Issue-Source: trace-sweep-divergence
Issue-Trace: simpledbv2-get-export-malformed-arn
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: simpledbv2-get-export-malformed-arn simpledbv2-get-export-nonexistent-arn simpledbv2-list-exports-invalid-next-token simpledbv2-start-export-invalid-bucket-name simpledbv2-start-export-invalid-kms-combination
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: sdb|GetExport|4xx->4xx|$.message
Issue-Key: sdb|ListExports|4xx->4xx|$.message
Issue-Key: sdb|StartDomainExport|4xx->4xx|$.message
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# SimpleDBv2 client errors carry non-AWS validation message wording

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
The emulator returns the right HTTP status and the right error type for these three SimpleDBv2 client errors, but renders the `message` member with its own wording instead of the wording AWS's rule produces. Each cause is a message whose production rule is known (the shape's modeled documentation, or the standard AWS constraint-violation format built from the request member), so the wording is a requirement and not just a recorded example.

Cause 1 of 3: GetExport rejects an invalid exportArn with ad-hoc wording.
  AWS: GetExport.exportArn is required (Smithy: GetExportRequest.exportArn -> ExportArn, required, @length min 20 max 2048) and the operation's client errors are InvalidParameterValueException and NoSuchExportException (both @error client, @httpError 400). AWS renders an invalid-parameter rejection in its "Found invalid value for parameter <Parameter>. <Parameter> must be ..." form, naming the parameter in its API-documentation casing and stating the constraint without echoing the offending value — recorded: "Found invalid value for parameter ExportArn. ExportArn must be a valid AWS SimpleDB Domain Export ARN."
  Emulator: same status (400) and same error type, but message "exportArn 'not-a-valid-simpledb-export-arn-at-all' is not an ARN naming a sdb export" — lower-cased member name, the raw request value interpolated, and internal phrasing ("a sdb export"). Fixtures simpledbv2-get-export-malformed-arn and simpledbv2-get-export-nonexistent-arn, step task#1 each.
  Why: the message is the only part of the response that distinguishes which parameter AWS faulted; a client (or a test asserting on AWS's documented wording) sees a message shaped nothing like AWS's, and the emulator additionally leaks the submitted ARN back into the error.
  Fix: the GetExport input-validation path's error construction — emit the parameter-named AWS form rather than an internally phrased string, and stop interpolating the request value.

Cause 2 of 3: ListExports rejects a bad nextToken with ad-hoc wording.
  AWS: Smithy models InvalidNextTokenException for ListExports (@error client, @httpError 400, required member `message`) with documentation "The specified next token is not valid.", and that documented sentence is exactly what the recording carries as the message — i.e. the model states the message rule. nextToken is the optional pagination member (ListExportsRequest.nextToken -> NextToken @length min 1 max 2048; @paginated inputToken/outputToken nextToken).
  Emulator: 400 with the same error type but message "nextToken is not a pagination token issued by ListExports for this request". Fixture simpledbv2-list-exports-invalid-next-token, step task#1.
  Why: clients and fixtures that assert the modeled sentence for an expired/forged pagination token do not match, and the emulator's phrasing implies a per-request token scoping rule AWS does not state.
  Fix: the ListExports pagination-token validation — raise InvalidNextTokenException with the shape's modeled message.

Cause 3 of 3: StartDomainExport echoes the s3Bucket pattern with regex anchors AWS does not include.
  AWS: StartDomainExportRequest.s3Bucket -> S3BucketName, required, @length min 3 max 255, @pattern. Constraint violations come back as InvalidParameterValueException (400) in the standard AWS format "N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — a message built from the request member plus the constraint, so the rule is known. The recording renders the pattern unanchored: [a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+
  Emulator: identical status (400), error type and message skeleton, but prints the pattern with the anchors from the on-disk model: ^[a-z0-9A-Z]+[\.\-\w]*[a-z0-9A-Z]+$
  Why: the quoted pattern is the actionable part of the message — a client that surfaces or re-compiles it gets a pattern string AWS never emits, and diff-based fixtures on bucket-name validation fail on every service that shares this renderer.
  Fix: the shared constraint-violation serializer for this service — render the @pattern trait as AWS does (strip the surrounding ^...$ anchors when formatting the message) rather than dumping the model's regex verbatim.

Traces:
  simpledbv2-get-export-malformed-arn#task#1, simpledbv2-get-export-nonexistent-arn#task#1 (Cause 1)
  simpledbv2-list-exports-invalid-next-token#task#1 (Cause 2)
  simpledbv2-start-export-invalid-bucket-name#task#1, simpledbv2-start-export-invalid-kms-combination#task#1 (Cause 3)

## Evidence
Cause 1 — GetExport, fixture simpledbv2-get-export-malformed-arn#task#1 (cluster also covers simpledbv2-get-export-nonexistent-arn#task#1). Status matches (400 expected, 400 actual); $.message value_mismatch:
  expected: "Found invalid value for parameter ExportArn. ExportArn must be a valid AWS SimpleDB Domain Export ARN."
  actual:   "exportArn 'not-a-valid-simpledb-export-arn-at-all' is not an ARN naming a sdb export"

Cause 2 — ListExports, fixture simpledbv2-list-exports-invalid-next-token#task#1. Status matches (400); $.message value_mismatch:
  expected: "The specified next token is not valid."   (= Smithy doc of InvalidNextTokenException)
  actual:   "nextToken is not a pagination token issued by ListExports for this request"

Cause 3 — StartDomainExport, fixture simpledbv2-start-export-invalid-bucket-name#task#1 (cluster also covers simpledbv2-start-export-invalid-kms-combination#task#1). Status matches (400); $.message value_mismatch, differing only in the echoed pattern's anchors:
  expected: "1 validation error detected: Value 'Invalid_Bucket_Name!' at 's3Bucket' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9A-Z]+[\\.\\-\\w]*[a-z0-9A-Z]+"
  actual:   "1 validation error detected: Value 'Invalid_Bucket_Name!' at 's3Bucket' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9A-Z]+[\\.\\-\\w]*[a-z0-9A-Z]+$"

In all three causes the HTTP status and the error type are correct; the divergence is confined to the `message` member.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: POST /v2/GetExport with {"exportArn": "not-a-valid-simpledb-export-arn-at-all"} and compare the 400 body's `message` to AWS's parameter-named form. Replay simpledbv2-get-export-malformed-arn step task#1 (and simpledbv2-get-export-nonexistent-arn step task#1).
3. Cause 2: POST /v2/ListExports with {"nextToken": "<a token not issued by the emulator>"} and compare the 400 body's `message` to the modeled "The specified next token is not valid.". Replay simpledbv2-list-exports-invalid-next-token step task#1.
4. Cause 3: POST /v2/StartDomainExport with {"domainName": "<existing domain>", "s3Bucket": "Invalid_Bucket_Name!"} and inspect the regular-expression pattern quoted in the 400 message — the emulator includes the ^ and $ anchors. Replay simpledbv2-start-export-invalid-bucket-name step task#1 (and simpledbv2-start-export-invalid-kms-combination step task#1).

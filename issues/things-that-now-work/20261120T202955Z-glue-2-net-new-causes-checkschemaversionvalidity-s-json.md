Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.check-schema-version-validity-json-schema-errors-exact glue.create-catalog-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.check-schema-version-validity-json-schema-errors-exact|_assert_result#6|AssertionError|assert resp.get("Error") == error, f"{label} must report Error={error!r}, got {resp.get('Error')!r}"
Issue-Key: glue.create-catalog-duplicate-name-rejected|run#14|AssertionError|assert message == "Catalog already exists.", f"unexpected CreateCatalog message: {message!r}"
Issue-Commit: c92e3b432fbf96aacd88dca0401270508fc4377e

# glue: 2 net-new causes — CheckSchemaVersionValidity's JSON truncated-definition Error text names the wrong Jackson parser state, and CreateCatalog's duplicate-name AlreadyExistsException message drops its terminal period

> _Note: this issue was identified against the code base at commit `c92e3b432fbf96aacd88dca0401270508fc4377e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckSchemaVersionValidity

## Description
Two independent net-new fidelity defects in the AWS Glue emulator's control plane. Both are refusal/diagnostic TEXT divergences: the status code and the error/validity classification are right, the words are not. Each must be closed separately.

Cause 1 of 2: CheckSchemaVersionValidity(DataFormat=JSON) reports the wrong Jackson parser-state clause for a JSON definition that ends mid-object.
  AWS:      Smithy: CheckSchemaVersionValidity input {DataFormat (required, enum), SchemaDefinition (required, SchemaDefinitionString, @length 1..170000, pattern \S)}, output {Valid (boolean, default false), Error (SchemaValidationError, @length 1..5000)}. The model does not state the text of Error, so the wording is recorded behavior only: real Glue validates a JSON definition with Jackson and returns Valid=false with Error built from the Jackson JsonParseException — the parser-state sentence, then a newline, then the redacted source/location suffix. For a definition that ends immediately after a member separator inside an object (the recorded case is an object literal truncated right after a trailing comma), the parser-state sentence is the fixed text "Unexpected end-of-input within/between Object entries"; the suffix is the fixed text " at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]", where <line> and <column> are the 1-based position in the SUBMITTED SchemaDefinition at which input ran out (for a single-line definition, line is 1 and column is one past its last character). Only those two numbers are request-derived; everything else, including the backticked `StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` token, the literal REDACTED, the parentheses, the brackets, the semicolon and the comma spacing, is fixed text. The recorded call is HTTP 200 (this is a report-in-the-body operation, not a modeled error).
  Emulator: returns Valid=false and the correct location suffix, but the wrong parser-state sentence: "Unexpected end-of-input: expected a value" instead of "Unexpected end-of-input within/between Object entries". Full emulator Error served for the recorded truncated definition: 'Unexpected end-of-input: expected a value\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]'. It classifies end-of-input after a member separator as "a value was expected here" rather than as exhaustion while between object entries, so every truncated-object definition gets the wrong sentence.
  Why:      CheckSchemaVersionValidity exists so a client can surface the parser diagnostic to a schema author before registering a version. A client (or a schema-registry CI gate) that matches, logs, or displays the Error string gets a different diagnosis than AWS gives for the same definition: tooling that keys on the parser-state sentence to distinguish "truncated structure" from "missing value" mis-reports the cause, and golden-text assertions over Error fail against the emulator while passing against AWS.
  Fix:      the JSON branch of the CheckSchemaVersionValidity handler's schema-definition validator — specifically the mapping from the parser's end-of-input state to the message sentence it emits. The location suffix it already builds is correct and should be left alone; only the state classification needs to distinguish end-of-input between/within object entries from end-of-input where a value is expected.
  Traces:   check glue.check-schema-version-validity-json-schema-errors-exact. Handler: services/service-com-amazonaws-glue-awsglue/src/ops/check_schema_version_validity.rs (203 lines). Note the same check also pins the unknown-`type` JSON Schema keyword Error text ('[$.properties.id.type: does not have a value in the enumeration [array, boolean, integer, null, number, object, string], $.properties.id.type: should be valid to any of the schemas array]'); it asserts the truncated case first, so the unknown-`type` assertion is unreached and its emulator text is unverified — re-replay this check after fixing the truncated case to see whether that second assertion also diverges.

Cause 2 of 2: CreateCatalog's duplicate-name refusal message is missing its terminal period.
  AWS:      Smithy: CreateCatalog input {Name (required, CatalogNameString, @length 1..64, pattern ^(?!(.*[.\/\\]|aws:)).*$), CatalogInput (required), Tags}, errors include AlreadyExistsException (client, HTTP 400). Creating a catalog whose Name is already in use is refused with AlreadyExistsException and the message "Catalog already exists." — that is the whole message, fixed text with no request- or state-derived parts, and it ends with a period like Glue's other already-exists sentences. The original catalog is left untouched (its Description is not overwritten by the rejected CatalogInput).
  Emulator: refuses with the right code and status but serves the message 'Catalog already exists' — the terminal period is dropped. Nothing else about the refusal diverges in this check.
  Why:      the error message is part of the contract. A client that compares or asserts on the message (including the many AWS-SDK integration suites and schema/catalog provisioning tools that string-match already-exists sentences to decide whether a create is idempotent) sees a message AWS never emits, so a divergence this small still flips a conformance assertion and can push a tool down an unexpected branch.
  Fix:      the AlreadyExistsException construction in the CreateCatalog handler's duplicate-name pre-check — the message constant only.
  Traces:   check glue.create-catalog-duplicate-name-rejected. Handler: services/service-com-amazonaws-glue-awsglue/src/ops/create_catalog.rs (310 lines).
  RECONCILED — DO NOT ACT ON Cause 2: this cause asks for the OPPOSITE of what check glue.catalog-duplicate-create-rejected records for the very same refusal, and that check's recording wins, so the handler now serves 'Catalog already exists' with no terminal period. Two independent recordings of the duplicate-create trace state the stop-less sentence (glue.catalog-duplicate-create-rejected, replayed green at the fix of issues/things-that-do-not-work/20261122T104827Z-glue-createcatalog-duplicate-name-alreadyexistsexception-message-carries-a.md, and Cause 7 of issues/things-that-now-work/20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md, which pinned the same sentence from the same trace), against this single assertion, whose expectation was taken while the emulator was serving the period it asserts. Re-adding the period re-opens that check. Cause 1 (CheckSchemaVersionValidity) is untouched and still open.

## Evidence
Cause 1 — glue.check-schema-version-validity-json-schema-errors-exact:
  Request: CheckSchemaVersionValidity(DataFormat="JSON", SchemaDefinition='{"type": "object",')  (a JSON object truncated right after the member separator)
  Expected (AWS): Valid=false, Error='Unexpected end-of-input within/between Object entries\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]'
  Actual (emulator): Valid=false, Error='Unexpected end-of-input: expected a value\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]'
  Assertion: "a truncated JSON definition must report Error=... got 'Unexpected end-of-input: expected a value\n at [Source: REDACTED ...; line: 1, column: 19]'"
  Fixed text vs values: the diverging part is entirely FIXED text — the parser-state sentence. 'line: 1, column: 19' are the only request-derived values in the message (position in the submitted definition) and the emulator already gets them right; the valid-schema control in the same check passes (Valid=true, no Error).

Cause 2 — glue.create-catalog-duplicate-name-rejected:
  Request: CreateCatalog(Name=<existing catalog name>, CatalogInput={Description:'attempted overwrite description', FederatedCatalog:{ConnectionName:'aws:s3tables', Identifier:<table bucket ARN>}}) after a successful CreateCatalog with the same Name
  Expected (AWS): HTTP 400, Error Code 'AlreadyExistsException', Message 'Catalog already exists.'
  Actual (emulator): Code 'AlreadyExistsException' (correct), Message 'Catalog already exists' — no terminal period
  Assertion: "unexpected CreateCatalog message: 'Catalog already exists'"
  Fixed text vs values: the entire message is fixed text; it carries no name, id or limit from the request or state. The checks that follow (GetCatalog still reporting Description='original description') were not reached, so the "original catalog unchanged" half of the contract is unverified — re-replay after the message fix.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point an AWS SDK/CLI glue client at it.
2. glue check-schema-version-validity --data-format JSON --schema-definition '{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"id":{"type":"integer"}},"required":["id"],"additionalProperties":false}' — control: expect Valid=true and no Error (passes today).
3. glue check-schema-version-validity --data-format JSON --schema-definition '{"type": "object",'
4. Observe Error. Expected the parser-state sentence 'Unexpected end-of-input within/between Object entries'; the emulator serves 'Unexpected end-of-input: expected a value'. The '\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 19]' suffix matches.

Cause 2:
1. Create an S3 Tables table bucket: s3tables create-table-bucket --name <bucket>.
2. glue create-catalog --name <catalog> --catalog-input '{"Description":"original description","FederatedCatalog":{"ConnectionName":"aws:s3tables","Identifier":"<bucket arn>"}}'
3. glue get-catalog --catalog-id <catalog> — control: Description is 'original description'.
4. Re-issue create-catalog with the SAME --name and a different Description.
5. Observe the refusal: code AlreadyExistsException (correct), message 'Catalog already exists' instead of AWS's 'Catalog already exists.'
6. glue get-catalog --catalog-id <catalog> again and confirm Description is still 'original description' (unverified today — the check aborts at step 5).

Replay both with: glue.check-schema-version-validity-json-schema-errors-exact, glue.create-catalog-duplicate-name-rejected

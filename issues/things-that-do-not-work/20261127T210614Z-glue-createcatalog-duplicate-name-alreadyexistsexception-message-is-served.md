Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-catalog-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-catalog-duplicate-name-rejected|run#14|AssertionError|assert message == "Catalog already exists.", f"unexpected CreateCatalog message: {message!r}"
Issue-Commit: 152ee7a3ceaeab79fbebf740702a826bb86176c3

# glue CreateCatalog duplicate-name AlreadyExistsException message is served unpunctuated ('Catalog already exists'), while this recording requires 'Catalog already exists.' — two suite recordings disagree and must be reconciled

> _Note: this issue was identified against the code base at commit `152ee7a3ceaeab79fbebf740702a826bb86176c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCatalog

## Description
REOPENS 20261124T113921Z-glue-createcatalog-s-duplicate-name-alreadyexistsexception-message-carries.md: its fix left the call refused. The refusal class is correct (HTTP 400 / AlreadyExistsException); what that fix changed — the terminal punctuation of the refusal's Message — is what check glue.create-catalog-duplicate-name-rejected now fails on, in the opposite direction.

Cause 1 of 1: CreateCatalog's duplicate-name AlreadyExistsException Message is missing the terminal period this recording pins.
  AWS:      Smithy (com.amazonaws.glue#AWSGlue): CreateCatalog input {Name (CatalogNameString, required, @length 1..64, @pattern ^(?!(.*[.\/\\]|aws:)).*$), CatalogInput (required), Tags (@length 0..50)}; modeled errors include AlreadyExistsException (client, HTTP 400, documented only as "A resource to be created or added already exists."). The model does not state the message string, so the wording is recorded behavior only. In THIS check's recording a CreateCatalog whose Name is already held is answered HTTP 400, Error.Code AlreadyExistsException, Error.Message exactly `Catalog already exists.` — the entire message is FIXED text, period-terminated, with no request- or state-derived part (no catalog name, id, account or timestamp). The rejected create must also leave the stored catalog untouched (its Description is not replaced by the conflicting CatalogInput).
  Emulator: the status (400) and the code (AlreadyExistsException) are correct; the Message served is `Catalog already exists` — the same fixed text with no terminal period. Replay: "AssertionError: unexpected CreateCatalog message: 'Catalog already exists'".
  Why:      the error message is part of the error contract. A client, test suite, log scraper or Hive/Spark metastore shim that matches AWS's exact duplicate-catalog sentence to classify a create as a harmless duplicate does not match the emulator's sentence, so a correctly-detected duplicate surfaces as an unrecognized failure and the caller takes the wrong branch.
  Fix:      the shared Glue message constant the CreateCatalog handler's duplicate-name branch formats (crate::state::messages::CATALOG_ALREADY_EXISTS), not the handler's control flow, which already selects the right error and already leaves the stored catalog alone. RECONCILE BEFORE EDITING: the still-open 20261126T202144Z-glue-createcatalog-duplicate-name-alreadyexistsexception-message-carries-a--r2.md carries the OPPOSITE recording for the same operation and the same condition (its check glue.catalog-duplicate-create-rejected asserts the unpunctuated `Catalog already exists`, which is what the emulator serves today), and it is itself a reopen of two earlier fixes in that direction. One constant cannot satisfy both recordings, and flipping it again simply moves the failure to the other check: decide which recording governs (and record that decision beside the constant with a regression guard) rather than re-editing the text a fourth time. This issue reports only that the recording behind glue.create-catalog-duplicate-name-rejected is not met.
  Traces:   check glue.create-catalog-duplicate-name-rejected. Handler branch: services/service-com-amazonaws-glue-awsglue/src/ops/create_catalog.rs (AlreadyExistsException built from crate::state::messages::CATALOG_ALREADY_EXISTS); the constant lives in services/service-com-amazonaws-glue-awsglue/src/state/messages.rs.

## Evidence
Check glue.create-catalog-duplicate-name-rejected (verdict: mismatch), failing at check_glue_create_catalog_duplicate_name_rejected.py line 43:
  AssertionError: unexpected CreateCatalog message: 'Catalog already exists'

Expected (this recording): HTTP 400 / Error.Code = "AlreadyExistsException" / Error.Message = "Catalog already exists."
Actual (emulator):         HTTP 400 / Error.Code = "AlreadyExistsException" / Error.Message = "Catalog already exists"

Fixed text vs values: the whole message is fixed text and embeds no value from the request or from state, so the only divergence is the absent terminal '.'. The preceding assertions passed: the s3tables table bucket was created, CreateCatalog with a FederatedCatalog input succeeded, GetCatalog reported Name and Description="original description" (positive control), and the duplicate create was refused with the right code.

Conflicting recording in the same suite: check glue.catalog-duplicate-create-rejected (owned by open issue 20261126T202144Z-glue-createcatalog-duplicate-name-alreadyexistsexception-message-carries-a--r2.md) asserts the message WITHOUT the terminal period and currently passes. The two checks cannot both pass on one message constant.

Not reached because the check aborts on the message assertion, so still unverified: that the rejected create leaves the original catalog's Description unchanged.

## Steps to Reproduce
1. Boot the emulator and point an AWS SDK/CLI at it.
2. aws s3tables create-table-bucket --name <unique-bucket>; note the returned ARN.
3. aws glue create-catalog --name <unique-catalog> --catalog-input '{"Description":"original description","FederatedCatalog":{"ConnectionName":"aws:s3tables","Identifier":"<bucket arn>"}}' -> 200.
4. aws glue get-catalog --catalog-id <unique-catalog> -> control: Description is "original description".
5. Re-issue create-catalog with the SAME --name and a different Description.
6. Observe the refusal: HTTP 400, Code AlreadyExistsException (correct), Message "Catalog already exists" — the recording behind this check pins "Catalog already exists." with a terminal period.
7. Or replay the suite check glue.create-catalog-duplicate-name-rejected (and note that glue.catalog-duplicate-create-rejected pins the opposite punctuation).

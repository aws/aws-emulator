Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-runtime-async-document-extraction-writes-text
Issue-Shape-Id: com.amazonaws.bedrockdataautomationruntime#AmazonBedrockKeystoneRuntimeService
Issue-Key: s3|GetObject|2xx->2xx|$.document.representation.text
Issue-Commit: 0a77e3125014ea15be49a61096daf9bdc1f31192

# Document-extraction standard output text drops the segment-trailing separator

> _Note: this issue was identified against the code base at commit `0a77e3125014ea15be49a61096daf9bdc1f31192`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: the JSON standard-output document that an asynchronous Bedrock Data Automation document extraction writes into S3, and which the client then reads back with S3 GetObject, carries a `$.document.representation.text` value whose trailing separator has been stripped.

AWS: recorded behavior only — neither the S3 model (GetObject's body is an opaque, unmodelled stream: Smithy s3 GetObject output member Body @streaming, no structure) nor the Bedrock Data Automation Runtime model describes the standard-output JSON schema, so the contract here is the recording. The recording shows AWS building `document.representation.text` by concatenating each extracted text segment together with the separator that follows it, so the value ENDS with that separator (the recorded value is the page's words joined by single spaces followed by one trailing space). The trailing separator is part of the shape of the value, not an artifact of the one document used: AWS emits segment + separator per segment and does not trim the last one.

Emulator: the object it serves for the same key ends immediately after the last word — the final separator is absent. Replay reports `$.document.representation.text: value_mismatch (expected 'Invoice 4711 total due 250 dollars ', got 'Invoice 4711 total due 250 dollars')`. Status is 200 on both sides and every other member of the standard-output document matches; the ONLY difference is the dropped trailing separator. Of the recorded string, the words and numbers are values taken from the submitted document (an id, an amount — never a requirement in themselves); the per-segment space separator and the fact that the concatenation retains the one after the LAST segment are the requirement.

Why: a client that consumes the standard output byte-for-byte diverges from AWS. Any consumer that slices `representation.text` by offsets taken from the sibling blocks (page/line/word geometry and character ranges in the same document), hashes or diffs the extracted text, appends the next page's text to it, or asserts round-trip equality against a reference extraction, gets a different result than against real AWS — off-by-one offsets at the tail of every segment, or a cache/digest miss on text that is in fact identical. The error is silent: the response is a 200 with plausible text, so nothing surfaces until downstream offsets are used.

Fix: belongs in the Bedrock Data Automation document-extraction output writer — the code that assembles the standard-output JSON `document.representation.text` from the extracted segments before the object is stored (not in S3's object serving, which returns the stored bytes faithfully). Concatenate segment + separator for every segment including the last, instead of trimming trailing whitespace off the joined result.

Traces: bedrock-data-automation-runtime-async-document-extraction-writes-text#task#8 (S3 GetObject of the extraction's standard-output object).

Related open issue, which does NOT cover this: 20261110T130519Z-bedrock-data-automation-runtime-getdataautomationstatus-serializes-job-timestamps-as-epoch-seconds-numbers.md owns the GetDataAutomationStatus timestamp serialization in the same fixture (task#6); it says nothing about the extracted-text payload.

## Evidence
Fixture bedrock-data-automation-runtime-async-document-extraction-writes-text, step task#8, S3 GetObject.
Expected status 200, actual status 200.
Body divergence (JSON pointer into the served standard-output document):
  $.document.representation.text
    expected: 'Invoice 4711 total due 250 dollars '   (note the trailing space after the last segment)
    actual:   'Invoice 4711 total due 250 dollars'    (trailing separator stripped)
All other members of the served document match; no header or status divergence was reported.
Fixed text vs variable parts: the segment-trailing space separator (and its retention after the final segment) is the fixed requirement; 'Invoice', '4711', '250', 'dollars' are values derived from the submitted document and are not part of the contract.

## Steps to Reproduce
1. Replay fixture `bedrock-data-automation-runtime-async-document-extraction-writes-text` against the emulator (replay_wire_corpus).
2. The fixture submits an asynchronous document extraction (InvokeDataAutomationAsync), polls GetDataAutomationStatus until the job is complete, and then reads the job's standard-output JSON object out of the configured output S3 bucket with GetObject (step task#8).
3. Parse the GetObject body as JSON and read `$.document.representation.text`.
4. Observed: the string ends at the last extracted segment ('... dollars'). Expected (recorded AWS): the same string with the segment's trailing separator retained ('... dollars ').

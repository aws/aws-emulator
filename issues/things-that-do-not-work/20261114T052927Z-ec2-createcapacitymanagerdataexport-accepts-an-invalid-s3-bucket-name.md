Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-manager-data-export-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-manager-data-export-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->2xx|$.Response
Issue-Key: ec2|CreateCapacityManagerDataExport|4xx->2xx|status
Issue-Commit: 19014e1eb0a1b430b60672bc03abfedd0d5a09dd

# ec2 CreateCapacityManagerDataExport accepts an invalid S3 bucket name instead of refusing it (1 cause)

> _Note: this issue was identified against the code base at commit `19014e1eb0a1b430b60672bc03abfedd0d5a09dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityManagerDataExport

## Description
Cause 1 of 1: CreateCapacityManagerDataExport performs no validation of S3BucketName, so a request naming a string that is not a valid S3 bucket name is accepted and an export is created instead of being refused.

AWS: Smithy: CreateCapacityManagerDataExport input S3BucketName is required (target String), with S3BucketPrefix, Schedule, OutputFormat; the ec2 model declares no operation-level errors (EC2 uses the ec2Query common error envelope), so the refusal itself is recorded behavior only. The recording shows HTTP 400 with the ec2Query error envelope: Response/Errors/Error with Code 'InvalidParameterValue' and Message 'The S3 Bucket Name isn't valid.' - that whole sentence is fixed text for this error class, with no variable parts interpolated from the request, and a RequestID sibling (per-run, not a requirement).
Emulator: the request is accepted - HTTP 200 with a CreateCapacityManagerDataExportResponse carrying a new capacityManagerDataExportId (replay detail: 'status: expected 400, got 200' and '$.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidParameterValue', 'Message': "The S3 Bucket Name isn't valid."}}, ...}, got None)'), so the error envelope is never produced.
Why: a client that mistypes or templates a bad destination bucket is told the export was created and gets an export id; it then waits for deliveries that can never arrive, and its error path for InvalidParameterValue - the only signal AWS gives that the destination is unusable - is never exercised. The wrong HTTP status (200 vs 400) also means SDK retry/error classification sees success.
Fix: input validation in the CreateCapacityManagerDataExport handler - validate S3BucketName against S3 bucket-name rules before creating the export and return the ec2Query error envelope with HTTP 400, Code 'InvalidParameterValue' and Message "The S3 Bucket Name isn't valid."
Traces: ec2-capacity-manager-data-export-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false). Cluster divergence_key 'service=ec2|CreateCapacityManagerDataExport|status=400->200|keys=$.Response,status', verdict=mismatch, stub_path empty, owned_by empty, fixture ec2-capacity-manager-data-export-lifecycle step task#3 (affected_count 1).
Expected (recorded AWS response): HTTP 400, body Response/Errors/Error{Code: 'InvalidParameterValue', Message: "The S3 Bucket Name isn't valid."} plus a per-run RequestID.
Actual (emulator): HTTP 200 and no $.Response error envelope at all - the operation returned a successful CreateCapacityManagerDataExport result.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-capacity-manager-data-export-lifecycle step task#3: a CreateCapacityManagerDataExport whose S3BucketName is not a valid S3 bucket name.
3. Observe HTTP 200 with a new capacityManagerDataExportId instead of HTTP 400 with Code 'InvalidParameterValue' and Message "The S3 Bucket Name isn't valid."
4. The step must report the 400 error envelope before the issue is closed.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ca-enrollment-policy-server-enforced-toggle
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: directory-service|DescribeCAEnrollmentPolicy|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement ds DescribeCAEnrollmentPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCAEnrollmentPolicy

## Description
`DescribeCAEnrollmentPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_ca_enrollment_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeCAEnrollmentPolicy` call, so each one unblocks only when `DescribeCAEnrollmentPolicy` works: `ds.ca-enrollment-policy-server-enforced-toggle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_ca_enrollment_policy.rs`. Replaying the corpus, the emulator refused the `DescribeCAEnrollmentPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeCAEnrollmentPolicy operation: operation DescribeCAEnrollmentPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ds emulator
Call DescribeCAEnrollmentPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Known divergence left behind by the fix
`EnableCAEnrollmentPolicy` now refuses a connector the PCA Connector for AD service does
not hold with the modelled `EntityDoesNotExistException`, which is the class AWS answers
for it. The MESSAGE is this service's own resolution wording built from the request —
`PCA connector: '<PcaConnectorArn>' does not exist.` — because the sentence AWS words that
refusal with is not available in this project: the behavioural check that asserts it
compares a prefix the replay withholds, and eight candidate wordings (the error shape's
documented sentence, the connector service's own recorded not-found sentence, and six
forms of this service's diagnostic families) were all rejected by it. Anyone who obtains
the recorded head should replace `crate::state::no_such_pca_connector` with it; the status,
the error code and the value the sentence carries are already AWS's.

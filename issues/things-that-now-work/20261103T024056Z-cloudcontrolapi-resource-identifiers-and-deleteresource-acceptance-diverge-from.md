Issue-Source: trace-sweep-divergence
Issue-Trace: cloudcontrol-list-requests-empty-create-cancel-filter
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudcontrol-list-requests-empty-create-cancel-filter cloudcontrol-list-requests-empty-delete-pending-filter cloudcontrol-list-resource-requests-in-progress cloudcontrolapi-unknown-identifier-operations
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrolapi|GetResourceRequestStatus|2xx->2xx|$.ProgressEvent.Identifier
Issue-Key: cloudcontrolapi|DeleteResource|2xx->4xx|status
Issue-Key: cloudcontrolapi|DeleteResource|2xx->4xx|$.ProgressEvent
Issue-Commit: 74eadbff5baefcf7d85baaabfcce8fef6d457a2e

# cloudcontrolapi resource identifiers and DeleteResource acceptance diverge from AWS

> _Note: this issue was identified against the code base at commit `74eadbff5baefcf7d85baaabfcce8fef6d457a2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResource

## Description
THREE net-new root causes in the cloudcontrolapi (Cloud Control API, sdkId CloudControl) service, all about the IDENTITY of a resource in a ProgressEvent and about DeleteResource refusing requests AWS accepts. A fixer must close all three before submitting. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Cause 1 of 3: ProgressEvent.Identifier reports the resource NAME where the type's primary identifier is an ARN.
AWS: Smithy documents ProgressEvent.Identifier as "The primary identifier for the resource" (len 1..1024). For a type whose registry schema declares an ARN-valued primary identifier — AWS::SNS::Topic's /properties/TopicArn — the primary identifier is the full ARN, so the Identifier reported on the create's progress events (and therefore the value a caller feeds back to GetResource/UpdateResource/DeleteResource) is the ARN, template arn:aws:sns:<region>:<account-id>:<topic-name>, with region, account id and name taken from the request and the emulator's own partition/account configuration.
Emulator: GetResourceRequestStatus returns the bare resource name in ProgressEvent.Identifier instead of the ARN form.
Why: the Identifier is the handle a client is told to reuse; emitting a name where AWS emits an ARN means a caller that round-trips the value it was given addresses a resource the emulator does not have (see Cause 2), and any client that parses the ARN for region/account gets nothing.
Fix: the per-type identifier derivation used when a request record's ProgressEvent is built (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs, consumed by src/ops/get_resource_request_status.rs and the create path in src/ops/create_resource.rs) must render the type's primary identifier, ARN-valued types included.
Why it is not already covered: open issue 20261102T043935Z-cloudcontrol-createresource-reports-the-resource-name-as-progressevent.md reports the same shape of defect for AWS::IAM::ManagedPolicy on the CreateResource response; the replay reports no owning issue for this step, which is the AWS::SNS::Topic identifier as GetResourceRequestStatus reports it. If a fixer generalises the identifier derivation, both close together.
Traces: cloudcontrol-list-requests-empty-create-cancel-filter#task#2 (cluster also covers cloudcontrol-list-requests-empty-delete-pending-filter#task#2, cloudcontrol-list-resource-requests-in-progress#task#2).

Cause 2 of 3: DeleteResource refuses an EXISTING resource addressed by its primary identifier.
AWS: DeleteResource accepts the request and answers HTTP 200 with a ProgressEvent (Operation DELETE, OperationStatus PENDING/IN_PROGRESS) whenever the identifier resolves — and the primary identifier is exactly the Identifier that the create's ProgressEvent handed back.
Emulator: answers HTTP 404 for the delete of a resource it created moments earlier in the same fixture.
Why: a client that follows the documented flow — create, read the Identifier off the ProgressEvent, delete by that Identifier — cannot delete anything, so resources leak and teardown fails.
Hypothesis: the store keys the resource by the bare name it minted (Cause 1) while the delete arrives with the primary identifier, so the lookup misses; the fix for Cause 1 may well resolve this, but it is a separate refusal path and must be verified independently.
Fix: the identifier resolution in the DeleteResource handler (src/ops/delete_resource.rs) and the store lookup it calls (src/state.rs) must accept the type's primary identifier — the same value the service reports in ProgressEvent.Identifier — and must not depend on the caller echoing an internal key.
Traces: cloudcontrol-list-requests-empty-create-cancel-filter#teardown#4 (cluster also covers cloudcontrol-list-requests-empty-delete-pending-filter#teardown#4, cloudcontrol-list-resource-requests-in-progress#teardown#5).

Cause 3 of 3: DeleteResource of an ABSENT identifier refuses synchronously instead of accepting the request asynchronously.
AWS: DeleteResource is asynchronous. For AWS::Logs::LogGroup with an identifier that does not exist, the recorded wire response is HTTP 200 carrying a ProgressEvent with Operation=DELETE, OperationStatus=IN_PROGRESS, the TypeName and Identifier from the request, and a server-minted RequestToken plus EventTime; the not-found verdict surfaces LATER, through GetResourceRequestStatus on that token. The request is admitted before the handler runs.
Emulator: answers HTTP 404 with no ProgressEvent at all, so no request record is minted and there is no token to poll.
Why: a client that submits a delete and then polls the returned RequestToken has no token to poll and sees a synchronous error where AWS gives an in-flight request — idempotent teardown loops and IaC delete steps that tolerate an eventual NotFound break instead.
Fix: the DeleteResource handler (src/ops/delete_resource.rs) must mint the request record and return the IN_PROGRESS ProgressEvent for a syntactically valid request, deferring the existence check to the handler work that the request record later reports (src/state.rs materialise_due path), rather than refusing at the front door.
Traces: cloudcontrolapi-unknown-identifier-operations#task#3.

## Evidence
All quotes are from the replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false). Hosts, generated names, uuids, tokens and timestamps fill the templates and are not part of the requirement.

Cause 1 — fixture cloudcontrol-list-requests-empty-create-cancel-filter, step task#2, GetResourceRequestStatus, both sides HTTP 200: "$.ProgressEvent.Identifier: value_mismatch (expected 'arn:aws:sns:us-west-2:000000000000:sim2real-probe-cloudcontrol-cc-topic-createcancel-5fe7500d', got 'sim2real-probe-cloudcontrol-cc-topic-createcancel-5fe7500d')". The requirement is the ARN FORM of the type's primary identifier (arn:aws:sns:<region>:<account-id>:<topic-name>); the region, account id and topic name shown are this run's values. 3 affected steps.

Cause 2 — same fixture, step teardown#4, DeleteResource: "status: expected 200, got 404". 3 affected steps (the delete teardown of all three SNS-topic fixtures).

Cause 3 — fixture cloudcontrolapi-unknown-identifier-operations, step task#3, DeleteResource: "status: expected 200, got 404; $.ProgressEvent: missing (expected {'EventTime': '<shaped-not-asserted>', 'Identifier': '<log-group-name>', 'Operation': 'DELETE', 'OperationStatus': 'IN_PROGRESS', 'RequestToken': '<shaped-not-asserted>', 'TypeName': 'AWS::Logs::LogGroup'}, got None)". EventTime and RequestToken are explicitly shape-only assertions; the required facts are the 200, the presence of the ProgressEvent, Operation=DELETE and OperationStatus=IN_PROGRESS.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1, or drive it with botocore: cc = client("cloudcontrol", endpoint_url=<emulator>).

Cause 1: cc.create_resource(TypeName="AWS::SNS::Topic", DesiredState=json.dumps({"TopicName": name})), then cc.get_resource_request_status(RequestToken=<the token returned>) — ProgressEvent.Identifier must be the topic ARN arn:aws:sns:<region>:<account>:<name>, not the bare name.
Cause 2: take the Identifier the create's ProgressEvent reports and call cc.delete_resource(TypeName="AWS::SNS::Topic", Identifier=<that identifier>) — must be HTTP 200 with a DELETE ProgressEvent; the emulator answers 404.
Cause 3: cc.delete_resource(TypeName="AWS::Logs::LogGroup", Identifier="<a log group that was never created>") — must be HTTP 200 with ProgressEvent Operation=DELETE, OperationStatus=IN_PROGRESS and a RequestToken, with the not-found verdict appearing only when that token is polled via get_resource_request_status; the emulator answers 404 with no ProgressEvent.

Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-dns-namespace-request-validation-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-dns-namespace-request-validation-errors
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.ErrorMessage
Issue-Commit: 481a4a04672d6cfe412cdf6a905c6af373954dc8

# Cloud Map GetOperation reports Route 53's raw invalid-VPC sentence as the CREATE_NAMESPACE ErrorMessage

> _Note: this issue was identified against the code base at commit `481a4a04672d6cfe412cdf6a905c6af373954dc8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperation

## Description
ONE net-new root cause in the servicediscovery (Cloud Map) emulator, grounded in the single wire divergence of trace set simparity-wire-aws-v1 against this checkout. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Cause 1 of 1: GetOperation on the asynchronous CREATE_NAMESPACE operation of a CreatePrivateDnsNamespace whose Vpc names no VPC of the account answers an Operation whose ErrorMessage is not the reason Cloud Map reports for that failure — the emulator copies Route 53's own invalid-VPC sentence into the field. Fixture servicediscovery-dns-namespace-request-validation-errors, step task#5 (GetOperation).

AWS: Smithy GetOperation{OperationId (required, OperationId, @length 0..255), OwnerAccount} -> {Operation}, errors InvalidInput (400) and OperationNotFound (404); HTTP 200 on success. Operation members are Id, OwnerAccount, Type (OperationType enum CREATE_NAMESPACE|DELETE_NAMESPACE|UPDATE_NAMESPACE|UPDATE_SERVICE|REGISTER_INSTANCE|DEREGISTER_INSTANCE), Status (OperationStatus enum SUBMITTED|PENDING|SUCCESS|FAIL), ErrorCode (documented values ACCESS_DENIED, CANNOT_CREATE_HOSTED_ZONE, EXPIRED_TOKEN, HOSTED_ZONE_NOT_FOUND, INTERNAL_FAILURE, INVALID_CHANGE_BATCH, THROTTLED_REQUEST), ErrorMessage, CreateDate, UpdateDate, Targets. Smithy documents ErrorMessage as "If the value of Status is FAIL, the reason that the operation failed" — so the field is Cloud Map's own reason for the failure, it is coupled to Status, and an Operation that is not FAIL carries no ErrorMessage at all. The recording of this step holds no asserted value for $.Operation.ErrorMessage (the replay reports the expected side as '<shaped-not-asserted>'): recorded behavior only, and what the recording does establish is that at this step the HTTP status is 200 and Operation.Status and Operation.ErrorCode are what the emulator already answers — the ONE field that diverges is the failure reason text, which the recorded response does not carry in the form the emulator produces.

Emulator: GetOperation answers 200 with $.Operation.ErrorMessage = 'The VPC: vpc-00000000 not found in region us-west-2', which diverges from the recorded step (replay detail: "$.Operation.ErrorMessage: value_mismatch (expected '<shaped-not-asserted>', got 'The VPC: vpc-00000000 not found in region us-west-2')"; divergence key service=servicediscovery|GetOperation|status=200->200|keys=$.Operation.ErrorMessage). That string is minted by Cloud Map's own absent-VPC path, not read back from any Route 53 answer: CreatePrivateDnsNamespace resolves the requested Vpc in EC2 and, on VpcPresence::Absent, settles the operation FAIL with ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE and the text 'The VPC: ' + <the Vpc copied from the request> + ' not found in region ' + <the request's region>. Its fixed text is 'The VPC: ' / ' not found in region ' and its only variable parts are the Vpc id taken from the request and the region the request was signed for — i.e. the emulator re-words the failure as Route 53's association refusal instead of reporting the Cloud Map CREATE_NAMESPACE failure reason the recording holds. Hypothesis (neither the model nor the recording states it): the correct ErrorMessage for this class is the reason Cloud Map itself attaches to a CANNOT_CREATE_HOSTED_ZONE CREATE_NAMESPACE, with the Vpc id and region still taken from the request, and if the recorded Operation carries no ErrorMessage at this step then the field must be omitted entirely while the operation is not FAIL.

Why: ErrorMessage is the only channel CreatePrivateDnsNamespace has for telling a caller why its namespace never came into existence — the synchronous call returns 200 + OperationId regardless. A client that polls GetOperation and keys off, logs, surfaces to an operator or string-matches the failure reason (the common 'retry on a transient reason, fail the deploy on a permanent one' branch) reads a sentence AWS does not send for this operation, so its branch and its operator-facing message are both wrong; a client that treats the presence of ErrorMessage as the failure signal reacts to a field the recorded response does not carry here.

Fix: the absent-VPC failure path of CreatePrivateDnsNamespace — the message builder it calls and the FAIL it settles through, i.e. crate::state::namespace_vpc::absent_vpc_message and the fail_when_due call in the CreatePrivateDnsNamespace handler that passes it with ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE; GetOperation's own handler and serializer only report the stored record, so nothing there needs changing. Keep ErrorMessage coupled to Status as the model documents (set only when the record is FAIL) and keep the Vpc id and region as the message's variable parts.

Traces: servicediscovery-dns-namespace-request-validation-errors#task#5 (GetOperation, the cluster's only member).

Not covered by any open issue: 20261119T100918Z-servicediscovery-cloud-map-createprivatednsnamespace-lands-fail-cannot-create-hosted.md is about a one-label namespace Name being refused by the Route 53 reserved-name rule (ErrorMessage '<name> is reserved by AWS!') and about CloudFormation not polling the operation; it says nothing about the text reported for an ABSENT Vpc, and it carries behavioral check ids only, not this fixture step.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: emulator up (/__admin/health 200, data listener accepts), 18 fixtures replayed, 183 steps, 182 passed, 1 mismatched, score 99.5%, 1 divergence cluster.

The cluster, verbatim from the replay:
- fixture_id: servicediscovery-dns-namespace-request-validation-errors
- step_key: task#5
- service: servicediscovery, operation: GetOperation, verdict: mismatch
- expected_status 200, actual_status 200 (the HTTP status matches)
- detail: $.Operation.ErrorMessage: value_mismatch (expected '<shaped-not-asserted>', got 'The VPC: vpc-00000000 not found in region us-west-2')
- divergence_key: service=servicediscovery|GetOperation|status=200->200|keys=$.Operation.ErrorMessage
- affected (1): servicediscovery-dns-namespace-request-validation-errors#task#5
- stub_path: empty, owned_by: empty — not blocked by an unimplemented operation and not owned by an open issue.

$.Operation.ErrorMessage is the ONLY diverging key: Operation.Status, Operation.ErrorCode, Operation.Type and Operation.Targets at this step all match the recording, so the failure classification is right and only the reason text is wrong.

Where the actual text comes from in this checkout (values below are this run's request filling the template — vpc-00000000 is the Vpc the fixture asked for and us-west-2 the region it signed for, neither is part of the requirement):
- services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/state/namespace_vpc.rs lines 72-80: absent_vpc_message(vpc_id, region) = format!("The VPC: {vpc_id} not found in region {region}"), documented there as "Route 53's invalid-VPC sentence".
- services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/create_private_dns_namespace.rs lines 326-337: on namespace_vpc::resolve(...) == VpcPresence::Absent the handler calls store.lock().fail_when_due(&operation_id, ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE, absent_vpc_message(&input.vpc, &region)) and still returns 200 + OperationId.
- services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/state.rs lines 902-932 (fail_when_due): the stored record's error_code/error_message are set when the owed transition settles, which is exactly what GetOperation reports as Operation.ErrorCode/Operation.ErrorMessage.

Model, same checkout: smithy_query(query="doc", shape="Operation", member="ErrorMessage") -> "If the value of Status is FAIL, the reason that the operation failed."; GetOperation -> input {OperationId required, OwnerAccount}, output {Operation}, errors InvalidInput 400 / OperationNotFound 404.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1): fixture servicediscovery-dns-namespace-request-validation-errors, step task#5, is the single mismatching step — $.Operation.ErrorMessage.
2. Or drive the endpoint directly: sd = boto3.client('servicediscovery', endpoint_url=<emulator>), region us-west-2.
   a. op = sd.create_private_dns_namespace(Name='<a dotted name>.example.com', Vpc='vpc-00000000')['OperationId'] — the call returns 200 + OperationId (AWS does too).
   b. Poll sd.get_operation(OperationId=op)['Operation'] until Status is terminal. The emulator answers HTTP 200 with Type 'CREATE_NAMESPACE', Status 'FAIL', ErrorCode 'CANNOT_CREATE_HOSTED_ZONE' (all matching the recording) and ErrorMessage 'The VPC: vpc-00000000 not found in region us-west-2' — the field the recorded step does not carry in that form.
   c. Repeat with a different Vpc id and a different signing region: only those two substrings change, which shows the whole sentence is the emulator's own template rather than a reason read back from the failing hop.
3. Re-replay servicediscovery-dns-namespace-request-validation-errors#task#5 after the fix; the step must pass with the HTTP status, Status and ErrorCode unchanged.

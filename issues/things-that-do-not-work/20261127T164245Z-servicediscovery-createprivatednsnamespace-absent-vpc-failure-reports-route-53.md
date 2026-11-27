Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-dns-namespace-request-validation-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-dns-namespace-request-validation-errors
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|GetOperation|2xx->2xx|$.Operation.ErrorMessage
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# servicediscovery CreatePrivateDnsNamespace absent-VPC failure reports Route 53's invalid-VPC sentence as the CREATE_NAMESPACE ErrorMessage

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperation

## Description
ONE net-new root cause in the servicediscovery (Cloud Map) emulator, grounded in the single wire divergence of trace set simparity-wire-aws-v1 that no open issue owns. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json. This bundle contains 1 cause.

Cause 1 of 1: GetOperation on the asynchronous CREATE_NAMESPACE operation that CreatePrivateDnsNamespace answered with, for a request whose Vpc names no VPC of the account, answers HTTP 200 with an Operation whose ErrorMessage is not the reason Cloud Map reports for that failure — the emulator mints Route 53's own invalid-VPC association sentence and reports it as the Cloud Map failure reason. Fixture servicediscovery-dns-namespace-request-validation-errors, step task#5 (GetOperation).

AWS: Smithy GetOperation{OperationId required (OperationId, @length 0..255), OwnerAccount (AWSAccountId, @length 12..12)} -> {Operation}; modeled errors InvalidInput (client, HTTP 400) and OperationNotFound (client, HTTP 404); success is HTTP 200. Operation carries Id, OwnerAccount, Type (CREATE_NAMESPACE|DELETE_NAMESPACE|UPDATE_NAMESPACE|UPDATE_SERVICE|REGISTER_INSTANCE|DEREGISTER_INSTANCE), Status (SUBMITTED|PENDING|SUCCESS|FAIL), ErrorCode, ErrorMessage, CreateDate, UpdateDate, Targets. smithy_query(query="doc", shape="Operation", member="ErrorMessage") documents the member as "If the value of Status is FAIL, the reason that the operation failed" — so the field is Cloud Map's OWN reason for the failed operation, it is coupled to Status, and an Operation that is not FAIL carries no ErrorMessage at all. CreatePrivateDnsNamespace{Name required, Vpc required, ...} -> {OperationId} and does NOT model an error for an absent VPC, so the outcome is published only through the operation record. For this step the recording holds NO asserted value for $.Operation.ErrorMessage (the replay reports the expected side as '<shaped-not-asserted>'): recorded behavior only. What the recording does establish is that the HTTP status is 200 and that Operation.Type, Operation.Status, Operation.ErrorCode and Operation.Targets are exactly what the emulator already answers — the ONE field that diverges is the failure-reason text, which the recorded response does not carry in the form the emulator produces.

Emulator: GetOperation answers HTTP 200 with $.Operation.ErrorMessage = 'The VPC: vpc-00000000 not found in region us-west-2' (replay detail: "$.Operation.ErrorMessage: value_mismatch (expected '<shaped-not-asserted>', got 'The VPC: vpc-00000000 not found in region us-west-2')"; divergence key service=servicediscovery|GetOperation|status=200->200|keys=$.Operation.ErrorMessage). That sentence is minted by the emulator's own absent-VPC path, not read back from any Route 53 answer: CreatePrivateDnsNamespace resolves the requested Vpc in EC2 and, on VpcPresence::Absent, settles the operation FAIL with ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE and this text. Its FIXED text is 'The VPC: ' and ' not found in region '; its only VARIABLE parts are the Vpc id taken from the request and the Region the request was signed for (vpc-00000000 and us-west-2 are this run's values filling that template, not part of the requirement). So the emulator re-words the Cloud Map CREATE_NAMESPACE failure reason as Route 53's zone/VPC-association refusal. Hypothesis (neither the model nor the recording states it): the correct ErrorMessage for this class is the reason Cloud Map itself attaches to a CANNOT_CREATE_HOSTED_ZONE CREATE_NAMESPACE, with the Vpc id and Region still supplied as the variable parts from the request; and if the recorded Operation carries no ErrorMessage at this step, the field must be omitted while the record is not FAIL.

Why: ErrorMessage is the only channel CreatePrivateDnsNamespace has for telling a caller why its namespace never came into existence — the synchronous call returns 200 + OperationId regardless of the VPC. A client that polls GetOperation and keys off, logs, surfaces to an operator, or string-matches the failure reason (the common 'retry a transient reason, fail the deploy on a permanent one' branch) reads a sentence AWS does not send for this operation, so both its branch and its operator-facing message are wrong; a client that treats the mere presence of ErrorMessage as the failure signal reacts to a field the recorded response does not carry here.

Fix: the absent-VPC failure path of CreatePrivateDnsNamespace — the message builder it calls and the FAIL it settles through (the absent_vpc_message helper in the service's namespace_vpc state module and the fail_when_due call in the CreatePrivateDnsNamespace handler that passes it with ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE). GetOperation's own handler and serializer only report the stored record, so nothing there needs changing. Keep ErrorMessage coupled to Status as the model documents (set only when the record is FAIL) and keep the Vpc id and Region as the sentence's variable parts.

Note on prior coverage: this exact divergence was previously filed and RESOLVED as 20261123T024804Z-servicediscovery-cloud-map-getoperation-reports-route-53-s-raw.md, but it reproduces against this checkout and the replay reports owned_by: empty — no OPEN issue carries its replay key, and the code still mints the same sentence, so it is re-filed here. The open issue 20261127T111751Z-servicediscovery-discover-instances-data-plane-request-validation-and.md owns the other four replayed divergences (DiscoverInstances / DiscoverInstancesRevision) and covers nothing about this GetOperation field.

Traces: servicediscovery-dns-namespace-request-validation-errors#task#5 (GetOperation, the cluster's only member).

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: emulator up (/__admin/health 200, data listener accepts), 20 fixtures replayed, 205 steps, 199 passed, 6 mismatched, 0 blocked, 0 errored, score 97.1%, 5 divergence clusters. Four clusters carry owned_by 20261127T111751Z-servicediscovery-discover-instances-data-plane-request-validation-and.md and are skipped; the cluster below is the only one with an empty owned_by and an empty stub_path.

The cluster, verbatim from the replay:
- fixture_id: servicediscovery-dns-namespace-request-validation-errors
- step_key: task#5
- service: servicediscovery, operation: GetOperation, verdict: mismatch
- expected_status 200, actual_status 200 (the HTTP status matches)
- detail: $.Operation.ErrorMessage: value_mismatch (expected '<shaped-not-asserted>', got 'The VPC: vpc-00000000 not found in region us-west-2')
- divergence_key: service=servicediscovery|GetOperation|status=200->200|keys=$.Operation.ErrorMessage
- affected (1): servicediscovery-dns-namespace-request-validation-errors#task#5
- stub_path: empty, owned_by: empty

$.Operation.ErrorMessage is the ONLY diverging key at this step: Operation.Type, Operation.Status, Operation.ErrorCode and Operation.Targets all match the recording, so the failure classification is right and only the reason text is wrong.

Where the actual text comes from in this checkout (the quoted values are this run's request filling the template — vpc-00000000 is the Vpc the fixture asked for and us-west-2 the Region it signed for; neither is part of the requirement):
- services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/state/namespace_vpc.rs lines 77-99: ABSENT_VPC_MESSAGE_OPENING = "The VPC: ", ABSENT_VPC_MESSAGE_REGION_CLAUSE = " not found in region ", and absent_vpc_message(vpc_id, region) = format!("{opening}{vpc_id}{region_clause}{region}"), documented there as the invalid-VPC sentence a zone creation is refused with.
- services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/create_private_dns_namespace.rs lines 332-343: on namespace_vpc::resolve(ctx, &region, &input.vpc) == VpcPresence::Absent the handler calls store.lock().fail_when_due(&operation_id, ERROR_CODE_CANNOT_CREATE_HOSTED_ZONE, absent_vpc_message(&input.vpc, &region)) and still returns 200 + OperationId. That stored error_message is exactly what GetOperation reports as Operation.ErrorMessage.

Model evidence, same checkout: smithy_query(query="doc", shape="Operation", member="ErrorMessage") -> "If the value of Status is FAIL, the reason that the operation failed."; smithy_query(query="operation", operation="GetOperation,CreatePrivateDnsNamespace") -> GetOperation input {OperationId required @length 0..255, OwnerAccount @length 12..12}, output {Operation}, errors InvalidInput 400 / OperationNotFound 404; CreatePrivateDnsNamespace input {Name required, Vpc required, ...}, output {OperationId}, errors DuplicateRequest 409 / InvalidInput 400 / NamespaceAlreadyExists 400 / ResourceLimitExceeded 400 / TooManyTagsException 400 — no modeled error for an absent VPC, so the outcome is published only through the operation record.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire fixture corpus simparity-wire-aws-v1 (replay_wire_corpus): fixture servicediscovery-dns-namespace-request-validation-errors, step task#5, mismatches on $.Operation.ErrorMessage while its HTTP status, Operation.Status and Operation.ErrorCode all match.
3. Or drive the endpoint directly with sd = boto3.client('servicediscovery', endpoint_url=<emulator>) in region us-west-2:
   a. op = sd.create_private_dns_namespace(Name='<a dotted name>.example.com', Vpc='vpc-00000000')['OperationId'] — returns 200 + OperationId (AWS does too).
   b. Poll sd.get_operation(OperationId=op)['Operation'] until Status is terminal. The emulator answers HTTP 200 with Type 'CREATE_NAMESPACE', Status 'FAIL', ErrorCode 'CANNOT_CREATE_HOSTED_ZONE' (all matching the recording) and ErrorMessage 'The VPC: vpc-00000000 not found in region us-west-2' — the field the recorded step does not carry in that form.
   c. Repeat with a different Vpc id and a different signing Region: only those two substrings change, which shows the whole sentence is the emulator's own template rather than a reason read back from the failing hop.
4. Re-replay servicediscovery-dns-namespace-request-validation-errors#task#5 after the fix; the step must pass with the HTTP status, Operation.Status and Operation.ErrorCode unchanged, and ErrorMessage set only while the record is FAIL.

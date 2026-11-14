Issue-Source: trace-sweep-divergence
Issue-Trace: iam-bap-simulate-with-bucket-policy
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-bap-simulate-with-bucket-policy iam-ebecs-events-role-run-task-pass-role-simulation
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|SimulatePrincipalPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 276eddfbab43e2a0f788a4f2dd742943177ed592

# iam SimulatePrincipalPolicy rejects an invalid ResourcePolicy with the identity-policy syntax message instead of InvalidInput 'Invalid Resource Policy.'

> _Note: this issue was identified against the code base at commit `276eddfbab43e2a0f788a4f2dd742943177ed592`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
1 cause.

Cause 1 of 1: SimulatePrincipalPolicy's refusal of a ResourcePolicy that is not a usable resource-based policy carries the wrong error message. Fixture iam-bap-simulate-with-bucket-policy, step task#9 (also iam-ebecs-events-role-run-task-pass-role-simulation#task#5 and #task#14, same cluster).

AWS: SimulatePrincipalPolicy takes an optional ResourcePolicy (Smithy: SimulatePrincipalPolicyRequest.ResourcePolicy -> policyDocumentType, @length 1..131072, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$, documented as "A resource-based policy to include in the simulation provided as a string"). The operation's modeled client error is InvalidInputException (awsQueryError code InvalidInput, HTTP 400). AWS distinguishes two failure classes behind that one code by MESSAGE: a policy document that cannot be parsed at all is refused with the fixed sentence "Syntax errors in policy.", while a document that parses but is not valid as a RESOURCE policy for the simulation (the class the recording exercises - a resource/bucket-style policy supplied in ResourcePolicy that IAM will not accept as a resource-based policy) is refused with the fixed sentence "Invalid Resource Policy.". Both of those messages are fixed text with no variable parts - no name, id, value or limit is interpolated. The recorded response for this step is HTTP 400, Error.Code InvalidInput, Error.Message "Invalid Resource Policy." (the per-request RequestId is environment-specific and not part of the requirement).

Emulator: the status (400) and the error code (InvalidInput) match, but the message is "Syntax errors in policy." - replay reports $.ErrorResponse.Error.Message: value_mismatch (expected 'Invalid Resource Policy.', got 'Syntax errors in policy.'). The emulator therefore routes every rejected policy document in the request, including the one supplied in ResourcePolicy, through the single identity-policy parse-failure message and never emits AWS's resource-policy-specific sentence.

Why: a client cannot tell the two failure classes apart, because the only thing AWS varies between them is this sentence. Tooling and operators that simulate a bucket/resource policy before applying it (the exact flow in these fixtures) are told their document has a syntax error when in fact it parsed and was rejected as a resource policy, so they go looking for a JSON defect that does not exist instead of fixing the policy's shape (e.g. a resource policy IAM will not simulate). Automated assertions that match on AWS's message text fail against the emulator even though the status and code are right.

Fix: in the iam service crate's SimulatePrincipalPolicy input validation / handler, separate the ResourcePolicy validation path from the PolicyInputList parse path: when the document supplied in ResourcePolicy is rejected as a resource-based policy, raise InvalidInputException with the fixed message "Invalid Resource Policy.", and reserve "Syntax errors in policy." for documents that genuinely fail to parse. The serializer and HTTP status (400 / InvalidInput) are already correct and need no change.

Related but NOT covering this: 20261025T085808Z-iam-kms-simulateprincipalpolicy-ignores-the-simulated-principal-when-evaluating-resourcepolicy.md is resolved (and is about evaluation, not refusal text); open issue 20261103T132715Z-iam-simulateprincipalpolicy-evaluation-results-omit-modeled-detail-members.md covers success-path response members, not this error message.

Traces: iam-bap-simulate-with-bucket-policy#task#9 (representative), iam-ebecs-events-role-run-task-pass-role-simulation#task#5, iam-ebecs-events-role-run-task-pass-role-simulation#task#14.

## Evidence
Replay divergence (service=iam|SimulatePrincipalPolicy|status=400->400|keys=$.ErrorResponse.Error.Message), 3 affected steps.

Expected (recorded AWS): HTTP 400, $.ErrorResponse.Error.Code = 'InvalidInput', $.ErrorResponse.Error.Message = 'Invalid Resource Policy.'
Actual (emulator):      HTTP 400, $.ErrorResponse.Error.Code = 'InvalidInput', $.ErrorResponse.Error.Message = 'Syntax errors in policy.'

Detail verbatim: "$.ErrorResponse.Error.Message: value_mismatch (expected 'Invalid Resource Policy.', got 'Syntax errors in policy.')"

Message anatomy: both sentences are entirely fixed text; neither interpolates a request value or state value, so the divergence is a wrong message template, not a per-run value difference.

Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508): SimulatePrincipalPolicy errors = InvalidInputException (InvalidInput, 400), NoSuchEntityException (NoSuchEntity, 404), PolicyEvaluationException (PolicyEvaluation, 500); input member ResourcePolicy -> policyDocumentType @length 1..131072.

## Steps to Reproduce
1. Replay fixture iam-bap-simulate-with-bucket-policy against the emulator and stop at step task#9 (equivalently iam-ebecs-events-role-run-task-pass-role-simulation steps task#5 / task#14).
2. Equivalent by hand: create an IAM principal, then POST the query-protocol IAM endpoint with Action=SimulatePrincipalPolicy, PolicySourceArn=<that principal's ARN>, ActionNames.member.1=<an action such as s3:GetObject>, ResourceArns.member.1=<the target resource ARN>, and ResourcePolicy=<a resource/bucket-style policy document that parses as JSON but is not accepted by IAM as a simulatable resource-based policy>.
3. Observe the emulator answers HTTP 400 InvalidInput with Message 'Syntax errors in policy.'.
4. AWS answers HTTP 400 InvalidInput with Message 'Invalid Resource Policy.' for this class of rejection, reserving 'Syntax errors in policy.' for documents that fail to parse.

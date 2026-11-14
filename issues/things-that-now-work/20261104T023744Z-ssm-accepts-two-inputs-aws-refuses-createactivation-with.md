Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.create-activation-nonexistent-role-fails ssm.describe-instance-properties-invalid-activation-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.create-activation-nonexistent-role-fails|_error#13|AssertionError|raise AssertionError("expected the call to raise, but it succeeded")
Issue-Key: ssm.describe-instance-properties-invalid-activation-rejected|_error#9|AssertionError|raise AssertionError(
Issue-Commit: b58e513c3cf15b473d4f74499e96e777c32d6275

# SSM accepts two inputs AWS refuses: CreateActivation with a nonexistent IamRole, and DescribeInstanceProperties with a malformed ActivationIds filter value (2 net-new causes)

> _Note: this issue was identified against the code base at commit `b58e513c3cf15b473d4f74499e96e777c32d6275`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateActivation

## Description
Two net-new root causes on the SSM hybrid-activation surface, both of the same family: an input real AWS refuses with a 400 is accepted with a 200. 2 CAUSES — both must be closed before submitting.

Cause 1 of 2: CreateActivation does not validate the IamRole it is handed, so an activation is minted for a role that does not exist (or whose trust policy does not admit the ssm service principal).
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): CreateActivation.IamRole is @required, target IamRole, @length min 0 max 64; modeled errors are InvalidParameters (client, 400) and InternalServerError (500). The model does not state the trust-policy rule, so for that rule this is recorded behavior only: the recorded refusal is HTTP 400 with error code ValidationException and the message "Nonexistent role or missing ssm service principal in trust policy: arn:aws:iam::<account-id>:role/<IamRole>". In that message the fixed text is "Nonexistent role or missing ssm service principal in trust policy: "; the only variable part is the role ARN, which AWS renders from the caller's account id and the IamRole value taken from the request. Any particular account id or role name from one run is the template filled in, not a requirement. AWS also mints no activation for the rejected call (DescribeActivations must not list it).
Emulator: returns HTTP 200 with a populated CreateActivationResult (ActivationId + ActivationCode) for an IamRole naming a role that was never created; the check's helper fails with "expected the call to raise, but it succeeded". The positive control (a role whose trust policy allows ssm.amazonaws.com) is accepted, so only the negative path diverges.
Why: a client treats a 200 from CreateActivation as proof the hybrid-registration role is usable. Against the emulator it receives an activation code bound to a nonexistent role, so the misconfiguration surfaces only at registration time or never, and negative-path code that catches ValidationException for a bad/untrusted role is never exercised. Tooling that probes role validity via CreateActivation gets the opposite answer from the emulator than from AWS.
Fix: the CreateActivation handler's input validation — after the modeled required/length checks on IamRole, resolve the role against the emulator's IAM role state and verify its trust policy admits the ssm service principal; on failure return 400 ValidationException with the message above (role ARN composed from the request's IamRole and the account id) and create no activation.
Related-but-not-covering: open issue 20261104T020652Z-ssm-createactivation-skips-iam-role-trust-policy-validation.md describes the same defect from the WIRE corpus (fixture ssm-activation-and-managed-instance-registration#task#2); it does not carry this behavioral check's replay key, which is why this check is still failing unowned. If that issue is fixed, re-replay the check id listed below to confirm.
Traces: ssm.create-activation-nonexistent-role-fails.

Cause 2 of 2: DescribeInstanceProperties accepts a syntactically malformed ActivationIds filter value instead of rejecting it.
AWS: Smithy: DescribeInstanceProperties models InvalidActivationId (client, 400) alongside InvalidInstancePropertyFilterValue, InvalidFilterKey, InvalidInstanceId, InvalidDocument, InvalidNextToken; InvalidActivationId is documented "The activation ID isn't valid. Verify that you entered the correct ActivationId or ActivationCode and try again." and carries a Message member. Recorded behavior for the message: HTTP 400, error code InvalidActivationId, message "Invalid Activation Ids <value>" — fixed text "Invalid Activation Ids " plus the rejected value(s) copied from the request's InstancePropertyFilterList valueSet. A well-formed ActivationId (GUID shape) that simply has no registrations stays a 200 with an empty InstanceProperties list.
Emulator: answers HTTP 200 with {"InstanceProperties": []} (content-type application/x-amz-json-1.1, content-length 25) for InstancePropertyFilterList=[{key: ActivationIds, valueSet: ["not-a-valid-activation-id"]}]; no error code and no message are emitted. The replay records: "real AWS rejects this call, but it SUCCEEDED and returned {'InstanceProperties': [], ...HTTPStatusCode: 200...}".
Why: a caller cannot distinguish "that activation id is malformed" from "that activation has no registered nodes" — both are an empty page — so a typo'd or wrongly-formatted activation id is silently treated as a valid filter that matches nothing, and retry/re-registration logic keyed on InvalidActivationId never runs.
Fix: DescribeInstanceProperties' filter-value validation (and its error classification) in that operation's handler: validate ActivationIds values against the ActivationId shape before any managed-node lookup and return 400 InvalidActivationId with Message "Invalid Activation Ids <rejected value(s) from the request>".
Related-but-not-covering: open issue 20261103T173026Z-ssm-describeinstanceinformation-and-describeinstanceproperties-filter-validation-is-missing-or.md covers DescribeInstanceProperties PlatformTypes VALUE validation (its Cause 3) and the ActivationIds classification on DescribeInstanceInformation (its Cause 5); neither is DescribeInstanceProperties' ActivationIds validation, which is net-new here.
Traces: ssm.describe-instance-properties-invalid-activation-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against this checkout; 50/479 passed.

Cause 1 — check ssm.create-activation-nonexistent-role-fails [mismatch]: after creating an IAM role trusted by ssm.amazonaws.com (positive control accepted, ActivationCode returned, activation visible via DescribeActivations), CreateActivation(IamRole=<never-created role name>, Description=..., RegistrationLimit=1) was expected to raise. Expected: HTTP 400, code ValidationException, message "Nonexistent role or missing ssm service principal in trust policy: arn:aws:iam::<account>:role/<IamRole>", and no activation minted. Actual: the call SUCCEEDED — harness raised AssertionError: "expected the call to raise, but it succeeded" from check_ssm_create_activation_nonexistent_role_fails.py line 72 (via run line 50).

Cause 2 — check ssm.describe-instance-properties-invalid-activation-rejected [mismatch]: positive control DescribeInstanceProperties(InstancePropertyFilterList=[{key: ActivationIds, valueSet: [<real activation id>]}]) correctly returned an empty InstanceProperties list; the negative case with valueSet=["not-a-valid-activation-id"] was expected to fail with code InvalidActivationId and message "Invalid Activation Ids not-a-valid-activation-id". Actual, verbatim from the replay: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'InstanceProperties': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '25', ...}, 'RetryAttempts': 0}} (check_ssm_describe_instance_properties_invalid_activation_rejected.py line 66, via run line 47).

Model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json — CreateActivation (input IamRole required/length 0..64; errors InvalidParameters, InternalServerError) and DescribeInstanceProperties (errors InvalidActivationId, InvalidInstancePropertyFilterValue, InvalidFilterKey, InvalidInstanceId, InvalidDocument, InvalidNextToken).

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite (run_behavioral_suite); the emulator boots and both checks below are replayed.

Cause 1 (check ssm.create-activation-nonexistent-role-fails):
2. iam:CreateRole a role whose trust policy allows Principal Service ssm.amazonaws.com; call ssm:CreateActivation with that IamRole — expect 200 with ActivationId/ActivationCode and the activation listed by DescribeActivations (this passes today).
3. Call ssm:CreateActivation with IamRole set to a role name that does not exist in the emulator's IAM state.
4. Observe HTTP 200 with an ActivationId/ActivationCode instead of HTTP 400 ValidationException "Nonexistent role or missing ssm service principal in trust policy: arn:aws:iam::<account>:role/<IamRole>"; DescribeActivations also shows the activation AWS would never have minted.

Cause 2 (check ssm.describe-instance-properties-invalid-activation-rejected):
5. Create an ssm-trusted role, CreateActivation with it, and call ssm:DescribeInstanceProperties with InstancePropertyFilterList=[{"key": "ActivationIds", "valueSet": [<that ActivationId>]}] — expect 200 with an empty InstanceProperties list (this passes today).
6. Repeat with valueSet=["not-a-valid-activation-id"].
7. Observe HTTP 200 and {"InstanceProperties": []} instead of HTTP 400 with code InvalidActivationId and Message "Invalid Activation Ids not-a-valid-activation-id".

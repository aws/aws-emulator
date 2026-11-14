Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-billing-configuration-monthly-reflected-in-get
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-billing-configuration-monthly-reflected-in-get
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: ssm|GetParameter|2xx->4xx|status
Issue-Commit: e4b73bef21735acad04abca14d336dea5ddc072f

# SSM GetParameter cannot resolve AWS-published /aws/... parameters, so a read AWS answers 200 is refused 400 ParameterNotFound

> _Note: this issue was identified against the code base at commit `e4b73bef21735acad04abca14d336dea5ddc072f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
1 cause.

Cause 1 of 1: GetParameter refuses a name that resolves on AWS.

AWS: GetParameter answers 200 with the `Parameter` member for any name the Region and account resolve, which includes the AWS-managed `/aws/...` namespace Systems Manager publishes in every account and Region (the public AMI aliases under `/aws/service/...`, say) — nobody creates those and no PutParameter is needed before they read back. The only refusals the model declares for this operation are `ParameterNotFound` (HTTP 400, "The parameter couldn't be found. Verify the name and try again."), `ParameterVersionNotFound` (400), `InvalidKeyId` (400) and `InternalServerError` (500); the service-common `ValidationException` covers the documented name rules ("If the specified name for a parameter contains spaces between characters, the request fails with a ValidationException error"). Smithy: GetParameter.Name -> PSParameterName @length(1..2048), required; errors ParameterNotFound | ParameterVersionNotFound | InvalidKeyId | InternalServerError (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json). Recorded behavior: the fixture's setup step reads one parameter and the recording's status for that read is 200.

Emulator: the same read answers HTTP 400. The emulator resolves a name ONLY out of the parameter store PutParameter wrote, keyed by (region, canonical name), and seeds nothing in the AWS-managed namespace — its own handler documentation states that a name in the `/aws/...` namespace "is the modelled ParameterNotFound" because "this emulator seeds none" of the published catalog values, even though the same crate DOES seed a published catalogue for documents (`PUBLISHED_DOCUMENTS`, resolved through `resolve_document`). So the refusal is `ParameterNotFound` with the message template "Parameter <Name> not found" — fixed text "Parameter" / "not found" around the requested name taken from the request. Hypothesis: the recorded name is an AWS-published parameter (an `/aws/service/...` alias), since that is the one class of name that resolves on AWS yet can never resolve in this emulator; the other 400s this handler can produce need a name with inner spaces or a trailing ':' (ValidationException), an unknown version/label on an existing parameter (ParameterVersionNotFound), or a WithDecryption read of a SecureString (AccessDeniedException), none of which a step recorded as 200 can be.

Why: a client that resolves configuration or an image id through Parameter Store — the standard way to obtain an AMI/image id or an AWS-published catalog value before creating a resource — gets a 400 where AWS gives it the value, so the whole workflow stops at its first read instead of proceeding. In this fixture the refused setup read blocks the rest of the scenario (the replay reports 4 blocked steps), so every assertion downstream of it is unexercised: the emulator looks healthy while no caller can complete an AWS-published-parameter lookup.

Fix: the parameter-resolution path of the ssm crate's state — give `ComAmazonawsSsmAmazonssmState` an AWS-published parameter catalogue resolved the way `resolve_document` already resolves published documents (created parameter first, published catalogue second), and have the GetParameter handler consult it before it falls through to ParameterNotFound; GetParameters/GetParametersByPath must read the same catalogue so a path listing and a single read agree. Reserve `ParameterNotFound` for names neither the store nor the published catalogue holds.

Traces: fixture workspaces-instances-billing-configuration-monthly-reflected-in-get, step setup#4 (GetParameter, expected 200, got 400). Related but NOT covering this step (neither carries its replay key): open issue 20261029T204746Z-pcs-the-aws-published-public-ssm-parameter-namespace-aws.md (filed against pcs, /aws/service/pcs empty) and open issue 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md (filed against fis, SSM public AMI alias parameter). Code citations: services/service-com-amazonaws-ssm-amazonssm/src/ops/get_parameter.rs lines 28-37 (the documented refusal of the /aws/... namespace) and 219-273 (store-only resolution); services/service-com-amazonaws-ssm-amazonssm/src/state.rs lines 28-34 (the published-document catalogue the parameter path lacks).

## Evidence
Replay divergence (trace set simparity-wire-aws-v1): service=ssm, operation=GetParameter, fixture_id=workspaces-instances-billing-configuration-monthly-reflected-in-get, step_key=setup#4, verdict=mismatch, detail "status: expected 200, got 400" (divergence_key service=ssm|GetParameter|status=200->400|keys=status). Expected: HTTP 200 carrying Parameter (Name/Type/Value/Version/ARN/LastModifiedDate) for the requested name. Actual: HTTP 400 ParameterNotFound, message template "Parameter <Name> not found" (get_parameter.rs:219-223). 4 further steps of the same fixture were blocked behind this failure. No stub_path and no owning issue were reported for this cluster.

## Steps to Reproduce
1. Boot the emulator built from this checkout (ssm service mounted).
2. Replay fixture workspaces-instances-billing-configuration-monthly-reflected-in-get and let it run to setup#4, or equivalently call ssm GetParameter with Name set to an AWS-published parameter in the AWS-managed namespace (e.g. an /aws/service/... alias) that no caller has created — PutParameter cannot create one, since it refuses names prefixed with `aws` or `ssm`.
3. Observe HTTP 400 ParameterNotFound instead of HTTP 200 with the published parameter's value; the remaining steps of the fixture are blocked.

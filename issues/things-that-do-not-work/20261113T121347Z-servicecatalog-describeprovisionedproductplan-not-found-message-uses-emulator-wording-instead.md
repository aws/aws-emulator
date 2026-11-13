Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-provisioned-product-plan-and-record-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-provisioned-product-plan-and-record-not-found
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|DescribeProvisionedProductPlan|4xx->4xx|$.Message
Issue-Commit: 3bfb7cd772781c7f8fd33f0bb610479e140a70f3

# DescribeProvisionedProductPlan not-found message uses emulator wording instead of AWS's text

> _Note: this issue was identified against the code base at commit `3bfb7cd772781c7f8fd33f0bb610479e140a70f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProvisionedProductPlan

## Description
Cause 1 of 1: DescribeProvisionedProductPlan's ResourceNotFoundException carries the emulator's own wording rather than AWS's message text for an unknown plan id.

AWS: Smithy (com-amazonaws-servicecatalog-aws242servicecatalogservice): DescribeProvisionedProductPlan takes PlanId (target Id, required, @length 1..100, @pattern ^[a-zA-Z0-9_\-]*$) and models errors InvalidParametersException and ResourceNotFoundException, both client errors with httpError 400. The model does not state message text, so the message requirement is recorded behavior only: for a syntactically valid PlanId that names no plan, AWS answers HTTP 400 with the AWSJSON error type ResourceNotFoundException and the message template "No plan with ID <PlanId> exists." — the fixed text is "No plan with ID " and " exists." and the only variable part is the PlanId value taken verbatim from the request ("plan-zzzzzzzzzzzzz" in the recording is just the value that filled the template, not part of it).

Emulator: replay of fixture servicecatalog-provisioned-product-plan-and-record-not-found step task#2 shows the status (400) and the error type matching, but $.Message is "Plan not found: plan-zzzzzzzzzzzzz" — the emulator's own "<Thing> not found: <id>" phrasing — where AWS's "No plan with ID plan-zzzzzzzzzzzzz exists." was expected. The divergence is confined to the message string; nothing per-run (request id, timestamp, host) is involved.

Why: clients and SDK-adjacent tooling that branch on, log, or surface the Service Catalog error text see a message AWS never emits. Error-message assertions in customer test suites fail against the emulator, operator-facing logs and CLI output read differently from production, and any consumer that pattern-matches the AWS wording (e.g. distinguishing a missing plan from a malformed PlanId) mis-classifies the failure even though the status and error code are right.

Fix: the DescribeProvisionedProductPlan handler's not-found path in the servicecatalog crate — the place that constructs the ResourceNotFoundException for an unresolved PlanId. It should build the message from AWS's template, interpolating the request's PlanId, instead of a generic "<entity> not found: <id>" helper. If that helper is shared across servicecatalog lookups, give the plan lookup its own AWS-worded message rather than changing the shared default.

Traces: servicecatalog-provisioned-product-plan-and-record-not-found#task#2 (expected $.Message "No plan with ID plan-zzzzzzzzzzzzz exists.", got "Plan not found: plan-zzzzzzzzzzzzz").

Related but not covering this: open issue 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md owns the portfolio/product message and metadata divergences, and 20261104T032856Z-servicecatalog-describeproduct-not-found-access-denied-error-message-uses.md owns DescribeProduct's not-found wording; neither carries this operation's replay key.

## Evidence
Fixture servicecatalog-provisioned-product-plan-and-record-not-found, step task#2, operation DescribeProvisionedProductPlan.

Expected (AWS, recorded): HTTP 400, error type ResourceNotFoundException, $.Message = "No plan with ID plan-zzzzzzzzzzzzz exists."
Actual (emulator): HTTP 400, error type ResourceNotFoundException, $.Message = "Plan not found: plan-zzzzzzzzzzzzz"

Replay detail verbatim: $.Message: value_mismatch (expected 'No plan with ID plan-zzzzzzzzzzzzz exists.', got 'Plan not found: plan-zzzzzzzzzzzzz')

Message anatomy: fixed text "No plan with ID " + " exists."; variable part = the PlanId supplied in the request. Status and error code already agree; only the message text diverges.

Model evidence: Smithy DescribeProvisionedProductPlan errors = [InvalidParametersException (client, 400), ResourceNotFoundException (client, 400)]; input member PlanId target Id, required, @length 1..100, @pattern ^[a-zA-Z0-9_\-]*$. No message text is modelled, so the wording requirement is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout (servicecatalog is bootstrapped; no pending bootstrap issue).
2. Replay fixture servicecatalog-provisioned-product-plan-and-record-not-found and stop at step task#2, or equivalently call the servicecatalog AWSJSON target DescribeProvisionedProductPlan with {"PlanId": "plan-zzzzzzzzzzzzz"} (any syntactically valid plan id matching ^[a-zA-Z0-9_\-]*$ that no CreateProvisionedProductPlan call created).
3. Observe HTTP 400 with __type ResourceNotFoundException and Message "Plan not found: plan-zzzzzzzzzzzzz".
4. AWS answers the same status and error type but Message "No plan with ID plan-zzzzzzzzzzzzz exists." — substitute any other unknown plan id and the fixed text must stay "No plan with ID <PlanId> exists."

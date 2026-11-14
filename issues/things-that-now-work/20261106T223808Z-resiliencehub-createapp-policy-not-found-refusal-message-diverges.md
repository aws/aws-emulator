Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-lifecycle
Issue-Step: task#8
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateApp|4xx->4xx|$.message
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# resiliencehub CreateApp policy-not-found refusal message diverges from AWS wording

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApp

## Description
One cause.

=== Cause 1 of 1: CreateApp's ResourceNotFoundException message text for a non-existent policyArn is not AWS's text ===

AWS: CreateApp accepts an optional `policyArn` (Smithy: CreateApp.policyArn -> Arn, with the standard ARN pattern; CreateApp errors include ResourceNotFoundException, errorKind client, httpError 404; ResourceNotFoundException carries `message`). When the supplied policyArn names a resiliency policy that does not exist, AWS refuses the create with HTTP 404 / ResourceNotFoundException and does NOT create the app. The message wording itself is recorded behavior only (the model documents the error, not its text); the recorded response shows the template:

  `Cannot attach policy <policyArn> to app <appName>. Policy <policyArn> does not exist.'`

Its FIXED text is `Cannot attach policy `, ` to app `, `. Policy `, and the trailing ` does not exist.'` — note the terminating period followed by a single apostrophe, which is part of AWS's fixed text for this error class. Its VARIABLE parts are the `policyArn` echoed from the request (twice) and the app `name` from the request; neither the recorded ARN nor the recorded app name is part of the requirement.

Emulator: returns the right status (404) and the right error, and otherwise the right sentence, but emits the fixed text without the trailing apostrophe — it ends the message at `does not exist.` So the response body's `$.message` differs from AWS's for every request in this error class, independent of the request's values.

Why: a client that keys off the refusal text — log scrapers, error-classification rules, test assertions that pin the service's message, and any code that string-matches the policy-attach failure to distinguish it from other 404s on this path — matches against AWS and fails against the emulator, so a negative test that passes in production fails here (or vice versa) for a reason unrelated to the behavior under test. Because status and error type are already correct, this is a text-fidelity defect only, hence LOW.

Fix: the message formatting in the CreateApp handler's policy-existence validation — the format string that builds the ResourceNotFoundException message must reproduce AWS's fixed text verbatim, including the trailing apostrophe after `does not exist.`, while continuing to interpolate the request's policyArn and app name.

Note: the open issue 'resiliencehub accepts invalid app-version and policy input with 200 instead of the modeled refusal' (20261029T195027Z-resiliencehub-accepts-invalid-app-version-and-policy-input.md) covers write paths that fail to refuse at all on CreateAppVersionAppComponent / CreateAppVersionResource / PublishAppVersion / CreateResiliencyPolicy; it does not mention CreateApp and does not cover a refusal that fires correctly with the wrong text. The open issue 'resiliencehub StartAppAssessment unreleased-version refusal message diverges from AWS wording' (20261104T232146Z) is the same class of fault on a different operation and does not carry this step's replay key.

Traces: resiliencehub-app-lifecycle#task#8.

## Evidence
Cause 1 — CreateApp, fixture resiliencehub-app-lifecycle, step task#8:
  expected_status: 404   actual_status: 404  (status and error class are correct)
  $.message: value_mismatch
    expected: "Cannot attach policy <policyArn> to app <appName>. Policy <policyArn> does not exist.'"
    actual:   "Cannot attach policy <policyArn> to app <appName>. Policy <policyArn> does not exist."
  Difference: the emulator drops the trailing apostrophe that terminates AWS's fixed text; every other character of the sentence matches.
  In the recording the variable slots were filled with the request's policyArn (a resiliency-policy ARN that was never created) and the request's app name; those recorded values are NOT the requirement — only the fixed text around them is.
  Replay divergence_key: service=resiliencehub|CreateApp|status=404->404|keys=$.message
  Smithy citations: CreateApp.policyArn -> Arn (optional); CreateApp errors include ResourceNotFoundException (client, httpError 404); ResourceNotFoundException carries `message`. The message text itself is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApp(name=<app>, policyArn=arn:aws:resiliencehub:<region>:<account>:resiliency-policy/<uuid-that-was-never-created>) — i.e. a well-formed resiliency-policy ARN for a policy that does not exist in state.
3. Observe HTTP 404 with $.message ending `... does not exist.` where AWS ends it `... does not exist.'` (trailing apostrophe).
4. Full replay: call replay_wire_corpus() and re-check fixture resiliencehub-app-lifecycle step task#8; the step must report $.message equal to AWS's fixed text, with policyArn and app name taken from the request.

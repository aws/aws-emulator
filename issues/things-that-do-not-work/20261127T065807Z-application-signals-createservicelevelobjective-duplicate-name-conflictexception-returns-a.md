Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-slo-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-slo-error-paths
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|CreateServiceLevelObjective|4xx->4xx|$.Message
Issue-Commit: a979fccb6b52301cb1b692ed521070b8834453a1

# application-signals CreateServiceLevelObjective duplicate-Name ConflictException returns a generic conflict sentence instead of the recorded Slo-already-exists message

> _Note: this issue was identified against the code base at commit `a979fccb6b52301cb1b692ed521070b8834453a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLevelObjective

## Description
Cause 1 of 1: CreateServiceLevelObjective's duplicate-Name 409 ConflictException carries the wrong Message text.

AWS: Smithy (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json): CreateServiceLevelObjective POST /slo, success code 200; input member Name required, target ServiceLevelObjectiveName with pattern ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$; modeled errors AccessDeniedException(403), ConflictException(409), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). ConflictException is errorKind client, httpError 409, with one required member Message, documented "This operation attempted to create a resource that already exists." The model does not state the message text, so for the wording this is recorded behavior only: the fixture records that when CreateServiceLevelObjective is called with a Name that already exists, AWS answers HTTP 409 ConflictException with Message of the form

    Slo with arn '<arn-of-existing-slo>' already exists.

Fixed text: `Slo with arn '` + `' already exists.` including the trailing period. Variable part: exactly one — the ARN of the already-existing SLO named by the request, i.e. arn:aws:application-signals:<region>:<account-id>:slo/<Name>, taken from the request's Name and the emulator's own region/account/state. The region `us-west-2`, the account id `000000000000` and the SLO name `sim2real-probe-application-signals-slo-errors-a7ce4fa4` in the recording are that run's values filling the template, not part of the requirement.

Emulator: the status is right (409) but the body's Message is the crate's shared generic conflict sentence `A resource with the same identifier already exists` — no ARN, no trailing period, nothing derived from the request. Replay reports $.Message value_mismatch at application-signals-slo-error-paths#task#5: expected "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-probe-application-signals-slo-errors-a7ce4fa4' already exists.", got 'A resource with the same identifier already exists'.

Why: a client provisioning SLOs idempotently (a converger, a retry-on-create wrapper, a Terraform/CDK-style tool) reads the ConflictException Message to learn WHICH resource it collided with and to recover the existing SLO's ARN without a second ListServiceLevelObjectives/GetServiceLevelObjective round trip, and log/alert rules match on the `Slo with arn ... already exists.` phrasing. Against the emulator the ARN is absent and the phrasing never matches, so such a client cannot identify the conflicting SLO and misclassifies or fails the conflict path that would succeed against AWS.

Fix: the duplicate-Name branch of the CreateServiceLevelObjective handler / the SLO store's Name-uniqueness check — it must format this operation's own conflict sentence, interpolating the existing SLO's ARN, rather than delegating to the crate's shared generic "A resource with the same identifier already exists" already-exists helper. Note the direction: for this operation the ARN-bearing sentence is the recorded AWS behavior.

Related open issue (does NOT cover this): 20261102T091730Z-application-signals-createservicelevelobjective-duplicate-name-conflictexception-under-concurrency.md, and the bundle it names (20261029T192739Z-application-signals-words-seven-refusal-messages-differently-from.md, its Cause 4), both assert the OPPOSITE wording for this same uniqueness guard — they call the generic sentence correct and the ARN-echoing one the defect. Neither carries this fixture step's replay key, and the wire recording in this corpus is the authority for the wire shape, so this divergence is filed separately. A fixer must reconcile the two before editing: whichever sentence is chosen, the sequential duplicate-Name path exercised by this fixture step must reproduce the recorded message above, and the concurrent path must produce the identical text.

Traces: application-signals-slo-error-paths#task#5.

## Evidence
Fixture application-signals-slo-error-paths, step task#5, verdict mismatch.
divergence_key: service=application-signals|CreateServiceLevelObjective|status=409->409|keys=$.Message

HTTP status: expected 409, actual 409 — CORRECT, not part of this defect.
Error code: ConflictException (httpError 409 per the model) — CORRECT.
Body member $.Message: value_mismatch.

  expected: "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-probe-application-signals-slo-errors-a7ce4fa4' already exists."
  actual:   "A resource with the same identifier already exists"

Breakdown of the expected message:
  fixed text    -> "Slo with arn '" ... "' already exists." (trailing period included)
  variable part -> the ARN of the pre-existing SLO whose Name the request reused:
                   arn:aws:application-signals:<region>:<account-id>:slo/<Name>
                   (here us-west-2 / 000000000000 / sim2real-probe-application-signals-slo-errors-a7ce4fa4 — per-run values of this recording, not requirements)
The actual message contains no variable part at all and omits the trailing period.

Model citation: CreateServiceLevelObjective POST /slo code 200; ConflictException errorKind client, httpError 409, required member Message, doc "This operation attempted to create a resource that already exists." The model fixes the status and the member, not the text; the text requirement comes from the recording.

Replay summary for context: 62 steps, 61 passed, 1 mismatched, score 98.4%, emulator_up true, bootstrap_signal false.

## Steps to Reproduce
Boot the emulator and point an `application-signals` client at its data listener.

1. Create an SLO: CreateServiceLevelObjective(Name=N, <valid SliConfig or RequestBasedSliConfig>), with N matching ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$. Note the returned Slo.Arn.
2. Call CreateServiceLevelObjective again with the SAME Name=N.
3. Observe the refusal: HTTP 409, Error.Code "ConflictException" — both correct.
4. Inspect Error.Message. The emulator returns "A resource with the same identifier already exists"; the recorded AWS response for this step is "Slo with arn '<the ARN from step 1>' already exists." — i.e. the message must interpolate the existing SLO's ARN and end with a period.
5. Re-replay fixture application-signals-slo-error-paths, step task#5, to confirm a fix.

Cleanup: DeleteServiceLevelObjective(Id=<Arn from step 1>).

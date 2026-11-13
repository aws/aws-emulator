Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*]
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].Description
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].EventType
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].TriggeredBy
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.PercentageComplete
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.State
Issue-Key: appconfig|StartDeployment|4xx->4xx|$.Message
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*]
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*].TriggeredBy
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.PercentageComplete
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*].ActionInvocations
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*].Description
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*].EventType
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*].TriggeredBy
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.PercentageComplete
Issue-Related: 20261029T190217Z-appconfig-stop-operations-diverge-stopdeployment-leaves-the-rollback.md
Issue-Commit: eb95e56228ce55bd248c3581c6fd09ba0e6c8f48

# AppConfig deployment lifecycle advances instantly through deploy, bake and rollback instead of progressing

> _Note: this issue was identified against the code base at commit `eb95e56228ce55bd248c3581c6fd09ba0e6c8f48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
This issue bundles 4 net-new causes sharing one root cause: for an all-at-once strategy with a non-zero FinalBakeTimeInMinutes, the emulator runs the whole rollout inside the StartDeployment call — it jumps straight past DEPLOYING to BAKING with PercentageComplete already 100.0 — and conversely it does NOT run the rollback that StopDeployment performs synchronously on AWS, leaving the deployment in ROLLING_BACK. The event log is correspondingly wrong in order, type, description and TriggeredBy, and omits the PERCENTAGE_UPDATED event and the per-event ActionInvocations list entirely.

Note: the open issue 20261029T190217Z-appconfig-stop-operations-diverge-stopdeployment-leaves-the-rollback.md describes the unfinished StopDeployment rollback for the appconfig-deployment-stop-rollback fixture; the replay reports the clusters below as NOT owned by it, and Causes 1, 2 and 4 are about the deploy/bake phase rather than the stop, so they are filed here.

Cause 1 of 4: StartDeployment returns a deployment that has already left the DEPLOYING phase.
  AWS: Smithy: StartDeployment POST .../deployments, http code 201, output Deployment with State -> DeploymentState enum [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED], PercentageComplete -> Percentage, EventLog -> DeploymentEvents (DeploymentEvent{EventType -> DeploymentEventType [PERCENTAGE_UPDATED, ROLLBACK_STARTED, ROLLBACK_COMPLETED, BAKE_TIME_STARTED, DEPLOYMENT_STARTED, DEPLOYMENT_COMPLETED, REVERT_COMPLETED], TriggeredBy -> TriggeredBy [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR], Description, ActionInvocations, OccurredAt}). Recorded behavior: the 201 response carries State `DEPLOYING`, PercentageComplete 0.0 and exactly ONE event — EventType `DEPLOYMENT_STARTED`, TriggeredBy `USER`, Description `Deployment started`. The bake period is entered later, by AppConfig, not by the caller's request.
  Emulator: the same 201 carries State `BAKING`, PercentageComplete 100.0, and TWO events: newest is EventType `BAKE_TIME_STARTED`, TriggeredBy `APPCONFIG`, Description `Deployment bake time started for 10 minute(s)`, followed by the DEPLOYMENT_STARTED event.
  Why: a client that starts a deployment and then polls for the deployment to leave DEPLOYING sees it already past that phase on the very first response, so it cannot observe the rollout at all; PercentageComplete 100.0 at creation tells it the configuration is fully rolled out before any growth tick has happened.
  Fix: StartDeployment's handler/state transition for the appconfig deployment state machine — the initial state must be DEPLOYING with PercentageComplete 0.0 and a single USER-triggered DEPLOYMENT_STARTED event; entering bake must be a separate, AppConfig-triggered transition.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#10.

Cause 2 of 4: the in-flight-deployment conflict message names the wrong state.
  AWS: Smithy: StartDeployment errors include ConflictException (httpError 409). Recorded behavior: starting a second deployment while one is in flight is refused 409 with Message `Environment <environmentId> currently has a state of DEPLOYING. The current deployment must finish before a new deployment can be started.` The fixed text is `Environment `, ` currently has a state of `, `. The current deployment must finish before a new deployment can be started.`; the variable parts are the environment's logical id and the current DeploymentState value taken from emulator state.
  Emulator: same 409 and the same fixed text, but the state value rendered is `BAKING` instead of `DEPLOYING`, because of Cause 1.
  Why: the message is the only place the client learns WHY it was refused and what to wait for; reporting BAKING when AWS reports DEPLOYING misleads a client that branches on the reported phase.
  Fix: once Cause 1 is fixed the value follows from state; the message template itself lives in StartDeployment's conflict validation.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#12 (status matches at 409; only the state word in $.Message diverges).

Cause 3 of 4: StopDeployment leaves the deployment mid-rollback instead of rolled back.
  AWS: Smithy: StopDeployment DELETE .../deployments/{DeploymentNumber}, http code 202, output Deployment. Recorded behavior: the 202 response already reports the completed rollback — State `ROLLED_BACK`, PercentageComplete 0.0, newest event EventType `ROLLBACK_COMPLETED` / Description `Deployment rolled back by user request` / TriggeredBy `USER`, with the earlier events preserved as `Deployment bake time started` (BAKE_TIME_STARTED, APPCONFIG), `Configuration available to 100.00% of clients` (PERCENTAGE_UPDATED, APPCONFIG) and `Deployment started` (DEPLOYMENT_STARTED, USER).
  Emulator: State `ROLLING_BACK`, PercentageComplete 100.0, newest event `ROLLBACK_STARTED` / `Deployment rollback started`; the PERCENTAGE_UPDATED event is absent, the bake description carries the extra ` for 10 minute(s)` suffix, and the DEPLOYMENT_STARTED event sits one slot earlier than AWS places it.
  Why: a client that calls StopDeployment and then checks the returned State for a terminal value never sees one, so it either loops forever or treats the stop as failed.
  Fix: StopDeployment's handler/state transition must complete the revert within the call and append the USER-triggered ROLLBACK_COMPLETED event; the PERCENTAGE_UPDATED event must be recorded when the rollout reaches a percentage.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13.

Cause 4 of 4: GetDeployment after the stop reports a different event log, and never emits ActionInvocations.
  AWS: Smithy: GetDeployment GET .../deployments/{DeploymentNumber}, code 200, output Deployment; DeploymentEvent.ActionInvocations -> ActionInvocations list ("The list of extensions that were invoked as part of the deployment"). Recorded behavior: every event in the log carries an `ActionInvocations` member — an EMPTY list when no extension action fired — and the log reads newest-first: `Deployment rolled back by user request` (ROLLBACK_COMPLETED, USER), `Deployment bake time started` (BAKE_TIME_STARTED, APPCONFIG), `Configuration available to 100.00% of clients` (PERCENTAGE_UPDATED, APPCONFIG), `Deployment started` (DEPLOYMENT_STARTED, USER); PercentageComplete is 0.0 after the rollback.
  Emulator: omits `ActionInvocations` on every event (null rather than []); reports the rollback as two events (`Deployment rollback completed` TriggeredBy APPCONFIG, then `Deployment rollback started` TriggeredBy USER) rather than AWS's single USER-triggered `Deployment rolled back by user request`; still has no PERCENTAGE_UPDATED event; keeps the ` for 10 minute(s)` suffix on the bake description; and reports PercentageComplete 100.0 after a rollback.
  Why: a client that iterates a deployment's event log to attribute extension invocations gets a null where AWS guarantees a list (a null-pointer/deserialization hazard), and audit logic keyed on EventType/TriggeredBy mis-attributes a user-requested rollback to AppConfig. PercentageComplete 100.0 after a rollback reports the reverted configuration as fully deployed.
  Fix: the Deployment/DeploymentEvent serializer (always emit ActionInvocations, defaulting to an empty list) plus the event-log recording in the deployment state machine.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#14.

## Evidence
Cause 1 — ...#task#10 StartDeployment (201 both sides): $.State expected 'DEPLOYING', got 'BAKING'; $.PercentageComplete expected 0.0, got 100.0; $.EventLog[0].EventType expected 'DEPLOYMENT_STARTED', got 'BAKE_TIME_STARTED'; $.EventLog[0].Description expected 'Deployment started', got 'Deployment bake time started for 10 minute(s)'; $.EventLog[0].TriggeredBy expected 'USER', got 'APPCONFIG'; $.EventLog[1] extra (an additional DEPLOYMENT_STARTED/USER event AWS does not return at creation).
Cause 2 — ...#task#12 StartDeployment (409 both sides): $.Message expected 'Environment <envId> currently has a state of DEPLOYING. The current deployment must finish before a new deployment can be started.', got the same text with 'BAKING' in place of 'DEPLOYING'.
Cause 3 — ...#task#13 StopDeployment (202 both sides): $.State expected 'ROLLED_BACK', got 'ROLLING_BACK'; $.PercentageComplete expected 0.0, got 100.0; $.EventLog[0].EventType expected 'ROLLBACK_COMPLETED', got 'ROLLBACK_STARTED'; $.EventLog[0].Description expected 'Deployment rolled back by user request', got 'Deployment rollback started'; $.EventLog[1].Description expected 'Deployment bake time started', got 'Deployment bake time started for 10 minute(s)'; $.EventLog[2] expected PERCENTAGE_UPDATED/APPCONFIG 'Configuration available to 100.00% of clients', got DEPLOYMENT_STARTED/USER 'Deployment started'; $.EventLog[3] missing (expected the DEPLOYMENT_STARTED/USER event).
Cause 4 — ...#task#14 GetDeployment (200 both sides): $.EventLog[0..3].ActionInvocations missing (expected [], got None) on every event; $.EventLog[0].Description expected 'Deployment rolled back by user request', got 'Deployment rollback completed'; $.EventLog[0].TriggeredBy expected 'USER', got 'APPCONFIG'; $.EventLog[1] expected BAKE_TIME_STARTED/APPCONFIG 'Deployment bake time started', got ROLLBACK_STARTED/USER 'Deployment rollback started'; $.EventLog[2] expected PERCENTAGE_UPDATED 'Configuration available to 100.00% of clients', got BAKE_TIME_STARTED 'Deployment bake time started for 10 minute(s)'; $.PercentageComplete expected 0.0, got 100.0.

## Steps to Reproduce
1. Create an application, environment and hosted configuration profile, plus a deployment strategy with DeploymentDurationInMinutes 0 (all-at-once), GrowthType LINEAR, GrowthFactor 100 and FinalBakeTimeInMinutes 10.
2. Call StartDeployment with that strategy and inspect the 201 body: AWS reports State DEPLOYING, PercentageComplete 0.0 and a single DEPLOYMENT_STARTED/USER event; the emulator reports State BAKING, PercentageComplete 100.0 and a BAKE_TIME_STARTED/APPCONFIG event first.
3. Call StartDeployment again on the same environment: both refuse 409, but the emulator's message names the environment's state as BAKING where AWS names DEPLOYING.
4. Call StopDeployment on the in-flight deployment: AWS's 202 already reports State ROLLED_BACK / PercentageComplete 0.0 with a ROLLBACK_COMPLETED 'Deployment rolled back by user request' (USER) event; the emulator reports ROLLING_BACK / 100.0 with ROLLBACK_STARTED.
5. Call GetDeployment on it: AWS returns ActionInvocations: [] on every event and the four-event log above; the emulator omits ActionInvocations entirely, splits the rollback into two events, has no PERCENTAGE_UPDATED event and still reports PercentageComplete 100.0.
6. Replay appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value (task#10, task#12, task#13, task#14).

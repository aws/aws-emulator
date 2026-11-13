Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.get-configuration-nonexistent-profile-not-found
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.get-configuration-nonexistent-profile-not-found|run#18|AssertionError|assert err_env["Message"] == (
Issue-Commit: 2bc4f9350f4794788afdc3a762d704c686e40d96

# AppConfig GetConfiguration returns the profile-not-found message when the Environment does not resolve, instead of AWS's "No deployed version" BadRequestException

> _Note: this issue was identified against the code base at commit `2bc4f9350f4794788afdc3a762d704c686e40d96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguration

## Description
One net-new cause.

Cause 1 of 1: the deprecated GetConfiguration path conflates "this (app, environment, profile) triple has no deployed version" with "the configuration profile could not be found", so an unresolvable Environment is reported as a missing Configuration Profile.

AWS: Smithy (com.amazonaws.appconfig#AmazonAppConfig): GetConfiguration GET /applications/{Application}/environments/{Environment}/configurations/{Configuration}, all three identifiers @httpLabel @required (StringWithLengthBetween1And64), ClientId @required @httpQuery('client_id'); modelled errors BadRequestException (httpError 400), InternalServerException (500), ResourceNotFoundException (404); the operation is marked @deprecated in favour of GetLatestConfiguration. The model does not state the resolution order, so this is recorded behavior only: this deprecated path resolves the (Application, Configuration) pair FIRST — when that pair does not resolve it refuses 400 BadRequestException with Message `Configuration Profile with Application Id <Application> and Configuration Profile Id <Configuration> could not be found for account <accountId>.` (fixed text `Configuration Profile with Application Id `, ` and Configuration Profile Id `, ` could not be found for account `, `.`; the variable parts are the Application and Configuration labels from the request and the caller's account id). The Environment label is used only to look up a *deployment*, so an Environment that does not resolve is NOT a profile error and is NOT a 404: it gets the same refusal as a real Environment with nothing deployed — 400 BadRequestException with Message `No deployed version for configuration profile <Configuration> in environment <Environment> and app <Application>` (fixed text `No deployed version for configuration profile `, ` in environment `, ` and app `; the three variable parts are the Configuration, Environment and Application labels exactly as given in the request — no trailing period, no account id).

Emulator: for a request whose Application and Configuration DO resolve but whose Environment does not, the emulator refuses with error code BadRequestException but with the profile-level message `Configuration Profile with Application Id <Application> and Configuration Profile Id <Configuration> could not be found for account <accountId>.` — i.e. it claims the configuration profile that the caller just created, and that the emulator itself resolves for other calls, could not be found, and it never emits the `No deployed version for configuration profile ... in environment ... and app ...` wording at all. The pair-not-found branch (unknown Application, and unknown Configuration) is served correctly, so only the environment/deployment-lookup branch is wrong. The replayed check stopped at this assertion, so the sibling case it covers next — a fully resolvable triple with nothing deployed yet, which AWS refuses with the same `No deployed version ...` message — is on the same unverified branch and must be fixed with it.

Why: the message is the requirement and here it is actively misleading. A client that gets "Configuration Profile ... could not be found" concludes its profile id is wrong (or that the profile was deleted) and goes off to re-create or re-look-up the profile, when the real condition is either a bad environment label or simply "nothing deployed to this environment yet" — the normal pre-first-deployment state, which clients and SDK wrappers detect precisely by matching AWS's `No deployed version for ...` text. Operators also lose the ability to distinguish a typo in the environment from a genuinely missing profile.

Fix: GetConfiguration's handler resolution/refusal path in the appconfig service — after the (Application, Configuration) pair resolves, the Environment must be treated as a deployment-lookup key only: a missing environment or a missing deployment for the triple must both raise BadRequestException with the `No deployed version for configuration profile <Configuration> in environment <Environment> and app <Application>` message built from the request's own labels, rather than falling back to the profile-not-found message.

Related open issues that do NOT own this cluster (the replay reports it unowned): 20261103T021132Z-appconfig-request-validation-and-refusal-divergences-on-startdeployment.md (its Cause 4 covers the pair-not-found message, which now passes) and 20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md (GetConfiguration answering 501).

Traces: behavioral check appconfig.get-configuration-nonexistent-profile-not-found.

## Evidence
Check appconfig.get-configuration-nonexistent-profile-not-found (mismatch).

Passing prefix: GetConfiguration with an unknown Application (and a real environment/profile) is refused with Code 'BadRequestException' and Message 'Configuration Profile with Application Id <fakeApp> and Configuration Profile Id <cpId> could not be found for account <accountId>.' — matches AWS.

Failing assertion: GetConfiguration(Application=<real app id>, Environment='sim2real-nonexistent-env-xyz', Configuration=<real profile id>, ClientId=...)
  expected Code    : BadRequestException  (emulator matched)
  expected Message : "No deployed version for configuration profile <cpId> in environment sim2real-nonexistent-env-xyz and app <appId>"
  actual   Message : "Configuration Profile with Application Id <appId> and Configuration Profile Id <cpId> could not be found for account <accountId>."
  AssertionError: unexpected message for an unknown Environment: 'Configuration Profile with Application Id ho65pb1 and Configuration Profile Id tn20ygr could not be found for account 000000000000.' (the ids/account in that line are this run's generated values; the requirement is the fixed text plus the request's own labels).

The same `No deployed version for configuration profile <Configuration> in environment <Environment> and app <Application>` message is also what AWS returns for the next assertion in the check (a fully resolvable triple with no deployment), which the check never reached.

## Steps to Reproduce
1. CreateApplication, then CreateEnvironment and CreateConfigurationProfile (LocationUri='hosted', Type='AWS.Freeform') under it. Note the application id, environment id and configuration profile id.
2. Call the deprecated GetConfiguration with Application=<app id>, Configuration=<profile id>, ClientId=<any>, but Environment set to a label that does not exist (e.g. 'sim2real-nonexistent-env-xyz').
   - AWS: 400 BadRequestException, Message 'No deployed version for configuration profile <profile id> in environment sim2real-nonexistent-env-xyz and app <app id>'.
   - Emulator: BadRequestException with 'Configuration Profile with Application Id <app id> and Configuration Profile Id <profile id> could not be found for account <account id>.'
3. Call the same GetConfiguration with the REAL environment id while nothing has been deployed to it: AWS returns the same 'No deployed version for configuration profile <profile id> in environment <env id> and app <app id>' message.
4. For contrast, call GetConfiguration with an unknown Application or an unknown Configuration: AWS (and the emulator) return 'Configuration Profile with Application Id ... and Configuration Profile Id ... could not be found for account ....' — that branch is correct and must stay.
5. Re-replay behavioral check appconfig.get-configuration-nonexistent-profile-not-found; it must also reach its final step, where after a completed deployment the identical GetConfiguration returns the deployed Content and ContentType.

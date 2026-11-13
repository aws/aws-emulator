Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.deployment-strategy-invalid-growth-factor-rejected appconfig.op-contract-createDeploymentStrategy appconfig.create-experiment-definition-invalid-treatment-weights-rejected appconfig.update-experiment-definition-invalid-schedule-rejected appconfig.update-experiment-run-malformed-input-rejected appconfig.experiment-run-second-stop-rejected appconfig.experiment-run-stop-transitions-and-second-stop-rejected appconfig.stop-already-stopped-experiment-run-conflict appconfig.stop-experiment-run-conflict-when-not-running appconfig.stop-experiment-run-not-started-rejected appconfig.extension-duplicate-name-conflict-rejected appconfig.op-contract-getConfiguration appconfig.create-extension-version-conflict-rejected appconfig.delete-extension-with-active-association-rejected appconfig.update-extension-stale-version-conflict-rejected appconfig.hosted-configuration-version-numeric-version-label-rejected appconfig.stop-deployment-allow-revert-reverts-completed-deployment appconfig.validate-configuration-invalid-json-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.deployment-strategy-invalid-growth-factor-rejected|run#13|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: appconfig.op-contract-createDeploymentStrategy|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.create-experiment-definition-invalid-treatment-weights-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.update-experiment-definition-invalid-schedule-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.update-experiment-run-malformed-input-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.experiment-run-second-stop-rejected|run#15|AssertionError|assert err["Code"] == "BadRequestException", (
Issue-Key: appconfig.experiment-run-stop-transitions-and-second-stop-rejected|run#13|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException on second stop, got {err['Code']!r}"
Issue-Key: appconfig.stop-already-stopped-experiment-run-conflict|run#15|AssertionError|assert err["Code"] == "BadRequestException", (
Issue-Key: appconfig|StopExperimentRun|ConflictException
Issue-Key: appconfig.stop-experiment-run-not-started-rejected|run#14|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException for a stop on an already-DONE run, got {err}"
Issue-Key: appconfig.extension-duplicate-name-conflict-rejected|run#9|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException, got {err['Code']}"
Issue-Key: appconfig.op-contract-getConfiguration|run#13|AssertionError|assert error["Code"] == "BadRequestException", (
Issue-Key: appconfig.create-extension-version-conflict-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.delete-extension-with-active-association-rejected|run#16|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: appconfig.update-extension-stale-version-conflict-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.hosted-configuration-version-numeric-version-label-rejected|run#9|AssertionError|assert code_msg == (
Issue-Key: appconfig.stop-deployment-allow-revert-reverts-completed-deployment|run#39|AssertionError|assert err2["Message"] == (
Issue-Key: appconfig.validate-configuration-invalid-json-rejected|run#14|AssertionError|assert message_malformed == expected_malformed, (
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# AppConfig control-plane refusals: wrong error classes and six divergent refusal/validation messages (10 causes)

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopExperimentRun

## Description
The AppConfig emulator does refuse these calls, but it refuses them with the wrong error class and/or a message whose text diverges from AWS's. 10 causes are bundled; every one is a separate defect in a refusal path of the appconfig service and each must be closed before this issue is submitted.

Cause 1 of 10: constraint-violation messages render float-typed members as integers.
AWS: Smithy CreateDeploymentStrategy.GrowthFactor targets GrowthFactor (float, @range min 1.0 max 100.0); Treatment/TreatmentInput.Weight targets Weight (float, @range min 0.0); UpdateExperimentRun.ExposurePercentage targets NullablePercentage (float, @range min 0.0 max 100.0). Recorded behavior: a value outside the modeled range is refused with BadRequestException (HTTP 400) and the message "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have value less than or equal to <bound>" (or "... greater than or equal to <bound>"). FIXED text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have value less than or equal to " / "Member must have value greater than or equal to ". VARIABLE parts: the rejected value from the request, the lowerCamel member path, and the modeled bound — and because the member is float-typed, AWS renders BOTH the value and the bound as float literals ('150.0', '-5.0', '100.0', '0.0'), never as bare integers.
Emulator: renders both as integers — "Value '150' at 'growthFactor' ... less than or equal to 100", "Value '-5' at 'treatments.1.member.weight' ... greater than or equal to 0", "Value '150' at 'exposurePercentage' ... less than or equal to 100".
Why: the message is part of the contract for this error class. A client (or a test harness, or a support tool) that matches or displays AWS's wording sees text that does not match for every float member in the service, and the rendering silently misreports the type of the member that failed.
Fix: the shared constraint-violation message formatter used by the appconfig input-validation layer — a member whose modeled target is a float must have its value and its @range bound formatted as a decimal.
Traces: appconfig.deployment-strategy-invalid-growth-factor-rejected, appconfig.op-contract-createDeploymentStrategy, appconfig.create-experiment-definition-invalid-treatment-weights-rejected, appconfig.update-experiment-definition-invalid-schedule-rejected, appconfig.update-experiment-run-malformed-input-rejected.

Cause 2 of 10: StopExperimentRun on a run that is not RUNNING answers 409 ConflictException instead of 400 BadRequestException.
AWS: Smithy StopExperimentRun (PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns/{Run}/stop, success 200) models BadRequestException (400), ConflictException (409), ResourceNotFoundException (404) and InternalServerException (500), so which class applies is a contract and not a choice. Recorded behavior: stopping a run that is already DONE (or was never started) is refused with BadRequestException / HTTP 400.
Emulator: answers HTTP 409 ConflictException with Message "Run <run> of experiment definition <definitionId> is DONE, so it cannot be stopped: only a running experiment can be stopped." — wrong status, wrong error code, and wording AWS does not use (the already-filed wire issue 20261029T190217Z-appconfig-stop-operations-diverge-stopdeployment-leaves-the-rollback.md records AWS's message for this refusal as "Experiment run <run> for experiment definition <definitionId> is not running.", whose fixed text is "Experiment run " / " for experiment definition " / " is not running." and whose variable parts are the run number and definition id from the request path).
Why: SDK clients dispatch on the modeled exception type; a caller catching BadRequestException never catches this and the error escapes as an unexpected ConflictException, while retry layers treat 409 as retriable and retry a call that can never succeed.
Fix: the StopExperimentRun handler's run-state check in the appconfig service.
Related open issue (does not cover these checks): 20261029T190217Z-appconfig-stop-operations-diverge-stopdeployment-leaves-the-rollback.md carries this same root cause for its WIRE fixture keys only, and 20261025T142404Z-appconfig-stopexperimentrun-stub.md is superseded because the operation now answers.
Traces: appconfig.experiment-run-second-stop-rejected, appconfig.experiment-run-stop-transitions-and-second-stop-rejected, appconfig.stop-already-stopped-experiment-run-conflict, appconfig.stop-experiment-run-conflict-when-not-running, appconfig.stop-experiment-run-not-started-rejected.

Cause 3 of 10: CreateExtension with a duplicate Name and no LatestVersionNumber answers ConflictException instead of BadRequestException.
AWS: Smithy CreateExtension (POST /extensions, success 201) models BadRequestException (400), ConflictException (409), ServiceQuotaExceededException (402), InternalServerException (500). Recorded behavior: re-creating an extension whose Name already exists, with LatestVersionNumber omitted, is refused with BadRequestException and Message "LatestVersionNumber not populated and an extension with name <name> already exists." — fixed text "LatestVersionNumber not populated and an extension with name " / " already exists.", variable part the Name from the request.
Emulator: answers ConflictException for the same call.
Why: the two cases are distinguishable on real AWS — a MISSING lock is a bad request, a MISMATCHED lock is a conflict (see Cause 5). Collapsing both onto ConflictException makes a client unable to tell "you forgot the lock" from "your lock is stale", and breaks catch-by-type.
Fix: the CreateExtension handler's name/version pre-check in the appconfig service.
Traces: appconfig.extension-duplicate-name-conflict-rejected.

Cause 4 of 10: GetConfiguration answers ResourceNotFoundException where AWS answers BadRequestException.
AWS: Smithy GetConfiguration (GET /applications/{Application}/environments/{Environment}/configurations/{Configuration}, deprecated) models BadRequestException (400), ResourceNotFoundException (404) and InternalServerException (500). Recorded behavior: the legacy resolution path reports every "nothing to serve for this Application/Environment/Configuration" condition — including a syntactically valid Configuration id that was never created — as BadRequestException / HTTP 400, with Message "Configuration Profile with Application Id <applicationId> and Configuration Profile Id <configurationProfileId> could not be found for account <accountId>." (fixed text "Configuration Profile with Application Id " / " and Configuration Profile Id " / " could not be found for account " / "."; variable parts the two ids from the request and the caller's account).
Emulator: answers ResourceNotFoundException for that call.
Why: a client of the deprecated API that branches on BadRequestException (the documented class for this family) does not catch the error, and a 404 invites "create the resource" remediation for what AWS reports as a malformed/unservable request.
Fix: the GetConfiguration handler's resolution-failure mapping in the appconfig service.
Related open issue (does not cover this check): 20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md owns appconfig.get-configuration-nonexistent-profile-not-found (the unknown-Application variant); this check exercises the unknown-Configuration variant and is not in that issue's replay set.
Traces: appconfig.op-contract-getConfiguration.

Cause 5 of 10: CreateExtension's stale-LatestVersionNumber refusal uses the wrong message.
AWS: recorded behavior only — CreateExtension with an existing Name and a LatestVersionNumber that is not the extension's current version is refused with ConflictException and Message "LatestVersionNumber supplied in request <supplied> does not match latest version <actual>." Fixed text: "LatestVersionNumber supplied in request " / " does not match latest version " / "."; variable parts: the value supplied on the request and the extension's current version from state.
Emulator: refuses with the correct class but the message "Extension <name> is at version <actual>, not <supplied>."
Why: the message is the only place the conditional-write failure explains itself; a client or operator that parses or displays AWS's wording gets unrecognised text, and the emulator's variant also leaks the extension NAME where AWS reports only version numbers.
Fix: the CreateExtension handler's optimistic-lock failure path in the appconfig service.
Traces: appconfig.create-extension-version-conflict-rejected.

Cause 6 of 10: DeleteExtension's still-associated refusal uses the wrong message.
AWS: Smithy DeleteExtension (DELETE /extensions/{ExtensionIdentifier}, success 204) models BadRequestException (400). Recorded behavior: deleting an extension that still has associations is refused with Message "Cannot delete extension <extensionId>, version <versionNumber>, for account <accountId>, as there are <count> associations of the extension still existing." Fixed text: "Cannot delete extension " / ", version " / ", for account " / ", as there are " / " associations of the extension still existing."; variable parts: the extension id and version being deleted, the caller's account, and the live association count from state.
Emulator: "Extension <id> version <n> still has extension associations; delete them before deleting the extension." — same meaning, different wording, and it drops the account and the association count AWS reports.
Fix: the DeleteExtension handler's association pre-check in the appconfig service.
Traces: appconfig.delete-extension-with-active-association-rejected.

Cause 7 of 10: UpdateExtension with an unknown VersionNumber reports a more specific not-found message than AWS.
AWS: Smithy UpdateExtension (PATCH /extensions/{ExtensionIdentifier}) models ResourceNotFoundException (404). Recorded behavior: naming a VersionNumber that never existed is refused with ResourceNotFoundException and the DELIBERATELY unspecific Message "Extension could not be found for account <accountId>." — fixed text "Extension could not be found for account " / ".", the only variable part being the caller's account. The identical wording is recorded for GetExtension on a deleted extension, so this is AppConfig's single extension-not-found message.
Emulator: "Extension with Id <id> and Version Number <n> could not be found for account <accountId>."
Why: clients and tests keyed to AWS's wording do not match, and the emulator discloses which of the id/version was resolvable where AWS deliberately does not.
Fix: the UpdateExtension handler's version lookup failure in the appconfig service (and the shared extension-not-found message).
Traces: appconfig.update-extension-stale-version-conflict-rejected.

Cause 8 of 10: the versionLabel pattern refusal quotes the raw model pattern instead of the pattern AWS reports.
AWS: Smithy CreateHostedConfigurationVersion.VersionLabel targets VersionLabel (@length 1-64, @pattern "[^0-9]"). Recorded behavior: an all-digits label is refused with BadRequestException (400) and Message "1 validation error detected: Value '<label>' at 'versionLabel' failed to satisfy constraint: Member must satisfy regular expression pattern: .*[^0-9].*" — AWS reports the anchored/wrapped form ".*[^0-9].*", not the model's bare "[^0-9]". Fixed text: "1 validation error detected: Value '", "' at 'versionLabel' failed to satisfy constraint: Member must satisfy regular expression pattern: .*[^0-9].*"; the only variable part is the rejected label from the request.
Emulator: emits "... regular expression pattern: [^0-9]".
Why: the quoted pattern is what a caller is told to satisfy; the bare form reads as "the label must be exactly one non-digit character" rather than "must contain at least one non-digit", so a client correcting its input from the message can correct it wrongly.
Fix: the pattern-violation message rendering for VersionLabel in the appconfig validation layer.
Traces: appconfig.hosted-configuration-version-numeric-version-label-rejected.

Cause 9 of 10: a second StopDeployment(AllowRevert=true) on an already-REVERTED deployment says "cannot be stopped" instead of "cannot be reverted".
AWS: Smithy StopDeployment (DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, AllowRevert bound to the Allow-Revert header, success 202) models BadRequestException (400). Recorded behavior: the refusal message distinguishes the two verbs — a plain stop of a terminal deployment is "Deployment <n> for App <applicationId> and Environment <environmentId> cannot be stopped, it has a status of <STATE>." while a stop WITH AllowRevert=true is "Deployment <n> for App <applicationId> and Environment <environmentId> cannot be reverted, it has a status of <STATE>." Fixed text: "Deployment " / " for App " / " and Environment " / " cannot be stopped, it has a status of " or " cannot be reverted, it has a status of " / "."; variable parts: the deployment number, the two ids from the request, and the deployment's current state from state.
Emulator: emits the "cannot be stopped" form even when AllowRevert=true was supplied, so the revert path is indistinguishable from the stop path in the refusal.
Fix: the StopDeployment handler's terminal-state refusal in the appconfig service must select the verb from the Allow-Revert header.
Traces: appconfig.stop-deployment-allow-revert-reverts-completed-deployment.

Cause 10 of 10: ValidateConfiguration's malformed-content refusal embeds the wrong original error.
AWS: Smithy ValidateConfiguration (POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/validators, success 204) models BadRequestException (400). Recorded behavior: validating content that is not a JSON document against a JSON_SCHEMA validator is refused with the message "Could not build JSON schema from invalid input:\n <schema>.\n Original error message:\n[message: \"instance type (string) does not match any allowed primitive type (allowed: [\"object\"])\" - parent node: /]" — AWS feeds the raw content to the schema library and reports ITS diagnostic, i.e. a type mismatch against the schema's declared type. Fixed text: "Could not build JSON schema from invalid input:\n" / ".\n Original error message:\n[message: " / " - parent node: /]"; variable parts: the profile's schema and the library diagnostic naming the instance type seen and the types the schema allows.
Emulator: substitutes its own diagnostic "the configuration is not a JSON document" into that slot.
Why: the embedded diagnostic is the only thing that tells a caller WHY the document failed (which node, which types were allowed); replacing it with a generic sentence makes the refusal unactionable and unparseable.
Fix: the ValidateConfiguration handler's JSON_SCHEMA validation path in the appconfig service.
Related open issues (neither covers this check): 20261023T032202Z-appconfig-validateconfiguration-stub.md (the operation now answers) and 20261027T064453Z-appconfig-validateconfiguration-answers-204-for-a-configuration-version.md (a MISSING refusal; this cause is about the text of a refusal that does happen).
Traces: appconfig.validate-configuration-invalid-json-rejected.

## Evidence
Cause 1 — float rendering (5 checks):
  appconfig.deployment-strategy-invalid-growth-factor-rejected / appconfig.op-contract-createDeploymentStrategy: expected "1 validation error detected: Value '150.0' at 'growthFactor' failed to satisfy constraint: Member must have value less than or equal to 100.0", got "... Value '150' ... less than or equal to 100".
  appconfig.create-experiment-definition-invalid-treatment-weights-rejected: expected "Value '-5.0' at 'treatments.1.member.weight' ... greater than or equal to 0.0", got "Value '-5' ... greater than or equal to 0".
  appconfig.update-experiment-definition-invalid-schedule-rejected: expected "Value '-10.0' at 'treatments.1.member.weight' ... greater than or equal to 0.0", got "Value '-10' ... greater than or equal to 0".
  appconfig.update-experiment-run-malformed-input-rejected: expected "Value '150.0' at 'exposurePercentage' ... less than or equal to 100.0", got "Value '150' ... less than or equal to 100".
  (150/-5/-10 are the per-check inputs; the requirement is the DECIMAL rendering of a float member's value and bound.)

Cause 2 — StopExperimentRun error class (5 checks): expected Code 'BadRequestException' (HTTP 400); got Code 'ConflictException' (HTTP 409) with Message 'Run <run> of experiment definition <id> is DONE, so it cannot be stopped: only a running experiment can be stopped.' (recorded instance: 'Run 1 of experiment definition 5w5gjbo ...'). AWS's message for the same refusal: 'Experiment run <run> for experiment definition <id> is not running.'

Cause 3 — CreateExtension duplicate name: expected Code 'BadRequestException' with Message 'LatestVersionNumber not populated and an extension with name <name> already exists.'; got Code 'ConflictException'.

Cause 4 — GetConfiguration unknown Configuration id: expected Code 'BadRequestException' with Message 'Configuration Profile with Application Id <appId> and Configuration Profile Id <profileId> could not be found for account <account>.'; got Code 'ResourceNotFoundException'.

Cause 5 — CreateExtension stale lock: expected Message 'LatestVersionNumber supplied in request <supplied> does not match latest version <actual>.' (recorded instance: '... request 99 does not match latest version 1.'); got 'Extension <name> is at version 1, not 99.'

Cause 6 — DeleteExtension with associations: expected Message 'Cannot delete extension <id>, version <n>, for account <account>, as there are <count> associations of the extension still existing.' (recorded instance: '... extension ho65pb1, version 1, for account 000000000000, as there are 1 associations ...'); got 'Extension ho65pb1 version 1 still has extension associations; delete them before deleting the extension.'

Cause 7 — UpdateExtension unknown version: expected Message 'Extension could not be found for account <account>.'; got 'Extension with Id <id> and Version Number <n> could not be found for account <account>.' (recorded instance ids are per-run values).

Cause 8 — versionLabel pattern: expected "1 validation error detected: Value '<label>' at 'versionLabel' failed to satisfy constraint: Member must satisfy regular expression pattern: .*[^0-9].*"; got the same message ending in 'pattern: [^0-9]'.

Cause 9 — second AllowRevert stop: expected Message 'Deployment <n> for App <appId> and Environment <envId> cannot be reverted, it has a status of REVERTED.'; got '... cannot be stopped, it has a status of REVERTED.' (Code BadRequestException was correct.)

Cause 10 — ValidateConfiguration malformed content: expected the embedded diagnostic 'instance type (string) does not match any allowed primitive type (allowed: ["object"])'; got 'the configuration is not a JSON document' inside the otherwise-correct 'Could not build JSON schema from invalid input: ... Original error message:[message: "..." - parent node: /]' envelope.

Smithy support: CreateDeploymentStrategy.GrowthFactor -> GrowthFactor float @range 1.0-100.0; Weight -> float @range min 0.0; NullablePercentage -> float @range 0.0-100.0; StopExperimentRun errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404; CreateExtension errors BadRequestException 400 / ConflictException 409; GetConfiguration errors BadRequestException 400 / ResourceNotFoundException 404; DeleteExtension errors BadRequestException 400 / ResourceNotFoundException 404; UpdateExtension errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404; CreateHostedConfigurationVersion.VersionLabel @pattern "[^0-9]"; StopDeployment DELETE .../deployments/{DeploymentNumber} with AllowRevert in the Allow-Revert header, errors BadRequestException 400; ValidateConfiguration success 204, errors BadRequestException 400.

## Steps to Reproduce
Cause 1: replay appconfig.deployment-strategy-invalid-growth-factor-rejected (also op-contract-createDeploymentStrategy). Call CreateDeploymentStrategy with GrowthFactor=150. Expected: 400 BadRequestException, "1 validation error detected: Value '150.0' at 'growthFactor' failed to satisfy constraint: Member must have value less than or equal to 100.0". Observed: the same message with '150' and '100'. Repeat with CreateExperimentDefinition Treatments[1].Weight=-5 and UpdateExperimentRun ExposurePercentage=150 for the other member paths.

Cause 2: replay appconfig.stop-experiment-run-conflict-when-not-running (and the four siblings). CreateExperimentDefinition, StartExperimentRun, StopExperimentRun once, then StopExperimentRun again on the same run. Expected: 400 BadRequestException. Observed: 409 ConflictException.

Cause 3: replay appconfig.extension-duplicate-name-conflict-rejected. CreateExtension(Name=N, Actions=...), then CreateExtension again with the same Name and NO LatestVersionNumber. Expected: 400 BadRequestException with 'LatestVersionNumber not populated and an extension with name N already exists.'. Observed: ConflictException.

Cause 4: replay appconfig.op-contract-getConfiguration. Deploy a profile, then GetConfiguration with a same-shaped Configuration id that was never created. Expected: 400 BadRequestException with the 'Configuration Profile with Application Id ... could not be found for account ...' message. Observed: ResourceNotFoundException.

Cause 5: replay appconfig.create-extension-version-conflict-rejected. CreateExtension(Name=N) (version 1), then CreateExtension(Name=N, LatestVersionNumber=99). Expected ConflictException with 'LatestVersionNumber supplied in request 99 does not match latest version 1.'. Observed the emulator's own wording.

Cause 6: replay appconfig.delete-extension-with-active-association-rejected. Create an extension, CreateExtensionAssociation to it, then DeleteExtension. Compare the Message against the 'Cannot delete extension <id>, version <n>, for account <acct>, as there are <count> associations of the extension still existing.' template.

Cause 7: replay appconfig.update-extension-stale-version-conflict-rejected. Create an extension (version 1) and call UpdateExtension(VersionNumber=5). Expected ResourceNotFoundException with 'Extension could not be found for account <acct>.'.

Cause 8: replay appconfig.hosted-configuration-version-numeric-version-label-rejected. CreateHostedConfigurationVersion with VersionLabel='123456'. Compare the quoted pattern: '.*[^0-9].*' expected, '[^0-9]' served.

Cause 9: replay appconfig.stop-deployment-allow-revert-reverts-completed-deployment. Complete a deployment, StopDeployment(AllowRevert=True) so it becomes REVERTED, then call StopDeployment(AllowRevert=True) again. Expected 'cannot be reverted, it has a status of REVERTED.'; observed 'cannot be stopped, ...'.

Cause 10: replay appconfig.validate-configuration-invalid-json-rejected. Attach a JSON_SCHEMA validator requiring an object, store a hosted version whose content is not a JSON document, then ValidateConfiguration. Compare the embedded 'Original error message' diagnostic.

Re-replay all check ids listed under trace_ids to confirm the fix.

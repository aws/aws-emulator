Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted appconfig-lacs-traj-deprecated-getconfiguration-logical-ids-then-names
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StartDeployment|4xx->2xx|$.Message
Issue-Key: appconfig|StartDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|StartDeployment|4xx->2xx|status
Issue-Key: appconfig|ListDeployments|2xx->2xx|$.Items[*].ConfigurationVersion
Issue-Key: appconfig|ListDeployments|2xx->2xx|$.Items[*].VersionLabel
Issue-Key: appconfig|GetConfiguration|4xx->4xx|$.Message
Issue-Key: appconfig|GetConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appconfig|GetConfiguration|4xx->4xx|status
Issue-Key: appconfig|GetConfiguration|4xx->5xx|$.Message
Issue-Key: appconfig|GetConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: appconfig|GetConfiguration|4xx->5xx|status
Issue-Related: 20261103T021132Z-appconfig-deployment-lifecycle-advances-instantly-through-deploy-bake.md
Issue-Commit: eb95e56228ce55bd248c3581c6fd09ba0e6c8f48

# AppConfig request validation and refusal divergences on StartDeployment and GetConfiguration

> _Note: this issue was identified against the code base at commit `eb95e56228ce55bd248c3581c6fd09ba0e6c8f48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
This issue bundles 5 net-new causes that share one root cause family: AppConfig refusals are missing, misclassified or misworded on the deployment-start and deprecated-GetConfiguration paths — a bad ConfigurationVersion is accepted instead of refused (and then corrupts the deployment record a later list returns), a validation message renders the modelled pattern differently from AWS, and GetConfiguration raises the wrong error class / no error at all where AWS raises a specific BadRequestException.

Related open issues that do NOT own these clusters (the replay reports them unowned): 20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md (GetConfiguration answering 501) and 20261102T091627Z-appconfig-control-plane-getconfigurationprofile-reports-the-application-level.md (profile-level not-found message on GetConfigurationProfile).

Cause 1 of 5: StartDeployment accepts a ConfigurationVersion that does not exist.
  AWS: Smithy: StartDeployment POST .../deployments, required member ConfigurationVersion -> Version (@length 1..1024); errors include ResourceNotFoundException (httpError 404). Recorded behavior: deploying a version number that was never created is refused 404 with Message `Could not find configuration <configurationProfileName> with version number <version>` — fixed text `Could not find configuration ` and ` with version number `, with the configuration profile's name and the requested version number taken from the request.
  Emulator: answers 201 and CREATES the deployment; no Message and no x-amzn-errortype header are emitted.
  Why: a typo'd or stale version is silently deployed, so the client believes it rolled out content that does not exist; the environment is also left occupied by a deployment AWS would never have created.
  Fix: StartDeployment's validation — the ConfigurationVersion must be resolved against the profile's hosted-configuration versions before the deployment is created.
  Traces: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#8 (expected 404, got 201).

Cause 2 of 5: the deploymentStrategyId validation message renders the pattern with an extra anchor group.
  AWS: Smithy: StartDeployment.DeploymentStrategyId @pattern ^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$; errors include BadRequestException (httpError 400). Recorded behavior: a non-conforming value is refused 400 with Message `1 validation error detected: Value '<value>' at 'deploymentStrategyId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)` — i.e. AWS quotes the alternation group WITHOUT the outer `^(` / `)$` wrapper. Fixed text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `; the variable parts are the rejected value, the member name and the pattern as AWS renders it.
  Emulator: same 400 and same fixed text, but renders the pattern as `^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)` — an extra leading `^(` and no closing anchor.
  Why: the message is the requirement; a client (or a test) that matches on AWS's rendered constraint text, or that re-compiles the quoted pattern to explain the failure to a user, gets a different regex than AWS reports.
  Fix: the validation-message renderer for @pattern constraints in the appconfig input validation layer.
  Traces: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#9 (status matches at 400; only $.Message diverges).

Cause 3 of 5: ListDeployments returns the bogus version and drops VersionLabel.
  AWS: Smithy: ListDeployments GET .../deployments, code 200, output Deployments{Items -> DeploymentList}; DeploymentSummary carries ConfigurationVersion and VersionLabel. Recorded behavior: the listing's newest item reports the ConfigurationVersion that was actually deployed and the VersionLabel recorded for that hosted version.
  Emulator: reports the never-created version from Cause 1 as the item's ConfigurationVersion and omits VersionLabel entirely.
  Why: the listing is the client's audit view of what is deployed; it both names content that does not exist and loses the human-readable label used to identify a release.
  Fix: the ListDeployments response serializer must carry VersionLabel from the deployment record; the bogus version disappears once Cause 1 refuses it.
  Traces: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#13 (200 both sides; $.Items[0].ConfigurationVersion and $.Items[0].VersionLabel diverge).

Cause 4 of 5: GetConfiguration reports an application-level not-found instead of AWS's profile-level BadRequestException.
  AWS: Smithy: GetConfiguration GET /applications/{Application}/environments/{Environment}/configurations/{Configuration} (deprecated in favor of GetLatestConfiguration); errors BadRequestException (400), InternalServerException (500), ResourceNotFoundException (404). Recorded behavior: when the identifiers do not resolve, this deprecated path refuses 400 with Message `Configuration Profile with Application Id <application> and Configuration Profile Id <configuration> could not be found for account <accountId>.` — fixed text `Configuration Profile with Application Id `, ` and Configuration Profile Id `, ` could not be found for account `, `.`; the variable parts are the Application and Configuration labels from the request and the caller's account id.
  Emulator: answers 404 with x-amzn-errortype `ResourceNotFoundException` and the application-only Message `Application with Id <application> could not be found for account <accountId>.`
  Why: the status, the error code and the message are all different, so a client that branches on 400/BadRequestException for this deprecated path takes the wrong branch, and the message names only the application when AWS names both the application and the configuration profile.
  Fix: GetConfiguration's resolution/refusal path — the unresolved-identifier case must raise BadRequestException (400) with the profile-level message rather than falling through to the generic application-level ResourceNotFoundException.
  Traces: appconfig-lacs-traj-deprecated-getconfiguration-logical-ids-then-names#task#7 (expected 400, got 404; cluster also covers task#8).

Cause 5 of 5: GetConfiguration without client_id answers 501 instead of the modelled validation error.
  AWS: Smithy: GetConfiguration.ClientId is @required, bound @httpQuery('client_id'), target StringWithLengthBetween1And64 (@length 1..64); errors include BadRequestException (400). Recorded behavior: omitting the query parameter is refused 400 with Message `1 validation error detected: Value null at 'clientId' failed to satisfy constraint: Member must not be null` — fixed text, with `clientId` the member name from the model.
  Emulator: answers 501 with x-amzn-errortype `NotImplemented` and NO Message at all.
  Why: 501 is a server-side "operation does not exist" signal, so a client treats its own malformed request as an emulator/service outage and retries instead of fixing the missing parameter; SDK error handling for BadRequestException never runs.
  Fix: GetConfiguration's required-member validation must run before any not-implemented fallback in that handler.
  Traces: appconfig-lacs-traj-deprecated-getconfiguration-logical-ids-then-names#task#9 (expected 400, got 501).

## Evidence
Cause 1 — appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#8 StartDeployment: expected status 404, actual 201; $.Message missing (expected 'Could not find configuration <profileName> with version number 7'); $headers.x-amzn-errortype missing.
Cause 2 — same fixture task#9 StartDeployment (400 both sides): $.Message expected "1 validation error detected: Value '<value>' at 'deploymentStrategyId' failed to satisfy constraint: Member must satisfy regular expression pattern: (^[a-z0-9]{4,7}$|^AppConfig\\.[A-Za-z0-9]{9,40}$)", got the same text ending "...pattern: ^(^[a-z0-9]{4,7}$|^AppConfig\\.[A-Za-z0-9]{9,40}$)".
Cause 3 — same fixture task#13 ListDeployments (200 both sides): $.Items[0].ConfigurationVersion expected '1', got '7' (the version Cause 1 should have refused); $.Items[0].VersionLabel missing (expected the recorded version label).
Cause 4 — appconfig-lacs-traj-deprecated-getconfiguration-logical-ids-then-names#task#7 GetConfiguration: expected status 400, actual 404; $.Message expected 'Configuration Profile with Application Id <application> and Configuration Profile Id <configuration> could not be found for account <accountId>.', got 'Application with Id <application> could not be found for account <accountId>.'; $headers.x-amzn-errortype got 'ResourceNotFoundException' where the recording's error type is not the 404 class. Cluster also covers task#8.
Cause 5 — same fixture task#9 GetConfiguration: expected status 400, actual 501; $.Message missing (expected "1 validation error detected: Value null at 'clientId' failed to satisfy constraint: Member must not be null"); $headers.x-amzn-errortype got 'NotImplemented'.

## Steps to Reproduce
1. Create an application, environment, hosted configuration profile (one hosted version, with a VersionLabel) and a deployment strategy.
2. Call StartDeployment with ConfigurationVersion set to a number that was never created (e.g. 7). AWS refuses 404 'Could not find configuration <profileName> with version number 7'; the emulator answers 201 and creates the deployment.
3. Call StartDeployment with DeploymentStrategyId set to a value that violates the modelled pattern (e.g. a long hyphenated name). Both refuse 400, but compare the rendered 'regular expression pattern:' text — the emulator prefixes an extra '^('.
4. Call ListDeployments for that environment and compare Items[0].ConfigurationVersion and Items[0].VersionLabel: the emulator reports the never-created version and omits VersionLabel.
5. Call the deprecated GetConfiguration with an Application/Configuration pair that does not resolve. AWS refuses 400 BadRequestException naming both the application id and the configuration profile id; the emulator answers 404 ResourceNotFoundException naming only the application.
6. Call GetConfiguration omitting the client_id query parameter. AWS refuses 400 with the "Value null at 'clientId'" validation message; the emulator answers 501 NotImplemented with no message.
7. Replay appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted (task#8, task#9, task#13) and appconfig-lacs-traj-deprecated-getconfiguration-logical-ids-then-names (task#7, task#9).

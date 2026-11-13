Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.directory-data-access-server-enforced-toggle ds.remove-primary-region-denied ds.state-machine-trust-create-verify-delete-404
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.directory-data-access-server-enforced-toggle|_wait_data_access_status#21|Timeout|status = ds.describe_directory_data_access(DirectoryId=directory_id)["DataAccessStatus"]
Issue-Key: ds.remove-primary-region-denied|_wait_for_stage#13|Timeout|time.sleep(interval_s)
Issue-Key: ds.state-machine-trust-create-verify-delete-404|_wait_for_trust_state#20|Timeout|time.sleep(interval_s)
Issue-Commit: e9c942b44c1a5b5c93516f44dd907bbc1ec7900c

# ds: 3 net-new causes — the MSAD stage chain, the RemoveRegion/DeleteDirectory leg and the trust create/verify/delete chain each settle only AFTER the probe that follows the call, so three more ds scenarios reach no verdict at the 60 s check budget

> _Note: this issue was identified against the code base at commit `e9c942b44c1a5b5c93516f44dd907bbc1ec7900c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectories

## Description
Three ds behavioral checks reach NO verdict in this replay: each is killed with "Check timed out after 60.0s" while asleep in a poll loop. The emulator answers every request; what diverges is WHEN a clock-owed transition becomes observable. Each ds state transition the emulator owns comes due only AFTER the probe that immediately follows the call that started it, so a client that calls, probes once, then sleeps its interval pays one FULL poll interval per transition. Two or three such legs exhaust the suite's 60 s per-check ceiling before any assertion runs.

Related open issues that the replay report did NOT mark as OWNING these three checks, and which therefore do not cover them: 20261111T120514Z-ds-the-directory-data-access-disable-re-enable-c3f4bd.md, 20261110T232837Z-ds-the-directory-data-access-disable-re-enable.md, 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md and 20261111T085740Z-ds-the-ldaps-enable-disable-and-directory-data.md carry sibling check ids for the same mechanism on the LDAPS and ds-data toggles. These three causes extend the same mechanism to the DIRECTORY-STAGE chain, the DELETE leg and the TRUST state machine, which none of those issues names; all of them should be resolved as one work item.

--- Cause 1 of 3: the directory-data-access server-enforced toggle scenario never reaches its assertions (check id ds.directory-data-access-server-enforced-toggle).
AWS: the statuses are modelled — Smithy com.amazonaws.directoryservice#DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed]; DescribeDirectoryDataAccess input {DirectoryId @required, pattern ^d-[0-9a-f]{10}$} / output {DataAccessStatus}; EnableDirectoryDataAccess and DisableDirectoryDataAccess take the same input, return an empty result, and model DirectoryInDesiredStateException (client, httpError 400, members Message + RequestId) and DirectoryUnavailableException (400), i.e. AWS starts the transition and answers before it settles. For the wall clock there is recorded behavior only: on real AWS the CreateMicrosoftAD leg alone takes tens of minutes, so the 60 s per-check ceiling is the emulator environment's, not the recording's.
Emulator: no status, code, message or shape divergence is reported — the only output is "Check timed out after 60.0s". The scenario drives four clock-owed legs and probes each immediately after the call that starts it: CreateMicrosoftAD then poll DescribeDirectories for Stage 'Active' at 30 s; EnableDirectoryDataAccess then poll DescribeDirectoryDataAccess for 'Enabled' at 10 s; DisableDirectoryDataAccess then poll for 'Disabled' at 10 s; EnableDirectoryDataAccess again then poll for 'Enabled' at 10 s. Because every one of those transitions lands after the first probe, 60 s of sleeping is owed inside a 60 s ceiling.
Why: a client gets no verdict on the contract this scenario exists to pin — that the data-access gate is SERVER-side and authoritative: DescribeDirectoryDataAccess reports 'Disabled' for a freshly created directory, Enable drives it to 'Enabled', a SECOND EnableDirectoryDataAccess on an already-enabled directory is refused with HTTP 400, Error.Code 'DirectoryInDesiredStateException' and leaves the status exactly 'Enabled' (no double transition, no silent success), and the gate survives a full disable/re-enable round trip. In that refusal the fixed text is AWS's sentence for the already-in-desired-state class, 'Directory <id> is already in the desired state. : RequestId: <id>' — the directory id is taken from the request and the RequestId from the response; neither the id nor the RequestId of any single run is part of the requirement. None of this is exercised today, so a regression in any of it passes unnoticed.

--- Cause 2 of 3: the RemoveRegion-on-the-primary-Region refusal plus its DeleteDirectory control never reach their assertions (check id ds.remove-primary-region-denied).
AWS: Smithy RemoveRegion input {DirectoryId @required, pattern ^d-[0-9a-f]{10}$}, output {}, errors AccessDeniedException(400), ClientException(400), DirectoryDoesNotExistException(400), DirectoryUnavailableException(400), ServiceException(500), UnsupportedOperationException(400, members Message + RequestId, doc 'The operation is not supported.'). DeleteDirectory input {DirectoryId @required}, output {DirectoryId}, errors ClientException / EntityDoesNotExistException / ServiceException. DirectoryStage = [Requested, Creating, Created, Active, Inoperable, Impaired, Restoring, RestoreFailed, Deleting, Deleted, Failed, Updating] — so the Deleting -> Deleted walk is modelled as an observable chain. Per AWS's RemoveRegion documentation the primary Region cannot be removed by RemoveRegion; it goes away only with the whole directory.
Emulator: again no response divergence is reported, only "Check timed out after 60.0s". Two clock-owed legs at 30 s each — CreateMicrosoftAD then poll DescribeDirectories for 'Active' at 30 s, and DeleteDirectory then poll for 'Deleted' at 30 s — are exactly the 60 s ceiling, so the check dies in the second sleep.
Why: a client gets no verdict on a refusal AND on the documented alternative. Unverified are: that RemoveRegion against a Standard-edition Managed Microsoft AD is refused with HTTP 400 and Error.Code 'UnsupportedOperationException' whose message is the edition sentence AWS words for this class, 'Multi-region replication is only supported for AWS Managed Microsoft AD (Enterprise edition) directories. : RequestId: <id>' (all fixed text except the RequestId, which is the response's); that the refused RemoveRegion leaves the directory's Stage unchanged and NOT in Deleting/Deleted; and that DeleteDirectory, the documented way to retire the primary Region, does drive the directory to Stage 'Deleted'. An emulator that silently accepted RemoveRegion on a primary Region, or that never reached 'Deleted', would pass today.

--- Cause 3 of 3: the two-way trust create/verify/delete state machine never reaches its assertions (check id ds.state-machine-trust-create-verify-delete-404).
AWS: Smithy TrustState = [Creating, Created, Verifying, VerifyFailed, Verified, Updating, UpdateFailed, Updated, Deleting, Deleted, Failed]; CreateTrust output {TrustId, pattern ^t-[0-9a-f]{10}$} with errors incl. EntityAlreadyExistsException / EntityDoesNotExistException; VerifyTrust input {TrustId @required} / output {TrustId}; DeleteTrust input {TrustId @required, DeleteAssociatedConditionalForwarder default false} / output {TrustId}; DescribeTrusts output {Trusts, NextToken}, errors incl. EntityDoesNotExistException (client, httpError 400, members Message + RequestId). So AWS answers each trust call before the trust settles and exposes the walk through DescribeTrusts.
Emulator: no response divergence reported, only "Check timed out after 60.0s". The scenario owes FIVE clock-owed legs, each probed immediately after its call: two CreateMicrosoftAD waits for Stage 'Active' at 30 s each, a wait for the trust to reach Created/Verified at 15 s, a wait after VerifyTrust for 'Verified' at 15 s, and a wait after DeleteTrust for DescribeTrusts to refuse the trust at 10 s — 100 s owed inside a 60 s ceiling.
Why: a client gets no verdict on the entire trust state machine: that CreateTrust mints a t-<10 hex> TrustId and the trust leaves Creating for 'Created'; that VerifyTrust moves it to 'Verified'; and that after DeleteTrust, DescribeTrusts(DirectoryId, TrustIds=[id]) eventually refuses the gone trust with HTTP 400, Error.Code 'EntityDoesNotExistException' and AWS's wording for that class, 'Trust: <trustId> does not exist : RequestId: <id>' — the fixed text is 'Trust: ' / ' does not exist : RequestId: ', the trust id comes from the request and the RequestId from the response; no run's literal values belong in the template. A trust that never changed state, or a deleted trust that kept reading back, would pass today.

Fix: these are not operation-handler response defects — no ds handler answers wrongly in any of the three. The owner is the ds state machine's clock-owed-transition contract as it meets the suite's per-check ceiling: either the transition spans the ds state applies to the directory-stage chain (Requested/Creating -> Active, Deleting -> Deleted), the data-access toggles (Enabling/Disabling) and the trust state machine (Creating -> Created, Verifying -> Verified, Deleting -> gone), so that a settled value is observable on the client's FIRST probe after the call — accepting that the transient values and the refusals that depend on them (DirectoryUnavailableException, DirectoryInDesiredStateException) stop being observable and that in-tree suites pinning them must be loosened — or the harness's per-check budget and poll intervals for these scenarios.

Traces: ds.directory-data-access-server-enforced-toggle, ds.remove-primary-region-denied, ds.state-machine-trust-create-verify-delete-404 (all verdict 'mismatch', all with detail "Check timed out after 60.0s", all killed inside a poll helper's time.sleep(interval_s)).

## Evidence
Failing checks (behavioral suite simparity-behavioral-aws-v1, all listed under service heading '### ds'):

  [mismatch] ds.directory-data-access-server-enforced-toggle
    Error: Check timed out after 60.0s
  [mismatch] ds.remove-primary-region-denied
    Error: Check timed out after 60.0s
  [mismatch] ds.state-machine-trust-create-verify-delete-404
    Error: Check timed out after 60.0s

None of the three is marked ALREADY FILED or OWNED in this replay.

Expected (AWS / the checks):
  1. directory-data-access-server-enforced-toggle — DescribeDirectoryDataAccess on a fresh Managed Microsoft AD reports DataAccessStatus 'Disabled'; EnableDirectoryDataAccess settles it on 'Enabled'; a second EnableDirectoryDataAccess is refused HTTP 400 / Error.Code 'DirectoryInDesiredStateException' with message 'Directory <directoryId> is already in the desired state. : RequestId: <requestId>' and the status stays 'Enabled'; DisableDirectoryDataAccess settles it on 'Disabled'; a further enable settles it on 'Enabled' again.
  2. remove-primary-region-denied — RemoveRegion on a Standard-edition directory's own primary Region is refused HTTP 400 / Error.Code 'UnsupportedOperationException' with message 'Multi-region replication is only supported for AWS Managed Microsoft AD (Enterprise edition) directories. : RequestId: <requestId>'; the directory's Stage is unchanged (not Deleting/Deleted); DeleteDirectory then drives it to Stage 'Deleted'.
  3. state-machine-trust-create-verify-delete-404 — CreateTrust returns a TrustId matching ^t-[0-9a-f]{10}$, the trust reaches TrustState 'Created' (or 'Verified'); VerifyTrust drives it to 'Verified'; after DeleteTrust, DescribeTrusts(DirectoryId=..., TrustIds=[trustId]) refuses with HTTP 400 / Error.Code 'EntityDoesNotExistException' and message 'Trust: <trustId> does not exist : RequestId: <requestId>'.

Actual (emulator): not one of those assertions is reached. Each check is killed at the 60 s ceiling inside a poll sleep. The replay reports NO status, error-code, message or shape divergence for any of the three — only the timeout.

Why the ceiling cannot be met, from the checks' own intervals (each poll probes immediately after the call that starts the transition, observes a transient, then sleeps one interval):
  ds.directory-data-access-server-enforced-toggle
    create_microsoft_ad           -> _wait_directory_active, interval 30 s      -> 30 s
    enable_directory_data_access  -> _wait_data_access_status 'Enabled',  10 s  -> 10 s
    disable_directory_data_access -> _wait_data_access_status 'Disabled', 10 s  -> 10 s
    enable_directory_data_access  -> _wait_data_access_status 'Enabled',  10 s  -> 10 s
                                                                        total     60 s
  ds.remove-primary-region-denied
    create_microsoft_ad -> _wait_for_stage {'Active'},  interval 30 s -> 30 s
    delete_directory    -> _wait_for_stage {'Deleted'}, interval 30 s -> 30 s
                                                        total           60 s
  ds.state-machine-trust-create-verify-delete-404
    create_microsoft_ad x2 -> _wait_directory_stage {'Active'}, interval 30 s each -> 60 s
    create_trust           -> _wait_for_trust_state {'Created','Verified'}, 15 s  -> 15 s
    verify_trust           -> _wait_for_trust_state {'Verified'},           15 s  -> 15 s
    delete_trust           -> _wait_for_trust_not_found,                    10 s  -> 10 s
                                                                            total  100 s
all inside a 60 s per-check ceiling, plus the sts/ec2/ds request traffic each scenario also issues.

Smithy consulted (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json):
  DirectoryStage = [Requested, Creating, Created, Active, Inoperable, Impaired, Restoring, RestoreFailed, Deleting, Deleted, Failed, Updating]
  DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed]
  TrustState = [Creating, Created, Verifying, VerifyFailed, Verified, Updating, UpdateFailed, Updated, Deleting, Deleted, Failed]
  EnableDirectoryDataAccess / DisableDirectoryDataAccess: input {DirectoryId @required, pattern ^d-[0-9a-f]{10}$}, output {}; errors incl. DirectoryInDesiredStateException(400), DirectoryUnavailableException(400)
  DescribeDirectoryDataAccess: output {DataAccessStatus}
  RemoveRegion: input {DirectoryId @required}, output {}; errors AccessDeniedException(400), ClientException(400), DirectoryDoesNotExistException(400), DirectoryUnavailableException(400), ServiceException(500), UnsupportedOperationException(400)
  DeleteDirectory: input {DirectoryId @required}, output {DirectoryId}; errors ClientException(400), EntityDoesNotExistException(400), ServiceException(500)
  CreateTrust: output {TrustId, pattern ^t-[0-9a-f]{10}$}; VerifyTrust/DeleteTrust: input {TrustId @required}, output {TrustId}; DescribeTrusts: output {Trusts, NextToken}, errors incl. EntityDoesNotExistException(400)
  UnsupportedOperationException / DirectoryInDesiredStateException / EntityDoesNotExistException: client errors, httpError 400, members {Message, RequestId}

Corroborating replay context: sibling ds checks that owe only ONE or TWO clock-owed legs do reach their assertions in this same run and fail on response content instead of the clock, which locates the overrun in the number of clock-owed legs each scenario adds rather than in slow request service.

## Steps to Reproduce
1. Boot the emulator and create botocore clients for 'sts', 'ec2' and 'ds' against it.
2. Replay ds.directory-data-access-server-enforced-toggle, ds.remove-primary-region-denied and ds.state-machine-trust-create-verify-delete-404. Each is killed at the 60 s budget inside a poll sleep, with no status/code/message divergence reported.
3. Reproduce the arithmetic by hand, timing each leg. In a VPC with two subnets in distinct AZs: call create_microsoft_ad, then poll describe_directories at 30 s intervals until Stage == 'Active' — observe that the FIRST probe lands on a transient stage and the settled 'Active' only becomes visible after the sleep, costing a full interval. Repeat for delete_directory -> Stage 'Deleted' at 30 s; for enable/disable_directory_data_access -> describe_directory_data_access at 10 s; and for create_trust / verify_trust / delete_trust -> describe_trusts at 15 s / 15 s / 10 s. Sum the owed intervals: 60 s, 60 s and 100 s respectively, against a 60 s ceiling.
4. Confirm the responses themselves are not the defect: walk the same sequences with the polling replaced by a one-second sleep after each call and check each assertion directly —
   (a) describe_directory_data_access on a fresh directory == 'Disabled'; enable_directory_data_access then 'Enabled'; a SECOND enable_directory_data_access must answer HTTP 400, Code 'DirectoryInDesiredStateException', Message 'Directory <directoryId> is already in the desired state. : RequestId: <requestId>', leaving the status 'Enabled'; disable then 'Disabled'; enable again then 'Enabled'.
   (b) remove_region on the Standard-edition directory must answer HTTP 400, Code 'UnsupportedOperationException', Message 'Multi-region replication is only supported for AWS Managed Microsoft AD (Enterprise edition) directories. : RequestId: <requestId>'; describe_directories must still report a Stage that is neither 'Deleting' nor 'Deleted'; delete_directory must then reach Stage 'Deleted'.
   (c) create_trust (Two-Way, with the matching remote-side trust and both conditional forwarders in place) must return a TrustId matching ^t-[0-9a-f]{10}$ and reach TrustState 'Created' or 'Verified'; verify_trust must reach 'Verified'; after delete_trust, describe_trusts(DirectoryId=..., TrustIds=[trustId]) must answer HTTP 400, Code 'EntityDoesNotExistException', Message 'Trust: <trustId> does not exist : RequestId: <requestId>'.
5. Resolve at the level that owns it — either the clock-owed transition spans the ds state machine applies to the directory-stage chain, the data-access toggles and the trust state machine (so a settled value is observable on the client's FIRST probe, accepting that the transient values and the DirectoryUnavailableException / DirectoryInDesiredStateException paths that depend on them stop being observable, and loosening the in-tree suites that pin them), or these checks' budgets and poll intervals.
6. Re-replay all three check ids and confirm each reaches a verdict on every assertion in step 4 rather than dying in a sleep. Resolve together with 20261111T120514Z-ds-the-directory-data-access-disable-re-enable-c3f4bd.md, 20261110T232837Z-ds-the-directory-data-access-disable-re-enable.md, 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md and 20261111T085740Z-ds-the-ldaps-enable-disable-and-directory-data.md, which carry sibling check ids for the same mechanism.

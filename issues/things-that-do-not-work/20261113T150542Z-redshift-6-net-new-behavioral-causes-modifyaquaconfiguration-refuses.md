Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.aqua-configuration-modify-input-ignored redshift.aqua-configuration-modify-transition redshift.cluster-db-revision-already-latest-blocked redshift.cluster-db-revision-already-latest-fault redshift.cluster-db-revision-modify-transition redshift.modify-cluster-db-revision-already-latest-rejected redshift.modify-cluster-db-revision-latest-blocked redshift.lakehouse-configuration-idc-application-not-exists-rejected redshift.modify-lakehouse-configuration-dependent-service-errors redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected
Issue-Key: redshift|ModifyAquaConfiguration|InternalFailure
Issue-Key: redshift.cluster-db-revision-already-latest-blocked|run#9|AssertionError|assert message == _ON_LATEST, f"expected {_ON_LATEST!r}, got {message!r}"
Issue-Key: redshift.cluster-db-revision-already-latest-fault|run#15|AssertionError|assert message == _ON_LATEST, f"expected {_ON_LATEST!r}, got {message!r}"
Issue-Key: redshift.cluster-db-revision-modify-transition|run#12|AssertionError|assert message == _ON_LATEST, f"expected {_ON_LATEST!r}, got {message!r}"
Issue-Key: redshift.modify-cluster-db-revision-already-latest-rejected|run#10|AssertionError|assert message == (
Issue-Key: redshift.modify-cluster-db-revision-latest-blocked|run#13|AssertionError|assert message == (
Issue-Key: redshift.lakehouse-configuration-idc-application-not-exists-rejected|run#17|AssertionError|assert message == (
Issue-Key: redshift.modify-lakehouse-configuration-dependent-service-errors|_wait_until#19|Timeout|time.sleep(interval_s)
Issue-Key: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected|_run_sql#26|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# redshift: 6 net-new behavioral causes — ModifyAquaConfiguration refuses every call with InternalFailure, ClusterOnLatestRevision/RedshiftIdcApplicationNotExists wording, LakehouseRegistrationStatus casing, DryRun returns 200, no stv_blocklist catalog

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyAquaConfiguration, ModifyClusterDbRevision, ModifyLakehouseConfiguration, ExecuteStatement

## Description
Replay of simparity-behavioral-aws-v1 against the emulated Redshift control plane and cluster data plane produced these failures. This bundle carries SIX causes, all in the redshift service; ALL SIX must be closed before submitting. Checks the suite marked ALREADY FILED or OWNED were skipped and are not represented here. Related but NOT covering any cause below: the open issues 20261111T163827Z-redshift-3-net-new-causes-modifyclusterdbrevision-answers-501.md (ModifyClusterDbRevision answering 501 on OTHER checks — the five checks in Cause 2 now get the right code and only the message diverges) and 20261029T193810Z-redshift-cluster-sql-engine-multi-statement-sql-refused.md (its Cause 3 names the missing pg_user/stv_blocklist catalog; the check in Cause 6 was NOT marked OWNED by it because it now fails at a different point — it is re-grounded here so it is re-replayed).

=== Cause 1 of 6: ModifyAquaConfiguration refuses EVERY request with InternalFailure, including one naming an existing, available cluster ===
AWS: Smithy: ModifyAquaConfiguration takes ClusterIdentifier (required) + AquaConfigurationStatus and returns ModifyAquaOutputMessage{AquaConfiguration}, HTTP 200; its modeled errors are ClusterNotFoundFault/InvalidClusterStateFault/UnsupportedOperationFault, and AquaConfiguration's members are documented "This field is retired. Amazon Redshift automatically determines whether to use AQUA". Recorded behavior: for a cluster identifier that does not exist the call is refused with error code InternalFailure and message "An internal error has occurred. Please try your query again at a later time." (fixed text, no variable part); for an EXISTING available cluster the call SUCCEEDS with 200 and reports an AquaConfiguration whose AquaConfigurationStatus is the service-determined value — the retired requested value is ignored, so asking for "enabled" and then "disabled" yields the same reported status, and a following DescribeClusters reports the same status on the cluster.
Emulator: the handler reads and writes nothing and refuses unconditionally, so every ModifyAquaConfiguration — existing cluster or not — comes back as InternalFailure with that same message (botocore retries it as a 5xx and then raises "(InternalFailure) ... (reached max retries: 3)"). There is no success path at all, and no AquaConfiguration is ever reported.
Why: a client that calls the retired-but-still-answered operation on a real cluster gets a retried server failure instead of a 200, so it cannot read the cluster's AQUA state at all; the only behavior it can observe is the one AWS reserves for a nonexistent cluster, which makes the operation's not-found signal indistinguishable from its success path.
Fix: the ModifyAquaConfiguration handler — resolve the cluster (keeping the recorded InternalFailure for an identifier the account does not hold), then return the cluster's service-determined AquaConfiguration without honouring the requested AquaConfigurationStatus, and report the same value through the cluster record DescribeClusters reads.
Traces: redshift.aqua-configuration-modify-input-ignored, redshift.aqua-configuration-modify-transition; handler services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/modify_aqua_configuration.rs:159-174 (unconditional Scaffolded::internal_failure).

=== Cause 2 of 6: the ClusterOnLatestRevision refusal message is the emulator's own sentence, not AWS's ===
AWS: Smithy: ModifyClusterDbRevision models ClusterOnLatestRevisionFault, awsQueryError code "ClusterOnLatestRevision", HTTP 400, documented "Cluster is already on the latest database revision." Recorded behavior: a ModifyClusterDbRevision whose RevisionTarget is the cluster's own CurrentDatabaseRevision is refused 400 ClusterOnLatestRevision with the message "Modification of cluster-db-revision was skipped. The cluster is already on the requested db-revision." — the whole sentence is FIXED text, with NO variable part (no cluster identifier, no revision value).
Emulator: the code and status are right, but the message is "Cluster <cluster-identifier> is already on the latest database revision." — different wording, and it interpolates the requested ClusterIdentifier into a message AWS words without one.
Why: this error's message is its only payload beyond the code; a client (or an operator runbook/log matcher) that matches AWS's sentence to tell "already on the requested revision" from other 400s never matches, and a message carrying a cluster id where AWS carries none also breaks exact-match assertions.
Fix: the ModifyClusterDbRevision refusal wording — crate::state::cluster_on_latest_revision_message must return the fixed AWS sentence and take no identifier (called from services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/modify_cluster_db_revision.rs:245-253).
Traces: redshift.cluster-db-revision-already-latest-blocked, redshift.cluster-db-revision-already-latest-fault, redshift.cluster-db-revision-modify-transition, redshift.modify-cluster-db-revision-already-latest-rejected, redshift.modify-cluster-db-revision-latest-blocked.

=== Cause 3 of 6: the RedshiftIdcApplicationNotExists message drops AWS's ", or the status isn't Completed." clause ===
AWS: Smithy: RedshiftIdcApplicationNotExistsFault, awsQueryError code "RedshiftIdcApplicationNotExists", HTTP 404, documented "The application you attempted to find doesn't exist." Recorded behavior for ModifyLakehouseConfiguration(LakehouseIdcRegistration="Associate", LakehouseIdcApplicationArn=<arn naming no application>): 404 RedshiftIdcApplicationNotExists with the message "The RedshiftIdcApplication with the ARN <arn> wasn't found, or the status isn't Completed." — fixed text "The RedshiftIdcApplication with the ARN " + the ARN taken from the request + " wasn't found, or the status isn't Completed."
Emulator: same code and status, but the message stops early: "The RedshiftIdcApplication with the ARN <arn> wasn't found." The trailing ", or the status isn't Completed" clause — which is what tells a caller the ARN may exist but not be usable yet — is missing.
Why: a client that distinguishes "no such IdC application" from "application not yet Completed" reads that clause; without it the refusal looks like a hard not-found and a caller that would otherwise retry while the application finishes provisioning gives up (and exact-message matchers fail).
Fix: the shared refusal wording crate::state::redshift_idc_application_not_exists (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/state.rs:1899-1901), used by services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/modify_lakehouse_configuration.rs:265-278.
Traces: redshift.lakehouse-configuration-idc-application-not-exists-rejected, redshift.modify-lakehouse-configuration-dependent-service-errors.

=== Cause 4 of 6: Cluster$LakehouseRegistrationStatus is reported in the wrong vocabulary ("Registered"/"Deregistered" instead of AWS's upper-case REGISTERED/DEREGISTERED), so a client polling for the terminal status never finishes ===
AWS: Smithy: Cluster$LakehouseRegistrationStatus / LakehouseConfiguration$LakehouseRegistrationStatus is "The current status of the lakehouse registration" and the model publishes no enum, so this is recorded behavior: after ModifyLakehouseConfiguration(LakehouseRegistration="Register") the status moves asynchronously through REGISTRATION_REQUESTED and settles on REGISTERED (upper case), and the deregistered counterpart is likewise upper case; a client waits for the terminal REGISTERED before issuing the next lakehouse call.
Emulator: ModifyLakehouseConfiguration stores "Registered" / "Deregistered" (title case) and both the operation output and DescribeClusters report those spellings, so a wait for status == "REGISTERED" never completes. The check consequently hangs in its poll loop and is reported as "Check timed out after 60.0s".
Why: the status string is a client-visible state token. Every poll loop or conditional written against AWS's value spins forever (and the check's own teardown, which deregisters only a cluster reporting REGISTERED, never runs), so a correct client hangs rather than merely mis-reading one field.
Fix: the lakehouse state vocabulary — crate::state::lakehouse LAKEHOUSE_REGISTRATION_REGISTERED / LAKEHOUSE_REGISTRATION_DEREGISTERED (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/state/lakehouse.rs:52-56), written by services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/modify_lakehouse_configuration.rs:293-302.
Traces: redshift.modify-lakehouse-configuration-dependent-service-errors.

=== Cause 5 of 6: ModifyLakehouseConfiguration with DryRun=true answers 200 instead of the DryRunOperation refusal ===
AWS: Smithy: ModifyLakehouseConfigurationMessage$DryRun is "A boolean value that, if true, validates the request without actually modifying the lakehouse configuration. Use this to check for errors before making changes." The model declares no DryRunOperation error, so this is recorded behavior: a dry run that WOULD have succeeded is signalled EC2-style by refusing the call with error code DryRunOperation and the message "Request would have succeeded, but DryRun flag is set." (fixed text, no variable part) rather than by returning a response body.
Emulator: after running the preconditions the handler returns 200 with the cluster's current lakehouse configuration, so no DryRunOperation error is ever raised; a caller using DryRun as a would-succeed probe sees a normal success and cannot tell a validated dry run from a real modification.
Why: DryRun is how a client checks a lakehouse change before applying it. Because the signal is an error and not a 200, every such client treats the emulator's 200 as "not a dry run / unsupported" or, worse, as a change having been applied; the two failing checks use the dry run as their positive control and can never pass while it returns 200.
Fix: the ModifyLakehouseConfiguration handler's DryRun branch (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/modify_lakehouse_configuration.rs:284-286) — once every precondition has passed, refuse with error code DryRunOperation and AWS's fixed message instead of returning the configuration.
Traces: redshift.lakehouse-configuration-idc-application-not-exists-rejected, redshift.modify-lakehouse-configuration-dependent-service-errors (both assert this after the message faults above, so Causes 3/4 must be fixed for the assertion to be reached).

=== Cause 6 of 6: the cluster SQL engine has no stv_blocklist system table, so a statement reading it lands FAILED with SQLite's wording ===
AWS: recorded behavior only (the Smithy model describes no SQL surface). A Redshift database exposes the Redshift system tables, so an INSERT ... SELECT <const> FROM stv_blocklist LIMIT 5 submitted through redshift-data on a provisioned cluster reaches DescribeStatement Status FINISHED and commits its five rows.
Emulator: the statement goes terminal Status='FAILED' with Error='no such table: stv_blocklist' — the relation does not exist in the embedded engine, and the diagnostic is the embedded engine's text rather than Redshift's "ERROR: relation ... does not exist" form.
Why: the stv_*/stl_*/pg_* relations are how clients introspect a cluster; any query touching them fails outright, and here it defeats the positive control proving an uncancelled statement on the same cluster still commits (the cancel semantics themselves now pass).
Fix: the cluster SQL engine's catalog component behind ExecuteStatement — back the Redshift system tables (stv_blocklist among them) with the emulator's own cluster state so they are selectable. This is the same root cause as Cause 3 of the open issue 20261029T193810Z-redshift-cluster-sql-engine-multi-statement-sql-refused.md, which names stv_blocklist; this check was not marked OWNED by it because its failure now lands on a different assertion (the control INSERT, not the STARTED poll), so it is re-grounded here and must be re-replayed.
Traces: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — Total 139/244 passed (57.0%); redshift 99/187 (53%), 88 failed, of which all but the ten grounded here were marked ALREADY FILED or OWNED and skipped.

Cause 1 — redshift.aqua-configuration-modify-input-ignored / redshift.aqua-configuration-modify-transition
  expected: modify_aqua_configuration(ClusterIdentifier=<existing available cluster>, AquaConfigurationStatus="enabled") -> HTTP 200 with AquaConfiguration{AquaConfigurationStatus=<service-determined>}, identical for a following "disabled" request, and the same value from describe_clusters
  actual:   botocore.exceptions.ClientError: An error occurred (InternalFailure) when calling the ModifyAquaConfiguration operation (reached max retries: 3): An internal error has occurred. Please try your query again at a later time.
  (the SAME refusal AWS gives only for a nonexistent cluster; the handler refuses before looking at any record)

Cause 2 — five checks, one assertion
  expected: Error.Code == "ClusterOnLatestRevision" (HTTP 400) and Error.Message == "Modification of cluster-db-revision was skipped. The cluster is already on the requested db-revision."
  actual:   Error.Code == "ClusterOnLatestRevision" but Error.Message == "Cluster <cluster-id> is already on the latest database revision."
  (observed with <cluster-id> = sim2real-behav-dbrev-00ab8a8bf9b5, sim2real-dbrev-113ea302602e, sim2real-clu-328e6c52cb4e, sim2real-dbrev-cl-d034d22730a6, sim2real-dbrev-4124c0b9969c — per-run identifiers; the requirement is the fixed AWS sentence, which contains no identifier at all)

Cause 3 — redshift.lakehouse-configuration-idc-application-not-exists-rejected
  expected: RedshiftIdcApplicationNotExists (HTTP 404), message "The RedshiftIdcApplication with the ARN <arn> wasn't found, or the status isn't Completed."
  actual:   AssertionError: unexpected RedshiftIdcApplicationNotExists message "The RedshiftIdcApplication with the ARN arn:aws:redshift:<region>:<account>:redshiftidcapplication:<name> wasn't found."
  (the ARN is the one from the request; the divergence is the missing ", or the status isn't Completed." clause)

Cause 4 — redshift.modify-lakehouse-configuration-dependent-service-errors
  expected: after ModifyLakehouseConfiguration(LakehouseRegistration="Register"), describe_clusters(...)["Clusters"][0]["LakehouseRegistrationStatus"] settles to "REGISTERED"
  actual:   Check timed out after 60.0s while polling for "REGISTERED"; the emulator stores and reports "Registered" (src/state/lakehouse.rs:52 LAKEHOUSE_REGISTRATION_REGISTERED = "Registered", :56 ..._DEREGISTERED = "Deregistered")

Cause 5 — same two lakehouse checks
  expected: modify_lakehouse_configuration(ClusterIdentifier=<cluster>, DryRun=True, ...) raises ClientError with Code "DryRunOperation" and Message "Request would have succeeded, but DryRun flag is set."
  actual:   the handler returns 200 with the cluster's current lakehouse configuration (src/ops/modify_lakehouse_configuration.rs:284-286), so the check's _refusal helper would raise "expected a ClientError, call succeeded"

Cause 6 — redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected
  expected: ExecuteStatement(Sql="INSERT INTO <table> SELECT 1 FROM stv_blocklist LIMIT 5") -> DescribeStatement Status FINISHED, five rows committed
  actual:   AssertionError: DATAPLANE_UNIMPLEMENTED: SQL 'INSERT INTO sim2real_rdapi_uncancelled SELECT 1 FROM stv_blocklist LIMIT 5' did not FINISH: status='FAILED' error='no such table: stv_blocklist'

(cluster identifiers, ARNs, account ids, regions and table names above are per-run examples; the requirements are the status codes, error codes, fixed message texts and state vocabularies.)

Model corroboration (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json): ModifyAquaConfiguration -> ModifyAquaOutputMessage{AquaConfiguration}, errors ClusterNotFoundFault/InvalidClusterStateFault/UnsupportedOperationFault; AquaConfiguration members documented "This field is retired. Amazon Redshift automatically determines whether to use AQUA"; ClusterOnLatestRevisionFault awsQueryError code ClusterOnLatestRevision, httpError 400; RedshiftIdcApplicationNotExistsFault awsQueryError code RedshiftIdcApplicationNotExists, httpError 404; ModifyLakehouseConfigurationMessage$DryRun "validates the request without actually modifying the lakehouse configuration"; Cluster$LakehouseRegistrationStatus has no modeled enum.

## Steps to Reproduce
0. git clone the repo, build and boot the emulator, and point botocore redshift / redshift-data clients at it. Create a cluster subnet group over the run's network and CreateCluster a single-node RA3 cluster; wait for ClusterStatus "available".
1. Cause 1: redshift.modify_aqua_configuration(ClusterIdentifier=<that cluster>, AquaConfigurationStatus="enabled") -> expect 200 with AquaConfiguration; observe InternalFailure ("An internal error has occurred. Please try your query again at a later time.") after botocore's retries. Repeat with "disabled" and compare with describe_clusters(...)["Clusters"][0]["AquaConfiguration"].
2. Cause 2: current = redshift.describe_cluster_db_revisions(ClusterIdentifier=<cluster>)["ClusterDbRevisions"][0]["CurrentDatabaseRevision"]; redshift.modify_cluster_db_revision(ClusterIdentifier=<cluster>, RevisionTarget=current) -> expect 400 ClusterOnLatestRevision with "Modification of cluster-db-revision was skipped. The cluster is already on the requested db-revision."; observe "Cluster <id> is already on the latest database revision."
3. Cause 4: redshift.modify_lakehouse_configuration(ClusterIdentifier=<cluster>, LakehouseRegistration="Register", CatalogName=<cluster>) then poll describe_clusters for LakehouseRegistrationStatus -> expect it to settle on "REGISTERED"; observe "Registered" (and "Deregistered" after a Deregister).
4. Cause 3: redshift.modify_lakehouse_configuration(ClusterIdentifier=<cluster>, LakehouseIdcRegistration="Associate", LakehouseIdcApplicationArn="arn:aws:redshift:<region>:<account>:redshiftidcapplication:does-not-exist") -> expect 404 RedshiftIdcApplicationNotExists with "... wasn't found, or the status isn't Completed."; observe the message ending at "wasn't found."
5. Cause 5: redshift.modify_lakehouse_configuration(ClusterIdentifier=<cluster>, DryRun=True) -> expect a ClientError DryRunOperation "Request would have succeeded, but DryRun flag is set."; observe a 200 response body.
6. Cause 6: through redshift-data on the same cluster, execute_statement(Sql="CREATE TABLE t (n INT)") then execute_statement(Sql="INSERT INTO t SELECT 1 FROM stv_blocklist LIMIT 5") and poll describe_statement -> expect FINISHED; observe FAILED with Error 'no such table: stv_blocklist'.
7. Re-replay every check id in trace_ids to confirm the fix.

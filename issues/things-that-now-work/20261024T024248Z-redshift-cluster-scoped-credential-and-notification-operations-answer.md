Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.cluster-scoped-ops-not-found-semantics redshift.delete-cluster-lifecycle redshift.resume-non-paused-cluster-fails redshift.get-cluster-credentials-with-iam-returns-usable-credentials redshift.identity-center-auth-token-ttl-expiry redshift.e2e-idc-application-auth-token-flow redshift.failover-primary-compute-unauthorized-blocked redshift.failover-primary-compute-unauthorized-denied redshift.aqua-configuration-modify-input-ignored redshift.enable-logging-insufficient-bucket-policy-denied redshift.event-subscription-missing-topic-fails redshift.snapshot-scoped-ops-not-found-semantics redshift.endpoint-access-ops-not-found-semantics redshift.datashare-ops-not-found-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.cluster-scoped-ops-not-found-semantics|run#6|AssertionError|assert observed == expected, f"cluster not-found contract drift:\n expected={expected}\n observed={observed}"
Issue-Key: redshift.delete-cluster-lifecycle|run#29|AssertionError|assert reboot_code == "ClusterNotFound", (
Issue-Key: redshift.resume-non-paused-cluster-fails|run#7|AssertionError|assert code == "InvalidClusterState", f"expected InvalidClusterState, got {code}"
Issue-Key: redshift.get-cluster-credentials-with-iam-returns-usable-credentials|run#4|AssertionError|assert code == "ClusterNotFound", f"expected ClusterNotFound for a nonexistent cluster, got {code!r}"
Issue-Key: redshift.identity-center-auth-token-ttl-expiry|run#8|AssertionError|assert code == "UnsupportedOperation", (
Issue-Key: redshift.e2e-idc-application-auth-token-flow|run#4|AssertionError|assert code_token == "UnsupportedOperation", (
Issue-Key: redshift.failover-primary-compute-unauthorized-blocked|run#6|AssertionError|assert control_code == "ClusterNotFound", (
Issue-Key: redshift.failover-primary-compute-unauthorized-denied|run#4|AssertionError|assert control_code == "ClusterNotFound", (
Issue-Key: redshift.aqua-configuration-modify-input-ignored|run#4|AssertionError|assert code == "InternalFailure", (
Issue-Key: redshift.enable-logging-insufficient-bucket-policy-denied|run#7|AssertionError|assert code == "InsufficientS3BucketPolicyFault", code
Issue-Key: redshift.event-subscription-missing-topic-fails|run#11|AssertionError|assert code == "SNSTopicArnNotFound", f"expected SNSTopicArnNotFound, got {code}"
Issue-Key: redshift.snapshot-scoped-ops-not-found-semantics|run#8|AssertionError|assert observed == expected, f"snapshot not-found contract drift:\n expected={expected}\n observed={observed}"
Issue-Key: redshift.endpoint-access-ops-not-found-semantics|run#11|AssertionError|assert observed == expected, f"endpoint-access contract drift:\n expected={expected}\n observed={observed}"
Issue-Key: redshift.datashare-ops-not-found-semantics|run#10|AssertionError|assert observed == expected, f"datashare contract drift:\n expected={expected}\n observed={observed}"
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Redshift cluster-scoped, credential and notification operations answer NotImplemented instead of their modeled errors (10 root causes)

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootCluster

## Description
Ten distinct root causes, all in the redshift service, all with the same shape: the operation's handler is a scaffold that returns the generic `NotImplemented` error instead of performing the operation or raising the error AWS models for the request. Each cause is a different handler/operation family and must be closed separately.

Cause 1 of 10: cluster-scoped operations do not resolve the cluster and do not raise ClusterNotFound.
AWS: every cluster-scoped operation looks the ClusterIdentifier up first and, when it does not exist, fails with ClusterNotFoundFault. Smithy (com-amazonaws-redshift-redshiftserviceversion20121201): ResumeCluster/ModifyAquaConfiguration/FailoverPrimaryCompute/EnableLogging/GetClusterCredentialsWithIAM all list error ClusterNotFoundFault, errorKind client, httpError 404, awsQueryError code `ClusterNotFound`, 404. The same holds for RebootCluster, PauseCluster, ResizeCluster, CancelResize, DescribeResize, RotateEncryptionKey, ModifyClusterMaintenance, ModifyClusterIamRoles, ModifyClusterSnapshotSchedule, DescribeClusterDbRevisions, DescribeLoggingStatus, DisableLogging, GetClusterCredentials, DescribeTableRestoreStatus, CreateUsageLimit.
Emulator: for a nonexistent cluster identifier only DescribeClusters, DeleteCluster and ModifyCluster answer `ClusterNotFound`; the other 17 operations in the probe answer error code `NotImplemented` with message "operation <X> is scaffolded but not implemented in this emulator".
Why: a client that branches on ClusterNotFound to decide "create it" vs "it exists" cannot branch at all, and SDK waiters/retry classifiers treat the unmodelled NotImplemented code as a non-retryable unknown failure instead of a 404. Lifecycle code that deletes a cluster then re-probes it with a mutation never sees the refusal AWS gives.
Fix: the per-operation handlers in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/ for the listed operations — resolve the cluster from service state and raise ClusterNotFoundFault (awsQuery code `ClusterNotFound`, HTTP 404) before any other work.
Traces: redshift.cluster-scoped-ops-not-found-semantics, redshift.delete-cluster-lifecycle.

Cause 2 of 10: ResumeCluster does not enforce cluster state.
AWS: resuming a cluster that is not paused fails with InvalidClusterStateFault. Smithy: ResumeCluster errors include InvalidClusterStateFault, httpError 400, awsQueryError code `InvalidClusterState`, 400.
Emulator: returns `NotImplemented`.
Why: a caller cannot distinguish "already running" from "the call is broken", so resume-if-paused logic loops or aborts.
Fix: the ResumeCluster handler and its state transition in the redshift service's cluster state.
Traces: redshift.resume-non-paused-cluster-fails.

Cause 3 of 10: GetClusterCredentialsWithIAM is not implemented.
AWS: returns DbUser, DbPassword (sensitive), Expiration and NextRefreshTime for a live cluster, and fails with ClusterNotFoundFault (`ClusterNotFound`, 404) for an unknown cluster or UnsupportedOperationFault (`UnsupportedOperation`, 400) when the caller's credential is not IAM-Identity-Center-enhanced. Smithy: GetClusterCredentialsWithIAM output = {DbUser, DbPassword, Expiration, NextRefreshTime}; errors = ClusterNotFoundFault, UnsupportedOperationFault.
Emulator: every call answers `NotImplemented` — for the not-found probe, for the non-enhanced-credential probe, and for the happy path.
Why: no client can obtain temporary cluster credentials, and the negative legs that distinguish "wrong credential type" from "no such cluster" both collapse to one unmodelled code.
Fix: the GetClusterCredentialsWithIAM handler plus its credential-minting/expiry state in the redshift service.
Traces: redshift.get-cluster-credentials-with-iam-returns-usable-credentials, redshift.identity-center-auth-token-ttl-expiry, redshift.e2e-idc-application-auth-token-flow.

Cause 4 of 10: FailoverPrimaryCompute is not implemented.
AWS: Smithy: FailoverPrimaryCompute errors = ClusterNotFoundFault (`ClusterNotFound`, 404), InvalidClusterStateFault (`InvalidClusterState`, 400), UnauthorizedOperation (`UnauthorizedOperation`, 400), UnsupportedOperationFault. A fully privileged call against a nonexistent cluster must reach business logic and fail ClusterNotFound; an unauthorized one must fail at authorization.
Emulator: both the authorized and the unauthorized probe answer `NotImplemented`, so authorization and business logic are indistinguishable.
Why: a caller cannot tell a permissions problem from a missing cluster; Multi-AZ failover automation has no working path.
Fix: the FailoverPrimaryCompute handler and its authorization check.
Traces: redshift.failover-primary-compute-unauthorized-blocked, redshift.failover-primary-compute-unauthorized-denied.

Cause 5 of 10: ModifyAquaConfiguration is not implemented.
AWS: the operation is retained but inert; for a nonexistent cluster the recorded response is an `InternalFailure` (recorded behavior only — Smithy models ClusterNotFoundFault/InvalidClusterStateFault/UnsupportedOperationFault for ModifyAquaConfiguration and does not document the InternalFailure, so take the status and code from the recording).
Emulator: `NotImplemented`.
Why: clients that still call the AQUA modify path get an unmodelled code instead of the error AWS actually returns.
Fix: the ModifyAquaConfiguration handler in the redshift ops directory.
Traces: redshift.aqua-configuration-modify-input-ignored.

Cause 6 of 10: EnableLogging performs no S3 bucket-policy validation.
AWS: enabling audit logging to a bucket whose policy does not grant Redshift delivery fails with InsufficientS3BucketPolicyFault. Smithy: EnableLogging errors include InsufficientS3BucketPolicyFault, httpError 400, awsQueryError code `InsufficientS3BucketPolicyFault`, alongside BucketNotFoundFault, InvalidS3BucketNameFault, InvalidS3KeyPrefixFault, ClusterNotFoundFault and InvalidClusterStateFault.
Emulator: `NotImplemented`.
Why: a caller that fixes the bucket policy and retries can never observe the transition from refusal to success, and misconfigured logging is never reported.
Fix: the EnableLogging handler plus its bucket-policy validation against the S3 service state.
Traces: redshift.enable-logging-insufficient-bucket-policy-denied.

Cause 7 of 10: CreateEventSubscription does not validate the SNS topic.
AWS: a subscription naming an SNS topic ARN that does not exist fails with SNSTopicArnNotFoundFault. Smithy: CreateEventSubscription errors include SNSTopicArnNotFoundFault, httpError 404, awsQueryError code `SNSTopicArnNotFound`, 404 (also SNSInvalidTopic, SNSNoAuthorization, SubscriptionCategoryNotFound, SubscriptionSeverityNotFound, SourceNotFound).
Emulator: `NotImplemented`.
Why: event-subscription wiring reports no error for a typo'd topic and no success either, so no notification path can be built or diagnosed.
Fix: the CreateEventSubscription handler and its SNS-topic resolution. Note: the open scaffold-stub issue filed for `CreateEventSubscription` covers standing the operation up; this cause names the validation contract the stood-up handler must satisfy.
Traces: redshift.event-subscription-missing-topic-fails.

Cause 8 of 10: snapshot-scoped operations do not resolve the snapshot and do not raise ClusterSnapshotNotFound.
AWS: for an unknown snapshot identifier DeleteClusterSnapshot, ModifyClusterSnapshot, CopyClusterSnapshot, AuthorizeSnapshotAccess, RevokeSnapshotAccess and DescribeNodeConfigurationOptions all fail `ClusterSnapshotNotFound`, and RestoreTableFromClusterSnapshot fails `ClusterNotFound` (recorded behavior; the modeled fault shapes are ClusterSnapshotNotFoundFault / ClusterNotFoundFault).
Emulator: only DescribeClusterSnapshots and RestoreFromClusterSnapshot answer `ClusterSnapshotNotFound`; the other seven answer `NotImplemented`.
Why: snapshot bookkeeping cannot distinguish a deleted snapshot from a broken call, so idempotent cleanup and restore flows break.
Fix: the snapshot-scoped handlers in the redshift ops directory and the snapshot registry they look up.
Traces: redshift.snapshot-scoped-ops-not-found-semantics.

Cause 9 of 10: the endpoint-access (Redshift-managed VPC endpoint) operation family is not implemented.
AWS: for a nonexistent endpoint DescribeEndpointAccess, DeleteEndpointAccess and ModifyEndpointAccess fail `EndpointNotFound`; CreateEndpointAccess with an unknown subnet group fails `ClusterSubnetGroupNotFoundFault`; AuthorizeEndpointAccess, RevokeEndpointAccess and DescribeEndpointAuthorization against an unknown cluster fail `ClusterNotFound` (recorded behavior, consistent with the modeled EndpointNotFoundFault / ClusterSubnetGroupNotFoundFault / ClusterNotFoundFault shapes).
Emulator: all seven answer `NotImplemented`.
Why: private-connectivity setup has no working path and no distinguishable failures.
Fix: the endpoint-access handlers and an endpoint registry in the redshift service state.
Traces: redshift.endpoint-access-ops-not-found-semantics.

Cause 10 of 10: the datashare operation family is not implemented.
AWS: for an unknown datashare ARN DescribeDataShares, AuthorizeDataShare, DeauthorizeDataShare, AssociateDataShareConsumer, DisassociateDataShareConsumer and RejectDataShare fail `InvalidDataShareFault`; DescribeDataSharesForConsumer and DescribeDataSharesForProducer with an unknown namespace fail `InvalidNamespaceFault` (recorded behavior, matching the modeled InvalidDataShareFault / InvalidNamespaceFault shapes).
Emulator: all eight answer `NotImplemented`.
Why: data-sharing clients get an unmodelled code for every call, so neither the happy path nor the negative classes can be exercised.
Fix: the datashare handlers and a datashare/namespace registry in the redshift service state.
Traces: redshift.datashare-ops-not-found-semantics.

## Evidence
Cause 1 — redshift.cluster-scoped-ops-not-found-semantics: AssertionError: cluster not-found contract drift:
expected={'DescribeClusters': 'ClusterNotFound', 'DeleteCluster': 'ClusterNotFound', 'ModifyCluster': 'ClusterNotFound', 'RebootCluster': 'ClusterNotFound', 'PauseCluster': 'ClusterNotFound', 'ResumeCluster': 'ClusterNotFound', 'ResizeCluster': 'ClusterNotFound', 'CancelResize': 'ClusterNotFound', 'DescribeResize': 'ClusterNotFound', 'RotateEncryptionKey': 'ClusterNotFound', 'ModifyClusterMaintenance': 'ClusterNotFound', 'ModifyClusterIamRoles': 'ClusterNotFound', 'ModifyClusterSnapshotSchedule': 'ClusterNotFound', 'DescribeClusterDbRevisions': 'ClusterNotFound', 'DescribeLoggingStatus': 'ClusterNotFound', 'DisableLogging': 'ClusterNotFound', 'GetClusterCredentials': 'ClusterNotFound', 'GetClusterCredentialsWithIAM': 'ClusterNotFound', 'DescribeTableRestoreStatus': 'ClusterNotFound', 'CreateUsageLimit': 'ClusterNotFound'}
observed={'DescribeClusters': 'ClusterNotFound', 'DeleteCluster': 'ClusterNotFound', 'ModifyCluster': 'ClusterNotFound', 'RebootCluster': 'NotImplemented', 'PauseCluster': 'NotImplemented', 'ResumeCluster': 'NotImplemented', 'ResizeCluster': 'NotImplemented', 'CancelResize': 'NotImplemented', 'DescribeResize': 'NotImplemented', 'RotateEncryptionKey': 'NotImplemented', 'ModifyClusterMaintenance': 'NotImplemented', 'ModifyClusterIamRoles': 'NotImplemented', 'ModifyClusterSnapshotSchedule': 'NotImplemented', 'DescribeClusterDbRevisions': 'NotImplemented', 'DescribeLoggingStatus': 'NotImplemented', 'DisableLogging': 'NotImplemented', 'GetClusterCredentials': 'NotImplemented', ...}

Cause 1 — redshift.delete-cluster-lifecycle: AssertionError: a mutation against the deleted cluster id must also be refused, got NotImplemented

Cause 2 — redshift.resume-non-paused-cluster-fails: AssertionError: expected InvalidClusterState, got NotImplemented

Cause 3 — redshift.get-cluster-credentials-with-iam-returns-usable-credentials: AssertionError: expected ClusterNotFound for a nonexistent cluster, got 'NotImplemented'
Cause 3 — redshift.identity-center-auth-token-ttl-expiry: AssertionError: expected UnsupportedOperation for a non IAM-Identity-Center-enhanced credential, got 'NotImplemented'
Cause 3 — redshift.e2e-idc-application-auth-token-flow: AssertionError: expected a non-enhanced-credential caller to be rejected, got 'NotImplemented'

Cause 4 — redshift.failover-primary-compute-unauthorized-blocked: AssertionError: expected ClusterNotFound for the authorized control call, got NotImplemented
Cause 4 — redshift.failover-primary-compute-unauthorized-denied: AssertionError: expected the fully-privileged control call to reach business logic with ClusterNotFound, got 'NotImplemented'

Cause 5 — redshift.aqua-configuration-modify-input-ignored: AssertionError: expected InternalFailure for a nonexistent cluster, got 'NotImplemented'

Cause 6 — redshift.enable-logging-insufficient-bucket-policy-denied: assert code == "InsufficientS3BucketPolicyFault" -> AssertionError: NotImplemented

Cause 7 — redshift.event-subscription-missing-topic-fails: AssertionError: expected SNSTopicArnNotFound, got NotImplemented

Cause 8 — redshift.snapshot-scoped-ops-not-found-semantics: AssertionError: snapshot not-found contract drift:
expected={'DescribeClusterSnapshots': 'ClusterSnapshotNotFound', 'DeleteClusterSnapshot': 'ClusterSnapshotNotFound', 'ModifyClusterSnapshot': 'ClusterSnapshotNotFound', 'CopyClusterSnapshot': 'ClusterSnapshotNotFound', 'AuthorizeSnapshotAccess': 'ClusterSnapshotNotFound', 'RevokeSnapshotAccess': 'ClusterSnapshotNotFound', 'RestoreFromClusterSnapshot': 'ClusterSnapshotNotFound', 'DescribeNodeConfigurationOptions': 'ClusterSnapshotNotFound', 'RestoreTableFromClusterSnapshot': 'ClusterNotFound'}
observed={'DescribeClusterSnapshots': 'ClusterSnapshotNotFound', 'DeleteClusterSnapshot': 'NotImplemented', 'ModifyClusterSnapshot': 'NotImplemented', 'CopyClusterSnapshot': 'NotImplemented', 'AuthorizeSnapshotAccess': 'NotImplemented', 'RevokeSnapshotAccess': 'NotImplemented', 'RestoreFromClusterSnapshot': 'ClusterSnapshotNotFound', 'DescribeNodeConfigurationOptions': 'NotImplemented', 'RestoreTableFromClusterSnapshot': 'NotImplemented'}

Cause 9 — redshift.endpoint-access-ops-not-found-semantics: AssertionError: endpoint-access contract drift:
expected={'DescribeEndpointAccess': 'EndpointNotFound', 'DeleteEndpointAccess': 'EndpointNotFound', 'ModifyEndpointAccess': 'EndpointNotFound', 'CreateEndpointAccess': 'ClusterSubnetGroupNotFoundFault', 'AuthorizeEndpointAccess': 'ClusterNotFound', 'RevokeEndpointAccess': 'ClusterNotFound', 'DescribeEndpointAuthorization': 'ClusterNotFound'}
observed={all seven: 'NotImplemented'}

Cause 10 — redshift.datashare-ops-not-found-semantics: AssertionError: datashare contract drift:
expected={'DescribeDataShares': 'InvalidDataShareFault', 'DescribeDataSharesForConsumer': 'InvalidNamespaceFault', 'DescribeDataSharesForProducer': 'InvalidNamespaceFault', 'AuthorizeDataShare': 'InvalidDataShareFault', 'DeauthorizeDataShare': 'InvalidDataShareFault', 'AssociateDataShareConsumer': 'InvalidDataShareFault', 'DisassociateDataShareConsumer': 'InvalidDataShareFault', 'RejectDataShare': 'InvalidDataShareFault'}
observed={all eight: 'NotImplemented'}

In every case the emulator's message is "operation <Operation> is scaffolded but not implemented in this emulator" under error code `NotImplemented`.

## Steps to Reproduce
1. Boot the emulator and point a redshift client at it.
2. Cause 1: call RebootCluster / PauseCluster / ResizeCluster / RotateEncryptionKey / DescribeLoggingStatus / GetClusterCredentials / CreateUsageLimit (etc.) with a ClusterIdentifier that does not exist. Expect error code ClusterNotFound, HTTP 404; observe NotImplemented.
3. Cause 2: create a cluster, wait for `available`, call ResumeCluster. Expect InvalidClusterState, HTTP 400.
4. Cause 3: call GetClusterCredentialsWithIAM for an unknown cluster (expect ClusterNotFound 404) and with a non-IAM-Identity-Center credential (expect UnsupportedOperation 400); then call it against a live cluster and expect DbUser/DbPassword/Expiration.
5. Cause 4: call FailoverPrimaryCompute against an unknown cluster with full privileges (expect ClusterNotFound 404) and with insufficient privileges (expect UnauthorizedOperation 400).
6. Cause 5: call ModifyAquaConfiguration with an unknown ClusterIdentifier.
7. Cause 6: create a bucket whose policy does not allow Redshift log delivery, then EnableLogging against an available cluster. Expect InsufficientS3BucketPolicyFault 400.
8. Cause 7: call CreateEventSubscription with an SnsTopicArn that does not exist. Expect SNSTopicArnNotFound 404.
9. Cause 8: call DeleteClusterSnapshot / ModifyClusterSnapshot / CopyClusterSnapshot / AuthorizeSnapshotAccess / RevokeSnapshotAccess / DescribeNodeConfigurationOptions with an unknown snapshot identifier (expect ClusterSnapshotNotFound) and RestoreTableFromClusterSnapshot with an unknown cluster (expect ClusterNotFound).
10. Cause 9: call the seven endpoint-access operations with unknown endpoint / subnet group / cluster names.
11. Cause 10: call the eight datashare operations with an unknown datashare ARN / namespace.
12. Replay the trace_ids checks to confirm.

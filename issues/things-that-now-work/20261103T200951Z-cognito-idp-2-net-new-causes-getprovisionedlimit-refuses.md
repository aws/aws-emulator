Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.update-provisioned-limit-concurrent-writes-conflict cognito-idp.user-pool-replica-lifecycle cognito-idp.user-pool-replica-update-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|GetProvisionedLimit|InvalidParameterException
Issue-Key: cognito-idp.user-pool-replica-lifecycle|run#16|AssertionError|assert status == "INACTIVE", (
Issue-Key: cognito-idp.user-pool-replica-update-lifecycle|_poll_until#19|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last value={value!r}")
Issue-Commit: 8b4952d5bd6ac92656117b5e6e80986c41b691fd

# cognito-idp: 2 net-new causes — GetProvisionedLimit refuses the API_CATEGORY category UserResourceRead as unknown, and the user-pool replica lifecycle cannot be driven to completion

> _Note: this issue was identified against the code base at commit `8b4952d5bd6ac92656117b5e6e80986c41b691fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProvisionedLimit

## Description
Two net-new root causes in cognito-idp. Both must be closed before submitting.

Cause 1 of 2: GetProvisionedLimit refuses the API_CATEGORY category `UserResourceRead` as an unknown category instead of reporting that category's limit.
AWS: Smithy — GetProvisionedLimit takes a required `LimitDefinition` (LimitDefinitionType) whose `LimitClass` is `API_CATEGORY` and whose `Attributes` map carries the `Category` key ("For API rate limits, specify the `Category` key with a value like `UserAuthentication` or `UserCreation`"); `Attributes` is a StringToStringMapType with no modeled enumeration of the category values, and the output is a required `Limit` (LimitType) carrying `LimitDefinition`, `ProvisionedLimitValue` and `FreeLimitValue`, both required. Recorded behavior only for the category catalogue itself: the recording reads `{LimitClass: API_CATEGORY, Attributes: {Category: UserResourceRead}}` and gets a `Limit` back with a ProvisionedLimitValue and a FreeLimitValue, so `UserResourceRead` is a category AWS publishes and answers for. InvalidParameterException (HTTP 400) is modeled on this operation, but it is the answer for a definition that identifies no limit, not for a published category.
Emulator: HTTP 400 `InvalidParameterException` with message `Unknown API_CATEGORY category: UserResourceRead` (fixed text `Unknown API_CATEGORY category: ` plus the request's `Attributes.Category` value — here `UserResourceRead`, which is a value from the request, not part of the template). The emulator's category catalogue therefore omits a category AWS publishes, and the operation never reaches the account's limit state at all.
Why: a caller cannot read the current provisioned rate for that category, which is the precondition for every UpdateProvisionedLimit call: the whole provisioned-capacity flow (read current value, attempt an update, read it back) aborts at the first call with a 400 instead of proceeding. Any client that enumerates or adjusts API-category rate limits is broken for this category.
Related but NOT covering this: open issue 20261029T182116Z-cognito-idp-provisioned-limit-api-diverges-no-default.md describes GetProvisionedLimit answering ResourceNotFoundException ("There is no limit for API category UserResourceRead in this account.") for a valid category and an invalid category being misclassified as not-found. The refusal has since changed class to InvalidParameterException, so the call is now refused with a different status/code/message — a valid category is refused as syntactically unknown — and that open issue does not describe this behavior.
Fix: the API-category catalogue and default-limit seeding behind `crate::state::api_category` / `api_category_limit`, which GetProvisionedLimit (and UpdateProvisionedLimit) resolve the definition through; the published categories must include `UserResourceRead` and every category must have a default (free) provisioned limit to report.
Traces: cognito-idp.update-provisioned-limit-concurrent-writes-conflict.

Cause 2 of 2: the multi-Region user-pool replica lifecycle cannot be driven to completion — the replica status is never readable as INACTIVE after creation, and a deleted replica is still reported DELETING at the point the check gives up.
AWS: Smithy — CreateUserPoolReplica returns a `UserPoolReplica` (UserPoolReplicaType) whose `Status` is a ReplicaStatusType (CREATING | ACTIVE | INACTIVE | DELETING); a freshly created replica settles INACTIVE, UpdateUserPoolReplica (Status: ACTIVE | INACTIVE) moves it between ACTIVE and INACTIVE, and DeleteUserPoolReplica removes it so ListUserPoolReplicas no longer lists that Region and UpdateUserPoolReplica on it answers ResourceNotFoundException "Replica not found in the requested region". Recorded behavior only for the transition durations: AWS takes minutes, which is why the checks poll.
Emulator: in `user-pool-replica-lifecycle` the create succeeds and ListUserPoolReplicas lists the replica Region once, but the very next read of that replica's status yields no status at all (`None` — the Region is not in the listing) where INACTIVE is due; in `user-pool-replica-update-lifecycle` the CREATING/ACTIVE/INACTIVE legs are served but after DeleteUserPoolReplica the replica is still reported `DELETING` and is never removed from ListUserPoolReplicas within the check's remaining budget. In both cases the lifecycle legs ran out of the check's wall-clock budget (the first poll was given 0.0 s), so the end-to-end replica lifecycle is never observable.
Hypothesis: the emulator stores a replica with a 500 ms activation span and a 500 ms deletion span (REPLICA_ACTIVATION_SPAN / REPLICA_DELETION_SPAN in src/state.rs), so the transitions themselves are fast; what consumes the budget is the shared prerequisite both checks serve before the lifecycle — the multi-Region CMK (KMS CreateKey with MultiRegion, ReplicateKey into the second Region, then DescribeKey polled until KeyState Enabled) plus CreateUserPool with KeyConfiguration and CreateUserPoolReplica. Every individual response observed in those legs was well-formed, so the divergence is that serving them leaves no budget for the lifecycle, not a wrong field value.
Why: a client cannot observe a replica settling INACTIVE after CreateUserPoolReplica, nor observe DeleteUserPoolReplica actually removing it, so multi-Region user-pool replication cannot be exercised end-to-end against the emulator even though each call answers 200.
Fix: the replica state machine and its listing — `UserPool::replica_regions` / `UserPool::live_replica` / `UserPool::to_replica_in` and the replica spans in src/state.rs, read by src/ops/list_user_pool_replicas.rs and written by src/ops/create_user_pool_replica.rs and src/ops/delete_user_pool_replica.rs — together with the latency of the multi-Region-key prerequisite those handlers depend on.
Traces: cognito-idp.user-pool-replica-lifecycle, cognito-idp.user-pool-replica-update-lifecycle.

## Evidence
Cause 1 — verbatim from the replay of cognito-idp.update-provisioned-limit-concurrent-writes-conflict:
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the GetProvisionedLimit operation: Unknown API_CATEGORY category: UserResourceRead
Expected: HTTP 200 with Limit = {LimitDefinition: {LimitClass: API_CATEGORY, Attributes: {Category: UserResourceRead}}, ProvisionedLimitValue: <account's current rate>, FreeLimitValue: <free rate>}.
Actual: HTTP 400, code InvalidParameterException, message 'Unknown API_CATEGORY category: UserResourceRead' (fixed text 'Unknown API_CATEGORY category: ' + the request's Attributes.Category).
Handler path: src/ops/get_provisioned_limit.rs:165-166 resolves the definition through crate::state::api_category and maps any failure to InvalidParameterException before the account state is read.

Cause 2 — verbatim from the replay:
  cognito-idp.user-pool-replica-lifecycle: AssertionError: expected a freshly created replica to settle INACTIVE, got None
    (raised at check_cognito_idp_user_pool_replica_lifecycle.py:78, after create_user_pool_replica succeeded and ListUserPoolReplicas had listed the replica Region once)
  cognito-idp.user-pool-replica-update-lifecycle: AssertionError: condition not met within 0.0s, last value='DELETING'
    (raised at check_cognito_idp_user_pool_replica_update_lifecycle.py:100, the post-DeleteUserPoolReplica poll for the replica to disappear from ListUserPoolReplicas)
Expected: ListUserPoolReplicas reports Status INACTIVE for a freshly created replica, ACTIVE after UpdateUserPoolReplica(Status=ACTIVE), INACTIVE after UpdateUserPoolReplica(Status=INACTIVE), and no entry for the Region after DeleteUserPoolReplica, with UpdateUserPoolReplica on the gone replica answering ResourceNotFoundException 'Replica not found in the requested region'.
Actual: no status at all for the replica Region on the read after creation, and a replica still DELETING with no budget left to see it removed.

## Steps to Reproduce
Cause 1:
1. Call GetProvisionedLimit with LimitDefinition={'LimitClass': 'API_CATEGORY', 'Attributes': {'Category': 'UserResourceRead'}}.
2. Observe HTTP 400 InvalidParameterException 'Unknown API_CATEGORY category: UserResourceRead' instead of a 200 carrying that category's ProvisionedLimitValue and FreeLimitValue.
3. Repeat for the other published API categories to confirm which are missing from the catalogue; then UpdateProvisionedLimit for the same definition and confirm it can read the value back.

Cause 2:
1. Create a multi-Region KMS CMK, ReplicateKey it into a second Region and wait for the replica key to report KeyState Enabled.
2. CreateUserPool with UserPoolTier=PLUS and KeyConfiguration={'KeyType': 'CUSTOMER_MANAGED_KEY', 'KmsKeyArn': <that key>}.
3. CreateUserPoolReplica(UserPoolId, RegionName=<second Region>), then poll ListUserPoolReplicas: the Region must appear and settle Status=INACTIVE.
4. UpdateUserPoolReplica(Status='ACTIVE') then ('INACTIVE'), confirming each on an independent ListUserPoolReplicas read.
5. DeleteUserPoolReplica, then poll ListUserPoolReplicas until the Region is gone, and confirm UpdateUserPoolReplica on it answers ResourceNotFoundException 'Replica not found in the requested region'. Time each leg: the lifecycle must be observable without the prerequisite legs consuming the run's whole budget.

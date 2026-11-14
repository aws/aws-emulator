Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.absent-condition-key-denied-unless-ifexists-forallvalues-or-null iam.account-summary-reflects-resource-counts iam.account-summary-server-side-aggregation-reflects-resource-counts iam.set-security-token-service-preferences iam.sts-preferences-update-persists iam.account-password-policy-lifecycle iam.delete-account-password-policy iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected iam.credential-report-ready-then-expires iam.credential-report-ttl-expiry-requires-regeneration iam.generate-credential-report iam.get-credential-report iam.get-outbound-web-identity-federation-info iam.get-outbound-web-identity-federation-info-contract iam.outbound-web-identity-federation-toggle iam.webidentity-federation-double-toggle-rejected iam.policy-empty-statement-array iam.policy-invalid-effect iam.policy-missing-action iam.reject-identity-policy-containing-principal iam.reject-trust-policy-without-principal iam.create-service-linked-role-unsupported-service-rejected iam.iam-access-key-crud iam.user-access-key-round-trips-status-and-never-used iam.service-specific-credential-long-term-api-key-shape iam.role-session-duration-and-description-bounds-rejected iam.role-permissions-boundary-lifecycle iam.list-attached-role-policies-denied-without-permission iam.condition-key-flips-a-real-authorization-decision iam.iam-eval-deep-iam-1-date-less-than-deadline-passes-then-denied iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct iam.iam-eval-deep-iam-1-omitted-request-tag-operators-diverge-denied iam.iam-eval-deep-iam-1-tag-keys-set-operator-swap-inverts-denied iam.op-contract-listRoles-missing-permission-denied iam.policy-simulation-explicit-deny-overrides-allow iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetAccountSummary|InvalidAction
Issue-Key: iam|GetAccountPasswordPolicy|InvalidAction
Issue-Key: iam|GenerateCredentialReport|InvalidAction
Issue-Key: iam.get-outbound-web-identity-federation-info|run#3|AssertionError|raise AssertionError(f"unexpected error before enabling the feature: {disabled_code}")
Issue-Key: iam|GetOutboundWebIdentityFederationInfo|InvalidAction
Issue-Key: iam|EnableOutboundWebIdentityFederation|InvalidAction
Issue-Key: iam.policy-empty-statement-array|run#10|AssertionError|assert code == ERR_MALFORMED_POLICY, f"Expected {ERR_MALFORMED_POLICY}, got {code!r}"
Issue-Key: iam.policy-invalid-effect|run#11|AssertionError|assert code == ERR_MALFORMED_POLICY, f"Expected {ERR_MALFORMED_POLICY}, got {code!r}"
Issue-Key: iam.policy-missing-action|run#12|AssertionError|assert code == ERR_MALFORMED_POLICY, f"Expected {ERR_MALFORMED_POLICY}, got {code!r}"
Issue-Key: iam.reject-identity-policy-containing-principal|_error_code#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iam.reject-trust-policy-without-principal|_error_code#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iam.create-service-linked-role-unsupported-service-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: iam.iam-access-key-crud|run#9|AssertionError|assert key_id.startswith("AKIA"), f"expected AKIA-prefixed access key id, got {key_id!r}"
Issue-Key: iam.user-access-key-round-trips-status-and-never-used|run#6|AssertionError|assert key["AccessKeyId"].startswith("AKIA"), f"a long-term key id starts AKIA: {key['AccessKeyId']!r}"
Issue-Key: iam.service-specific-credential-long-term-api-key-shape|run#13|AssertionError|assert created.get("ServiceCredentialAlias") == expected_alias, (
Issue-Key: iam.role-session-duration-and-description-bounds-rejected|run#5|AssertionError|assert "MaxSessionDuration" not in created, (
Issue-Key: iam.role-permissions-boundary-lifecycle|run#16|AssertionError|assert got["PermissionsBoundary"]["PermissionsBoundaryType"] == "Policy", (
Issue-Key: iam.list-attached-role-policies-denied-without-permission|_error#15|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: iam.condition-key-flips-a-real-authorization-decision|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: iam.iam-eval-deep-iam-1-date-less-than-deadline-passes-then-denied|_poll#28|Timeout|time.sleep(interval_s)
Issue-Key: iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct|_poll#32|Timeout|time.sleep(interval_s)
Issue-Key: iam.iam-eval-deep-iam-1-omitted-request-tag-operators-diverge-denied|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: iam.iam-eval-deep-iam-1-tag-keys-set-operator-swap-inverts-denied|_poll#20|Timeout|time.sleep(interval_s)
Issue-Key: iam.op-contract-listRoles-missing-permission-denied|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: iam.policy-simulation-explicit-deny-overrides-allow|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call|_settle#11|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# iam: 10 net-new behavioral root causes — four modeled operation families answer InvalidAction, policy-document validation is absent, generated credential shapes are wrong, and IAM's own API authorizes nothing

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSummary

## Description
Ten distinct net-new root causes in the bootstrapped `iam` service, all grounded in failing behavioral checks of this suite. None of them is marked ALREADY FILED or OWNED in the replay report. A fixer must close all ten before submitting.

Cause 1 of 10: GetAccountSummary is not routed at all.
AWS: Smithy (com.amazonaws.iam) binds GetAccountSummary to the service; input Unit, output GetAccountSummaryResponse{SummaryMap: summaryMapType}, only modeled error ServiceFailureException (500). A valid awsQuery Action=GetAccountSummary must therefore be served, never rejected as an unknown action.
Emulator: `ClientError (InvalidAction) when calling the GetAccountSummary operation: unknown action: GetAccountSummary` — the action is absent from the awsQuery dispatch table, so the operation is unreachable (not even a scaffold stub answer).
Why: any client that reads account-level quota/usage counters (console-equivalent summaries, STS-preference round-trips, condition-key probes that read SummaryMap) sees a protocol-level InvalidAction and cannot distinguish it from a bad SDK/region; retries never help.
Fix: the service's awsQuery action dispatch (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/service.rs plus the generated op table behind src/ops/mod.rs) must bind GetAccountSummary, and a handler must aggregate the counters from src/state.rs.
Traces: iam.account-summary-reflects-resource-counts, iam.account-summary-server-side-aggregation-reflects-resource-counts, iam.absent-condition-key-denied-unless-ifexists-forallvalues-or-null, iam.set-security-token-service-preferences, iam.sts-preferences-update-persists.

Cause 2 of 10: the account password-policy family is not routed.
AWS: Smithy binds GetAccountPasswordPolicy (output PasswordPolicy required; errors NoSuchEntityException 404 / ServiceFailureException 500). With no policy set AWS answers NoSuchEntity 404; after UpdateAccountPasswordPolicy it answers 200 with the stored policy, and DeleteAccountPasswordPolicy removes it.
Emulator: `ClientError (InvalidAction) when calling the GetAccountPasswordPolicy operation: unknown action: GetAccountPasswordPolicy`.
Why: a client cannot read, assert or tear down the account password policy, and password-strength failures (ChangePassword/CreateLoginProfile) cannot be correlated with the policy that caused them.
Fix: same dispatch table; add the password-policy handlers plus the policy record in src/state.rs.
Traces: iam.account-password-policy-lifecycle, iam.delete-account-password-policy, iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected.

Cause 3 of 10: the credential-report family is not routed.
AWS: Smithy binds GenerateCredentialReport (output State: ReportStateType, Description) and GetCredentialReport (output Content/ReportFormat/GeneratedTime; errors CredentialReportNotPresentException 410 ReportNotPresent, CredentialReportNotReadyException 404 ReportInProgress, CredentialReportExpiredException 410 ReportExpired). The documented state machine is generate -> INPROGRESS/COMPLETE -> readable -> expires.
Emulator: `ClientError (InvalidAction) when calling the GenerateCredentialReport operation: unknown action: GenerateCredentialReport` — the first call of every credential-report check fails.
Why: compliance tooling that enumerates credentials through the report (and every check of its TTL/expiry semantics) is dead on arrival; the three distinct 404/410 codes AWS uses are unobservable.
Fix: same dispatch table; add generate/get handlers and the report + generated-time/TTL record in src/state.rs.
Traces: iam.generate-credential-report, iam.get-credential-report, iam.credential-report-ready-then-expires, iam.credential-report-ttl-expiry-requires-regeneration.

Cause 4 of 10: the outbound web-identity-federation family is not routed.
AWS: Smithy binds GetOutboundWebIdentityFederationInfo (output IssuerIdentifier, JwtVendingEnabled: boolean default false; modeled error FeatureDisabledException 404 FeatureDisabled). Before the feature is enabled AWS answers FeatureDisabled 404; after enabling it answers 200 with the issuer, and a second enable of an already-enabled account is refused.
Emulator: `ClientError (InvalidAction) when calling the GetOutboundWebIdentityFederationInfo operation: unknown action: GetOutboundWebIdentityFederationInfo`; the enable/disable toggle is equally unreachable (check reports 'unexpected error enabling outbound web identity federation: InvalidAction').
Why: the feature's whole contract (disabled-by-default 404 vs enabled 200, and the double-toggle refusal) is replaced by a protocol error, so clients cannot even detect the feature exists.
Fix: same dispatch table; add the info/enable/disable handlers and the per-account flag in src/state.rs.
Traces: iam.get-outbound-web-identity-federation-info, iam.get-outbound-web-identity-federation-info-contract, iam.outbound-web-identity-federation-toggle, iam.webidentity-federation-double-toggle-rejected.

Cause 5 of 10: policy-document validation is not performed, so malformed documents are stored with 200.
AWS: CreateRole and CreatePolicy both model MalformedPolicyDocumentException (awsQuery code MalformedPolicyDocument, HTTP 400). AWS refuses: a Statement array that is empty, a Statement whose Effect is not Allow/Deny, a Statement with no Action, an identity policy that carries a Principal element, and a role trust policy with no Principal.
Emulator: every one of those is accepted with HTTP 200 and the entity is created — e.g. CreateRole with trust document {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:AssumeRole"}]} returned a full Role, and CreatePolicy with a Principal in an identity policy returned a full Policy. The checks report 'Expected MalformedPolicyDocument, got None' and 'expected a ClientError, but the call SUCCEEDED'.
Why: a client's broken IAM template deploys silently and the resulting role/policy grants or trusts something the author never expressed — the failure surfaces later as an unexplained allow/deny rather than at the write.
Fix: a shared policy-document validator invoked by the CreateRole/UpdateAssumeRolePolicy/CreatePolicy/PutRolePolicy/PutUserPolicy handlers in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/, raising MalformedPolicyDocument 400.
Traces: iam.policy-empty-statement-array, iam.policy-invalid-effect, iam.policy-missing-action, iam.reject-identity-policy-containing-principal, iam.reject-trust-policy-without-principal.

Cause 6 of 10: CreateServiceLinkedRole mints a role for a service principal that does not exist.
AWS: Smithy CreateServiceLinkedRole models InvalidInputException (InvalidInput, 400); AWS only creates service-linked roles for service principals that support them and refuses an unknown one.
Emulator: returns 200 with a fabricated role — Path '/aws-service-role/sim2real-not-a-real-service.amazonaws.com/', RoleName 'AWSServiceRoleForSim2real-not-a-real-service' and a trust policy naming that nonexistent service.
Why: a typo in AWSServiceName silently produces a role that can never be assumed by anything, and tests that assert the refusal pass against AWS and fail here.
Fix: validate AWSServiceName against the supported service-principal set in src/ops/create_service_linked_role.rs and raise InvalidInput 400.
Traces: iam.create-service-linked-role-unsupported-service-rejected.

Cause 7 of 10: generated credential identifiers do not follow AWS's documented shapes.
AWS: IAM long-term access key ids are AKIA-prefixed (the model constrains accessKeyIdType to ^[\w]+$ length 16-128; the AKIA prefix is AWS's documented convention for long-term user keys, ASIA being the temporary-credential prefix), and a service-specific credential's ServiceUserName follows the '{user}-at-{account}' convention. Recorded behavior confirms both.
Emulator: access key ids are minted as 'EMULATORAKUV5SQD4MU6' / 'EMULATORAKZDFSWOK7KB' (prefix 'EMULATOR'), and the service-specific credential alias came back as 'sim2real-behav-ssckeyshape-53ec82df91a5-v1-1792885262'. The particular random suffix is not the complaint — the prefix/format rule is.
Why: client code and SDK/credential tooling that classify a credential by its prefix (long-term AKIA vs session ASIA) misroute these keys, and anything that parses ServiceUserName to recover user+account breaks.
Fix: the id/alias minting in src/ops/create_access_key.rs and src/ops/create_service_specific_credential.rs (or the shared id generator in src/state.rs).
Traces: iam.iam-access-key-crud, iam.user-access-key-round-trips-status-and-never-used, iam.service-specific-credential-long-term-api-key-shape.

Cause 8 of 10: CreateRole echoes MaxSessionDuration in its response.
AWS: recorded behavior only — real IAM's CreateRole response omits MaxSessionDuration from the returned Role (it is readable later via GetRole). The model's Role shape permits the member, so this is a serializer-population divergence, not a schema one.
Emulator: the CreateRole response carried MaxSessionDuration=3600; response keys were ['Arn','AssumeRolePolicyDocument','CreateDate','MaxSessionDuration','Path','RoleId','RoleName','Tags'].
Why: a client that treats the presence of MaxSessionDuration on create as 'the caller set a session duration' records a value the caller never sent, and golden-response comparisons against AWS fail.
Fix: the CreateRole response serializer in src/ops/create_role.rs.
Traces: iam.role-session-duration-and-description-bounds-rejected.

Cause 9 of 10: PermissionsBoundaryType is serialized as the enum symbol, not its wire value.
AWS: Smithy AttachedPermissionsBoundary.PermissionsBoundaryType targets PermissionsBoundaryAttachmentType, whose member documentation states 'This data type can only have a value of Policy'. The wire value is Policy.
Emulator: returns 'PermissionsBoundaryPolicy' (the enum identifier) wherever the attached boundary is reported.
Why: a client that switches on PermissionsBoundaryType == 'Policy' never matches, so boundary-aware tooling treats a bounded role as unbounded.
Fix: the enum serialization for PermissionsBoundaryAttachmentType used by the role/user boundary responses (src/ops/get_role.rs, src/ops/put_role_permissions_boundary.rs and the shared serializer).
Traces: iam.role-permissions-boundary-lifecycle.

Cause 10 of 10: IAM performs no authorization on its own API, so denial-based checks either succeed wrongly or never converge.
AWS: IAM evaluates the caller's identity policy, session policy and condition keys on every IAM call; a principal with no grant is refused AccessDenied 403, and a policy write flips a subsequent signed call from allowed to denied (or back) within propagation.
Emulator: ListAttachedRolePolicies for a principal with no permission returned HTTP 200 with the AWSDenyAll attachment list ('real AWS refuses this call, but it succeeded'). Seven further checks poll a signed call for the expected flip and hit the harness 60 s cap because the decision never changes: condition-key flips, the four iam-eval-deep condition-operator cases, ListRoles-without-permission, the explicit-deny-over-allow simulation and the StringNotEquals/SourceVpce composition.
Why: every least-privilege assertion a client makes against this emulator passes vacuously — a policy that should deny does nothing, so a deployment validated here is unsafe in production. The open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md describes the session-policy half of this for other operations; these checks were not attributed to it by the replay report, and they add the identity-policy/condition-key legs on IAM's own read APIs.
Fix: an authorization gate ahead of the IAM operation handlers (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/service.rs) that resolves the signing principal, evaluates attached/inline/session policies with condition keys, and refuses with AccessDenied 403.
Traces: iam.list-attached-role-policies-denied-without-permission, iam.condition-key-flips-a-real-authorization-decision, iam.iam-eval-deep-iam-1-date-less-than-deadline-passes-then-denied, iam.iam-eval-deep-iam-1-denial-reason-messages-are-distinct, iam.iam-eval-deep-iam-1-omitted-request-tag-operators-diverge-denied, iam.iam-eval-deep-iam-1-tag-keys-set-operator-swap-inverts-denied, iam.op-contract-listRoles-missing-permission-denied, iam.policy-simulation-explicit-deny-overrides-allow, iam.sep-policy-composition-stringnotequals-missing-sourcevpce-key-deny-applies-denied-by-simulator-and-real-call.

## Evidence
Cause 1-4 (routing): botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the GetAccountSummary operation: unknown action: GetAccountSummary — identically for GetAccountPasswordPolicy, GenerateCredentialReport and GetOutboundWebIdentityFederationInfo. Expected: the operation served (200) or its modeled error (NoSuchEntity 404 / ReportNotPresent 410 / FeatureDisabled 404).
Cause 5: AssertionError: Expected MalformedPolicyDocument, got None (empty Statement array / invalid Effect / missing Action); AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'Role': {... 'AssumeRolePolicyDocument': {'Version': '2012-10-17', 'Statement': [{'Effect': 'Allow', 'Action': 'sts:AssumeRole'}]} ...}, 'HTTPStatusCode': 200}. Expected: MalformedPolicyDocument / 400.
Cause 6: AssertionError: expected a ClientError, but the call succeeded with {'Role': {'Path': '/aws-service-role/sim2real-not-a-real-service.amazonaws.com/', 'RoleName': 'AWSServiceRoleForSim2real-not-a-real-service', ...}, 'HTTPStatusCode': 200}. Expected: InvalidInput / 400.
Cause 7: AssertionError: expected AKIA-prefixed access key id, got 'EMULATORAKUV5SQD4MU6'; AssertionError: a long-term key id starts AKIA: 'EMULATORAKZDFSWOK7KB'; AssertionError: expected the alias to follow the password shape's '{user}-at-{account}' convention, got 'sim2real-behav-ssckeyshape-53ec82df91a5-v1-1792885262'.
Cause 8: AssertionError: real IAM's CreateRole response does not echo MaxSessionDuration at all; got 3600 in a response whose keys were ['Arn','AssumeRolePolicyDocument','CreateDate','MaxSessionDuration','Path','RoleId','RoleName','Tags'].
Cause 9: AssertionError: expected PermissionsBoundaryType 'Policy', got 'PermissionsBoundaryPolicy'.
Cause 10: AssertionError: real AWS refuses this call, but it succeeded and returned {'AttachedPolicies': [{'PolicyName': 'AWSDenyAll', 'PolicyArn': 'arn:aws:iam::aws:policy/AWSDenyAll'}], 'IsTruncated': False, ... 'HTTPStatusCode': 200}; plus 'Error: Check timed out after 60.0s' on the eight polling checks listed in that cause.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1-4: issue awsQuery Action=GetAccountSummary / GetAccountPasswordPolicy / GenerateCredentialReport / GetOutboundWebIdentityFederationInfo against the iam endpoint — observe InvalidAction 'unknown action' instead of the modeled response/error.
3. Cause 5: CreateRole with a trust document whose single statement has no Principal, and CreatePolicy with a document containing Principal, an empty Statement array, an invalid Effect, or no Action — observe 200 and a created entity instead of MalformedPolicyDocument 400.
4. Cause 6: CreateServiceLinkedRole(AWSServiceName='sim2real-not-a-real-service.amazonaws.com') — observe 200 instead of InvalidInput 400.
5. Cause 7: CreateAccessKey for a fresh user and CreateServiceSpecificCredential — inspect AccessKeyId prefix and ServiceUserName.
6. Cause 8/9: CreateRole and read the response keys; PutRolePermissionsBoundary then GetRole and read PermissionsBoundaryType.
7. Cause 10: create a user with no identity policy and one access key, then call ListAttachedRolePolicies / ListRoles signed with that key — observe 200; attach a deny/allow policy and poll a signed call for the flip — it never changes (60 s cap).
8. Re-replay the check ids in trace_ids to confirm.

Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cluster-policy-workflow
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cluster-policy-workflow
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|PutClusterPolicy|2xx->2xx|$.policyVersion
Issue-Commit: 82aaf2cb51feda9d788e7c4e8f0e80ec9e3e7eed

# dsql PutClusterPolicy answers policyVersion as a global decimal counter instead of an opaque policy-version token

> _Note: this issue was identified against the code base at commit `82aaf2cb51feda9d788e7c4e8f0e80ec9e3e7eed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
One cause.

Cause 1 of 1: PutClusterPolicy returns a policyVersion minted from a store-global incrementing integer, so the value it serves is a small decimal number ('1', '2', ...) rather than a policy-version token of the form AWS issues.

AWS: Smithy (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json) — PutClusterPolicy (POST /cluster/{identifier}/policy, 200) has required output member policyVersion targeting com.amazonaws.dsql#PolicyVersion, a plain string with NO length, pattern or enum constraint, documented as "The version of the policy after it has been updated or created."; the matching input member expectedPolicyVersion is documented as "The expected version of the current policy. This parameter ensures that you're updating the correct version of the policy and helps prevent concurrent modification conflicts." GetClusterPolicy returns the same member. Because the model states no lexical rule, the value's FORM is recorded behavior only: the recording's policyVersion is a per-run value, so the corpus holds it to its shape class instead of asserting the literal ('<shaped-not-asserted>'), and the class it records is an opaque version token — a high-entropy identifier-shaped string — not a small decimal integer. The requirement is therefore the token's FORM/class (an opaque per-write version identifier that a caller only ever round-trips into expectedPolicyVersion), never the one recorded literal, which is a per-run value. Hypothesis: AWS mints the token per policy write from random/identifier material (UUID-form), which is why it is neither guessable nor ordered.

Emulator: answers 200 with policyVersion "1" (replay detail: "$.policyVersion: value_mismatch (expected '<shaped-not-asserted>', got '1')"), and "2" on the next write in the same fixture. The value is the decimal rendering of a single STORE-WIDE counter: services/service-com-amazonaws-dsql-dsql/src/state.rs lines 1020-1023 (`mint_policy_version`: `self.policy_versions_issued += 1; self.policy_versions_issued.to_string()`), reached from `attach_cluster_policy` (lines 1035-1046) and returned by services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs lines 335-357. GetClusterPolicy and the `expectedPolicyVersion` conflict check read the same stored string, so the whole surface agrees internally — it just serves a different value class than AWS.

Why: a client that validates, stores or logs policyVersion sees a value of a shape AWS never produces, so round-trip tests, fixtures and schema/format assertions written against AWS fail against the emulator. Worse for correctness-by-construction: because the counter is dense, ordered and shared by every cluster in the store, a caller can trivially GUESS a valid policyVersion for a cluster it has not read (send expectedPolicyVersion="1") and have the optimistic-concurrency check pass where against AWS an unguessable token makes that impossible — so concurrent-modification protection, the member's stated purpose, is materially weaker, and the served value additionally discloses how many policy writes the account has performed.

Fix: the policy-version minting in the service's state module — `mint_policy_version` in services/service-com-amazonaws-dsql-dsql/src/state.rs — must issue an opaque, non-sequential per-write version token (still never re-issued, still store-unique, still the exact string GetClusterPolicy and the `expectedPolicyVersion` comparison use). No change is needed in PutClusterPolicy's handler, GetClusterPolicy, or the CreateCluster inline-policy path beyond consuming the new token, since all three already go through that one mint.

Related but NOT the same cause: open issue 20261029T185208Z-dsql-putclusterpolicy-s-lockout-safety-refusal-carries-the.md covers PutClusterPolicy's lockout ValidationException message; its replay keys do not cover $.policyVersion, which replay reports here with owned_by empty.

Traces: dsql-cluster-policy-workflow#task#4 (representative) and the same divergence again at dsql-cluster-policy-workflow#task#6 (the second policy write in that workflow, which answers "2").

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 96.9%, 3 mismatched steps of 96):
- Cluster divergence_key service=dsql|PutClusterPolicy|status=200->200|keys=$.policyVersion, verdict mismatch, expected_status 200 / actual_status 200, stub_path empty (operation implemented), owned_by empty (no open issue carries this replay key).
- detail: "$.policyVersion: value_mismatch (expected '<shaped-not-asserted>', got '1')".
- affected (2): dsql-cluster-policy-workflow#task#4, dsql-cluster-policy-workflow#task#6.
- Which parts of the value are a requirement: the member's CLASS — an opaque per-policy-write version token that is unique in the service and is the exact string GetClusterPolicy returns and `expectedPolicyVersion` is compared against. The recorded literal is a per-run value and is NOT a requirement; neither is any particular length or alphabet beyond its being non-sequential and not caller-guessable.
- Emulator source: services/service-com-amazonaws-dsql-dsql/src/state.rs:1007-1023 (`mint_policy_version`, store-global `policy_versions_issued` counter rendered with `to_string()`), 1025-1046 (`attach_cluster_policy`); services/service-com-amazonaws-dsql-dsql/src/ops/put_cluster_policy.rs:335-357 (returns that string as `policy_version`), 307-332 (`expectedPolicyVersion` compared to the stored counter value).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture dsql-cluster-policy-workflow.
2. CreateCluster and wait for ACTIVE (earlier steps of the fixture).
3. At step task#4, POST /cluster/{identifier}/policy with a valid policy document (PutClusterPolicy).
4. Read $.policyVersion from the 200 response: the emulator returns "1", a decimal store-global sequence number, where AWS returns an opaque per-write policy-version token.
5. At step task#6, write the cluster's policy again: the emulator returns "2", confirming the value is a dense ordered counter rather than an opaque token.
6. Optional confirmation of the consequence: against a fresh emulator, call PutClusterPolicy on a cluster whose policy version was never read, passing expectedPolicyVersion="1" — the guess is accepted, which cannot happen against AWS's unguessable token.

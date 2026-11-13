Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-policy-catalog-browsing
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-policy-catalog-browsing mpa-policy-version-lookup-not-found-and-default-flag
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|GetPolicyVersion|2xx->2xx|$.PolicyVersion.Document
Issue-Key: mpa|ListPolicyVersions|2xx->2xx|$.PolicyVersions[Arn="arn
Issue-Commit: f6da550b15b4b05284a85cab6065d0eace79555a

# mpa policy-catalog projections diverge from the published catalog: wrong managed-policy document and second-precision date-times

> _Note: this issue was identified against the code base at commit `f6da550b15b4b05284a85cab6065d0eace79555a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicyVersion

## Description
Two net-new causes, both in the same place: the seeded MPA policy catalog in `services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs` and the projection the catalog reads serialize. Both are read-path (HTTP 200) body divergences on the one policy the catalog publishes, `arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy`, so every reader of that policy (`GetPolicyVersion`, `ListPolicyVersions`, and the resource-policy reads that resolve the same entry) is affected.

Cause 1 of 2: GetPolicyVersion returns the wrong document for the AWS-managed MPA approval-team policy (fixture `mpa-policy-catalog-browsing`, step `task#3`).
  AWS: `GetPolicyVersion` returns `PolicyVersion.Document` as the IAM policy document the catalog publishes for that version, verbatim. Smithy: `GetPolicyVersion` -> `PolicyVersion` (required), member `Document` -> `PolicyDocument` (@length max 400000, @sensitive), documented as "Document that contains the policy contents"; `GetPolicyVersion` is `@readonly` and no operation in the model mints or edits a policy, so the document is AWS's published text and not something the service derives. The model does not state that text, so for the text itself this is recorded behavior only: the recording shows a PRETTY-PRINTED document (2-space indentation, one member per line, trailing newline) whose single statement has `"Sid": "VaultOwnerPermissions"`, carries a `"Principal": {"AWS": "*"}` element, `"Resource": "*"`, `"Action": ["mpa:StartSession", "mpa:CancelSession"]`, and a `Condition` with `StringEquals` `"mpa:RequestedOperation": "backup:RevokeRestoreAccessBackupVault"` plus `"mpa:ProtectedResourceAccount": "${aws:PrincipalAccount}"` and `Bool` `"aws:ViaAWSService": "true"`.
  Emulator: it answers 200 with a different document altogether — a COMPACT (no whitespace, no trailing newline) JSON whose statement has `"Sid":"AWSBackupLAGVaultProtection"`, NO `Principal` element at all, and `"mpa:RequestedOperation":"backup:DisassociateRecoveryPoint"`. Only `Version`, `Effect`, `Action`, `Resource`, `ProtectedResourceAccount` and the `Bool` condition agree. The text is a hand-written constant, `VAULT_PROTECTION_POLICY_DOCUMENT`, that the seeded catalog entry hands straight to the projection, so the divergence is in the catalog's data, not in the lookup: `$.PolicyVersion.Document: value_mismatch`.
  Why: this document is the authorization contract a client reads to learn which operation the MPA approval team actually gates and who may start/cancel its sessions. A client that parses it sees the wrong protected operation (`backup:DisassociateRecoveryPoint` instead of `backup:RevokeRestoreAccessBackupVault`) and no principal element, so a policy-analysis, drift-detection or least-privilege tool run against the emulator reaches the opposite conclusion about what the published managed policy permits; byte-level consumers (hash, signature, string diff against the published policy) also disagree because the whole document is reflowed.
  Fix: the seeded policy catalog in `crate::state` — the published document constant and the catalog row that carries it — must hold AWS's published text for this managed policy version, including its formatting and its `Principal` element, and the read projection must return it unchanged. No change is needed in the ARN/version resolution, which resolved the right version.
  Traces: mpa-policy-catalog-browsing#task#3 (GetPolicyVersion, expected 200, actual 200, body mismatch).

Cause 2 of 2: ListPolicyVersions serializes `date-time` timestamps without the fractional-second field (fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`).
  AWS: `PolicyVersionSummary.CreationTime` and `PolicyVersionSummary.LastUpdatedTime` both target `IsoTimestamp` with `@timestampFormat("date-time")` (Smithy: `ListPolicyVersions` -> `PolicyVersions` -> `PolicyVersionSummary.CreationTime`/`LastUpdatedTime`, both required). The model fixes the format family only; the precision AWS emits is recorded behavior only, and the recording shows MPA always writing three fractional digits: `2025-06-17T00:00:00.000Z` for both members of version 1 of `arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy`.
  Emulator: it answers 200 but writes `2025-06-17T00:00:00Z` for both members — the fractional-second field is dropped whenever it would be all zeros, because the value is formatted from `aws_smithy_types::DateTime` with the elide-trailing-zeros date-time rendering rather than with fixed millisecond precision. Replay reports two `value_mismatch` entries on that one element: `$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime` and `...LastUpdatedTime`.
  Why: the divergence is in the SHAPE of the rendering, not in the instant (the instant itself is correct, and it is a seeded catalog instant, not a per-run clock value). A client that compares the serialized string, caches/etags it, or re-signs a canonicalized body sees a different value from AWS for the same timestamp; any consumer with a strict `yyyy-MM-dd'T'HH:mm:ss.SSS'Z'` parser fails outright. Because this is the shared date-time rendering, every other mpa member of this format is rendered the same way and diverges whenever the instant lands on a whole second.
  Fix: the restJson1 `date-time` serializer path used by this service's output projections (the `ListPolicyVersions` / `GetPolicyVersion` serializers and whatever helper they share for `IsoTimestamp`) must emit a fixed three-digit fractional-second field, so a whole-second instant renders as `...T00:00:00.000Z`. The stored instants are correct and need no change.
  Traces: mpa-policy-version-lookup-not-found-and-default-flag#task#3 (ListPolicyVersions, expected 200, actual 200, two body mismatches).

Not covered by anything already open: the only open mpa issue touching these operations, `20261029T083022Z-mpa-listpolicyversions-error-response.md`, is about the PRECEDENCE of the 404 `ResourceNotFoundException` against the `NextToken` `ValidationException` on `ListPolicyVersions`; it says nothing about the catalog's document text or about timestamp rendering, and both causes here are on 200 responses. The other open mpa issues (`mpa-createapprovalteam-event-propagation`, `mpa-getresourcepolicy-wrong-behavior`, `mpa-deleteidentitysource-wrong-behavior`, `mpa-updateapprovalteam-*`, `mpa-listidentitysources-pagination`, `mpa-identitysourcearn-refusals-diverge`) are on unrelated operations. Neither replay cluster reported a `stub_path` or an `owned_by` owner.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 35 steps, 33 passed, 2 mismatched, score 94.3%, emulator_up=true, bootstrap_signal=false. Both divergences are service `mpa`, verdict `mismatch`, stub_path empty, owned_by empty.

Cause 1 — fixture `mpa-policy-catalog-browsing`, step `task#3`, operation GetPolicyVersion, expected_status 200, actual_status 200.
divergence_key: service=mpa|GetPolicyVersion|status=200->200|keys=$.PolicyVersion.Document
detail: `$.PolicyVersion.Document: value_mismatch`
  expected (AWS, pretty-printed, trailing newline):
    {
      "Version": "2012-10-17",
      "Statement": [
        {
          "Sid": "VaultOwnerPermissions",
          "Effect": "Allow",
          "Principal": { "AWS": "*" },
          "Resource": "*",
          "Action": [ "mpa:StartSession", "mpa:CancelSession" ],
          "Condition": {
            "StringEquals": {
              "mpa:RequestedOperation": "backup:RevokeRestoreAccessBackupVault",
              "mpa:ProtectedResourceAccount": "${aws:PrincipalAccount}"
            },
            "Bool": { "aws:ViaAWSService": "true" }
          }
        }
      ]
    }
  actual (emulator, compact, no trailing newline):
    {"Version":"2012-10-17","Statement":[{"Sid":"AWSBackupLAGVaultProtection","Effect":"Allow","Action":["mpa:StartSession","mpa:CancelSession"],"Resource":"*","Condition":{"StringEquals":{"mpa:RequestedOperation":"backup:DisassociateRecoveryPoint","mpa:ProtectedResourceAccount":"${aws:PrincipalAccount}"},"Bool":{"aws:ViaAWSService":"true"}}}]}
Three differences: the `Sid` token, the absent `Principal` element, and the `mpa:RequestedOperation` value. Source of the served text — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:2795-2802 (`VAULT_PROTECTION_POLICY_DOCUMENT`, the compact constant with `Sid` `AWSBackupLAGVaultProtection` and `backup:DisassociateRecoveryPoint`), carried into the catalog at state.rs:2818-2827 (`SEEDED_POLICY_CATALOG`, the single `IdentityCenter/MPAApprovalTeamPolicy` row, `document: VAULT_PROTECTION_POLICY_DOCUMENT`) and returned unchanged by src/ops/get_policy_version.rs:211-227 (`detail()` -> `document: version.document.clone()`).

Cause 2 — fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`, operation ListPolicyVersions, expected_status 200, actual_status 200.
divergence_key: service=mpa|ListPolicyVersions|status=200->200|keys=$.PolicyVersions[Arn="arn
detail:
  `$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].CreationTime: value_mismatch (expected '2025-06-17T00:00:00.000Z', got '2025-06-17T00:00:00Z')`
  `$.PolicyVersions[Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1"].LastUpdatedTime: value_mismatch (expected '2025-06-17T00:00:00.000Z', got '2025-06-17T00:00:00Z')`
The instant is the same in both; only the rendering differs (missing `.000` fractional-second field). The instant is a seeded catalog constant, not a per-run clock value — state.rs:2825-2826 records `published_at_epoch_secs: 1_750_118_400` ("2025-06-17T00:00:00Z, the instant the catalog records for this version") — so this is a reproducible serialization difference, not a timing artifact.
Model grounding: smithy_query(operation="GetPolicyVersion,ListPolicyVersions") and smithy_query(shape="PolicyVersion,PolicyVersionSummary") on services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json: service AWSFluffyCoreService (sdkId MPA, protocol restJson1); `CreationTime`/`LastUpdatedTime` -> `IsoTimestamp`, `timestampFormat: date-time`, required, on both `PolicyVersion` and `PolicyVersionSummary`; `Document` -> `PolicyDocument`, required, @length max 400000, @sensitive.

## Steps to Reproduce
Prerequisite: clone this checkout and boot the emulator from it (the mpa service is bootstrapped: `service-com-amazonaws-mpa-awsfluffycoreservice` is in get_bootstrap_status's `bootstrapped` set). No setup calls are needed — the policy catalog is seeded on first touch of the store.

Cause 1 (GetPolicyVersion document):
1. Replay fixture `mpa-policy-catalog-browsing`, step `task#3`, or issue it directly:
   GET /policy-versions/arn%3Aaws%3Ampa%3A%3A%3Aaws%3Apolicy%2FIdentityCenter%2FMPAApprovalTeamPolicy%2F1 (SigV4, service name `mpa`). `$DEFAULT` as the qualifier resolves to the same version.
2. Read `$.PolicyVersion.Document` from the 200 response.
3. Observe the compact one-line document with `"Sid":"AWSBackupLAGVaultProtection"`, no `Principal` element, and `"mpa:RequestedOperation":"backup:DisassociateRecoveryPoint"`, where AWS returns the pretty-printed document with `"Sid": "VaultOwnerPermissions"`, a `"Principal": {"AWS": "*"}` element, and `"mpa:RequestedOperation": "backup:RevokeRestoreAccessBackupVault"`.

Cause 2 (date-time precision):
4. Replay fixture `mpa-policy-version-lookup-not-found-and-default-flag`, step `task#3`, or issue it directly:
   POST /policies/arn%3Aaws%3Ampa%3A%3A%3Aaws%3Apolicy%2FIdentityCenter%2FMPAApprovalTeamPolicy/?List&MaxResults=20
5. In the 200 response, find the `PolicyVersions` element whose `Arn` is `arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1`.
6. Observe `CreationTime` and `LastUpdatedTime` rendered as `2025-06-17T00:00:00Z`; AWS renders the same instant as `2025-06-17T00:00:00.000Z` (fixed three-digit fractional-second field).
7. Confirm the rendering, not the instant, is at fault by reading the same two members off `GetPolicyVersion` in step 1 — any mpa `IsoTimestamp` landing on a whole second is rendered without `.000`.

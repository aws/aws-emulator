Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b880698c4f2cb4e70545429d30696ac71b7bccc0

# GetRevocationStatus answers 200 "nothing revoked" for a profileVersionArn/jobArn that is no Signer ARN, although the handler already detects it and its own doc says a malformed identifier is told so

_Filed: 2026-11-28T12:27:15Z_
_Operation: GetRevocationStatus | Class: validation_

> _Note: this issue was identified against the code base at commit `b880698c4f2cb4e70545429d30696ac71b7bccc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's module doc states the rule it holds a request's identifiers to, and the code under it breaks that rule for the two ARN-carrying identifiers.

The handler judges three value rules before it reads the registry (a future `signatureTimestamp`, an empty `certificateHashes`, and each `certificateHashes` entry), all taken from the shared rules module it imports as `signer_input_rules`. It judges NOTHING about the SHAPE of `profileVersionArn` and `jobArn` beyond the modelled 20..2048 length the framework already enforces — yet the same shared module it imports exposes exactly those shape judgements (`profile_version_in_arn`, `job_region_and_id_in_arn`) and the sentence that refuses one (`invalid_member_message`).

The handler already COMPUTES the "this value is no Signer version ARN" fact one step earlier: the authorization block calls `signer_authorization::profile_of_version_arn(&input.profile_version_arn)` and, on `None`, falls back to the all-resources form with the comment "A value that is no version ARN of Signer addresses no profile". Having established that the request's `profileVersionArn` addresses no profile at all, the handler then uses that same string as a registry key, finds nothing (it is not a key any revocation was recorded under), and answers HTTP 200 with `{"revokedEntities": []}` — telling the caller that nothing it named is revoked, which is precisely what the module doc says must NOT happen for a malformed identifier. The same holds for `jobArn`, for which no shape judgement is made at all.

This is a fail-open answer on a revocation check: a caller that truncates, mis-renders or mixes up an ARN is told the artifact is not revoked instead of being told its identifier is invalid.

Note on scope/dedup: the existing issues about this refusal (20261023T031331Z Cause 2, 20261029T230237Z Cause 4) were closed against the Signer CONTROL-plane crate's handler, which now performs exactly these two shape gates; this crate's handler — the one serving the `data-signer` route key — still has no such gate, and the two open issues on this crate (private-registry / event-propagation, and the ENDPOINT_PREFIX doc) are about other defects.

## Evidence

The rule, stated by the handler's own module doc — services/service-com-amazonaws-signerdata-signerdataplane/src/ops/get_revocation_status.rs:9-14:
  "//! The VALUES a request carries are held to this route's published contract before the
   //! registry is read, by the rules the control plane owns ([`signer_input_rules`]): a
   //! `signatureTimestamp` the service's present time has not reached names no signature
   //! that can exist, and a `certificateHashes` entry that is not a certificate identifier
   //! names no certificate — so a request whose identifiers are malformed is told so rather
   //! than told that nothing it named is revoked."

The same file, on the imported rules module — src/ops/get_revocation_status.rs:96-100:
  "/// The rules the VALUES of a `GET /revocations` request are held to, and the one wording
   /// of each refusal of one, owned by that same control plane for the same reason: the two
   /// endpoints publish ONE contract for this route, so a rule stated twice could come to
   /// refuse — or admit — different requests at the two of them.
   use ::service_com_amazonaws_signer_wallabyservice::state as signer_input_rules;"

The handler already detects the malformed identifier — src/ops/get_revocation_status.rs:178-189:
  "// A value that is no version ARN of Signer addresses no profile, so it is
   // judged against the all-resources form IAM writes for an action that names none.
   ...
   let authorized_resource =
       signer_authorization::profile_of_version_arn(&input.profile_version_arn)
           .unwrap_or_else(|| signer_authorization::ANY_RESOURCE.to_string());"

…and then refuses nothing for it. The complete set of value rules the handler applies is src/ops/get_revocation_status.rs:213-243: `future_signature_timestamp_refusal` (213), `input.certificate_hashes.is_empty()` (227), `certificate_hashes_refusal` (240). No `profileVersionArn`/`jobArn` rule appears, and the handler's closing comment limits what it assumes was already checked — src/ops/get_revocation_status.rs:249-251:
  "// Every modelled constraint (the `platformId` pattern, the 20..2048 ARN
   // lengths, the required query parameters) has already been refused with a 400
   // before this handler runs, so reaching here means the input satisfies them."
so only the LENGTH of the two ARNs is ever judged; the value is then used directly as a registry key — src/ops/get_revocation_status.rs:252-258:
  "let revoked_entities = state.lock().revoked_entities(... &input.profile_version_arn, &input.job_arn, ...)" → src/state.rs:228 `let Some(record) = self.revocation(kind, key) else { continue; };` → 200 with an empty list.

The shape judgements and the refusal sentence the handler would need are public members of the very module it imports as `signer_input_rules`:
  services/service-com-amazonaws-signer-wallabyservice/src/state.rs:1190 `pub fn profile_version_in_arn(arn: &str) -> Option<(&str, &str, &str)>` ("a judgement on the ARN's SHAPE alone … so it says nothing about whether such a version exists", lines 1187-1189),
  :1241 `pub fn job_region_and_id_in_arn(arn: &str) -> Option<(&str, &str)>`,
  :1263 `pub fn invalid_member_message(member: &str, value: &str) -> String` → `format!("Invalid {member}: {}", stated_value(value))`.

Test coverage: nothing pins the current acceptance. Every request in this crate's tests builds well-formed Signer ARNs — tests/all/revocation_status.rs:65-72: `engine::emulator_arn("signer", REGION, &format!("/signing-profiles/{name}/v1"))` and `engine::emulator_arn("signer", REGION, &format!("/signing-jobs/{name}"))` — and the only test asserting a 200 for a complete request uses those helpers (tests/all/refusal_clause_wording.rs:259-282, `a_request_that_breaks_no_constraint_is_still_served`). The ARN rows in tests/all/signature_timestamp_forms.rs:379-397 and refusal_clause_wording.rs:173-195 only exercise values OUTSIDE the modelled length band, which stay refused by the framework.

## Steps to Reproduce

1. Read services/service-com-amazonaws-signerdata-signerdataplane/src/ops/get_revocation_status.rs:9-14 — the handler's own doc says a request whose identifiers are malformed is told so rather than told that nothing it named is revoked.
2. Read lines 187-189 of the same file: the handler computes `signer_authorization::profile_of_version_arn(&input.profile_version_arn)` and handles `None` as "a value that is no version ARN of Signer" (comment, lines 178-181).
3. Read lines 213-243: the only value rules applied are the future-timestamp rule and the two `certificateHashes` rules; neither ARN member is judged.
4. Send `GET /revocations?signatureTimestamp=<an instant at or before now>&platformId=Notation-OCI-SHA384-ECDSA&profileVersionArn=arn:aws:signer:us-east-1:000000000000:not-a-real-arn-shape&jobArn=arn:aws:signer:us-east-1:000000000000:not-a-real-arn-shape&certificateHashes=<192 hex characters>` to the `data-signer` route key (as tests/all/revocation_status.rs:130-147 dispatches it).
5. Observe HTTP 200 with `{"revokedEntities": []}` — the "nothing it named is revoked" answer — for two identifiers the handler itself established address no Signer resource, instead of a refusal naming the offending member.
6. Confirm the fix is additive: re-run this crate's test suite, whose every request uses the well-formed ARNs built at tests/all/revocation_status.rs:65-72, which `profile_version_in_arn` / `job_region_and_id_in_arn` accept.

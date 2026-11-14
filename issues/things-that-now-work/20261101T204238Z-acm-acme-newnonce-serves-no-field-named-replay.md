Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.revoke-acme-account-sets-terminal-state
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.revoke-acme-account-sets-terminal-state|_acme_register_account#10|AssertionError|assert nonce, f"no Replay-Nonce returned from newNonce (status={status})"
Issue-Commit: 8f015019990d45131c5e858500566f2ce48bb7da

# ACM ACME newNonce serves no field named Replay-Nonce, wedging the native ACME data plane before the first JWS (1 cause)

> _Note: this issue was identified against the code base at commit `8f015019990d45131c5e858500566f2ce48bb7da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeAcmeAccount

## Description
Cause 1 of 1: the ACME `newNonce` resource an ACM ACME endpoint publishes answers with the correct statuses but with no field named `Replay-Nonce`, so a client that reads the response's field names exactly as received is handed no nonce and cannot sign a single JWS. Failing check: acm.revoke-acme-account-sets-terminal-state.

Related already-open issue, which does NOT cover this check: `20261027T102721Z-acm-acme-newnonce-emits-a-lower-cased-replay-nonce-field-name.md` describes the same seam (the nonce field arriving lower-cased as `replay-nonce`) but is keyed to a different check, `acm.update-acme-endpoint-on-revoked-account-succeeds`. This filing carries the replay key for `acm.revoke-acme-account-sets-terminal-state`, which the replay report did not mark as owned; a fixer closing that issue should re-replay this check too, and may close both together if one change fixes the seam.

AWS: recorded behavior only — the ACME data plane is not a modelled Smithy operation (the ACM Smithy service `com.amazonaws.acm#CertificateManager` is awsJson1_1 and binds only control-plane operations such as CreateAcmeEndpoint / DescribeAcmeEndpoint / RevokeAcmeAccount / DescribeAcmeAccount; the endpoint URL they hand back is spoken to with RFC 8555, not with X-Amz-Target). RFC 8555 §7.2 fixes the field spelling as `Replay-Nonce` and requires it on BOTH the 200 answer to `HEAD <newNonce>` and the 204 answer to `GET <newNonce>`; AWS's ACM ACME endpoints put it on the wire under that exact spelling. The field name is the fixed text of the requirement; the nonce value itself is a per-request value and is not part of the template.

Emulator: `GET <EndpointUrl>` serves the directory 200 with `newNonce`/`newAccount` URLs, and the nonce resource answers with the right statuses (`HEAD` -> 200, `GET` -> 204), but neither answer carries a field named `Replay-Nonce`. The check does `HEAD <newNonce>`, finds nothing under that name, retries with `GET <newNonce>` and gets HTTP 204 with still nothing under that name, and fails with `AssertionError: no Replay-Nonce returned from newNonce (status=204)`. The ACM crate already STATES the canonical name — `services/service-com-amazonaws-acm-certificatemanager/src/service/acme_http.rs` documents `HEAD <base>/new-nonce -> 200`, `GET <base>/new-nonce -> 204` with the nonce in the `Replay-Nonce` header (RFC 8555 §7.2) — so the name is being normalised after the handler states it, on the response-writing path below the service crate. Hypothesis: the field name is lower-cased by the typed response object and again when the outgoing response is rebuilt name-by-name, so the bytes a client receives spell it `replay-nonce`.

Why: an ACME client (certbot, lego, acme.sh) takes its first anti-replay nonce from `newNonce` and can send no JWS POST without it. Against AWS the client registers an account and proceeds; against the emulator it wedges before the first POST. Everything downstream of registration is therefore unreachable for a reason unrelated to its own implementation — in this check: ACME NewAccount with the external-account binding, DescribeAcmeAccount reporting VALID, RevokeAcmeAccount moving the account to the terminal REVOKED state (rather than deleting it), and the idempotent second RevokeAcmeAccount that must succeed without ConflictException and leave the account REVOKED.

Fix: the response-writing seam for this surface must preserve the canonical spelling of a field name the ACME handler states; the ACME surface in `acme_http.rs` already names it correctly and needs no change. If that seam is not reachable from `services/`, the nonce surface needs whatever mechanism this emulator offers for emitting a field name verbatim.

Traces: acm.revoke-acme-account-sets-terminal-state.

## Evidence
acm.revoke-acme-account-sets-terminal-state [mismatch]:
  AssertionError: no Replay-Nonce returned from newNonce (status=204)
  raised at check_acm_revoke_acme_account_sets_terminal_state.py line 203 (`assert nonce, f"no Replay-Nonce returned from newNonce (status={status})"`), inside `_acme_register_account`, called from line 57 of `run`.

  Expected (AWS / RFC 8555 §7.2): `HEAD <newNonce>` -> 200 with a field named exactly `Replay-Nonce` holding a fresh nonce; `GET <newNonce>` -> 204 with the same field present.
  Actual (emulator): `HEAD <newNonce>` -> no field named `Replay-Nonce`; the fallback `GET <newNonce>` -> HTTP 204, again no field under that name. The check therefore has no nonce and aborts.

  Upstream steps all passed: CreateAcmeEndpoint(AuthorizationBehavior=PRE_APPROVED, CertificateAuthority=PublicCertificateAuthority, Contact=NOT_REQUIRED) polled to Status==ACTIVE; iam.CreateRole for the EAB trust policy; CreateAcmeExternalAccountBinding; GetAcmeExternalAccountBindingCredentials returned KeyId and MacKey; DescribeAcmeEndpoint returned AcmeEndpoint.EndpointUrl; `GET <EndpointUrl>` served the directory 200 containing `newNonce` and `newAccount`.

  Unreached, so unverified by this run: ACME NewAccount (ES256 JWS with the HS256 externalAccountBinding) answering 200/201 with a `Location` account URL; DescribeAcmeAccount reporting Status VALID; RevokeAcmeAccount moving it to REVOKED; a redundant RevokeAcmeAccount succeeding idempotently (no ConflictException) with DescribeAcmeAccount still reporting REVOKED.

  Note on the status code: the 204 in the message is the GET fallback's status and is correct per RFC 8555 §7.2 — the divergence is solely the absent `Replay-Nonce` field name, not the status.

## Steps to Reproduce
1. Boot the emulator. Create an ACME endpoint: CreateAcmeEndpoint(AuthorizationBehavior=PRE_APPROVED, CertificateAuthority={PublicCertificateAuthority:{}}, Contact=NOT_REQUIRED), then poll DescribeAcmeEndpoint until AcmeEndpoint.Status == ACTIVE and read AcmeEndpoint.EndpointUrl.
2. `GET <EndpointUrl>` — the ACME directory is served 200. Take `directory['newNonce']`.
3. `HEAD <newNonce>`. Read the response's field names EXACTLY as received (do not lower-case them before lookup). No field named `Replay-Nonce` is present.
4. `GET <newNonce>` — HTTP 204; again no field under the name `Replay-Nonce`.
5. Because no nonce is available, no JWS can be signed: ACME NewAccount is never attempted, so RevokeAcmeAccount's terminal-REVOKED contract and its idempotent second call cannot be exercised.
6. Against AWS, both the HEAD 200 and the GET 204 carry the field spelled `Replay-Nonce`, the client registers an account, and the revoke path is reachable.

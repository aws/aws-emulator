Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-access-mapping-requires-directory-idp
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-access-mapping-requires-directory-idp transfer-certificate-blob-preserved
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ListAccesses|4xx->4xx|$.Message
Issue-Key: transfer|DescribeCertificate|2xx->2xx|$.Certificate.Status
Issue-Commit: b55e0f7c845ae67f084f86c93612944062e106e8

# Transfer response-field fidelity: ListAccesses IdP-type error message wording and DescribeCertificate Status derivation diverge

> _Note: this issue was identified against the code base at commit `b55e0f7c845ae67f084f86c93612944062e106e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccesses

## Description
Two net-new causes in the AWS Transfer Family emulator, both response-value divergences on a successfully routed call (the status code matches, the payload field does not). Both must be closed before this issue is submitted.

Cause 1 of 2: ListAccesses on a server whose IdentityProviderType is SERVICE_MANAGED returns the wrong error message text.
  AWS: ListAccesses is modelled only for servers backed by a directory/custom identity provider; on a SERVICE_MANAGED server it fails with a 400 client error (Smithy: ListAccesses errors InvalidRequestException, errorKind client, httpError 400; ResourceNotFoundException 404; input ServerId required, pattern ^s-([0-9a-f]{17})$). Recorded behavior for the message: HTTP 400 with Message exactly "Cannot list accesses on server with IdP type: SERVICE_MANAGED". Fixed text of that message template is "Cannot list accesses on server with IdP type: "; the only variable part is the identity-provider type taken from the named server's stored configuration (SERVICE_MANAGED in the recording, but it is state-derived, not a literal).
  Emulator: same 400, but Message is "Cannot list access on server with IdP type: SERVICE_MANAGED" — the fixed text is misspelled, "list access" instead of "list accesses".
  Why: clients and test suites that branch on the error message (Transfer reuses one InvalidRequestException shape for many distinct rejections, so the message is the only discriminator) never match this rejection, and assertions on AWS's wording fail against the emulator.
  Fix: the error-message constant used by the ListAccesses handler's identity-provider-type guard in the transfer service crate.

Cause 2 of 2: DescribeCertificate reports Status INACTIVE for a certificate that AWS reports as ACTIVE.
  AWS: Smithy DescribedCertificate.Status targets CertificateStatusType (values ACTIVE, PENDING_ROTATION, INACTIVE) and its documentation states the rule: "You can set ActiveDate and InactiveDate in the UpdateCertificate call. If you set values for these parameters, those values are used to determine whether the certificate has a status of ACTIVE or INACTIVE. If you don't set values for ActiveDate and InactiveDate, we use the NotBefore and NotAfter date as specified on the X509 certificate to determine when a certificate is active and when it is inactive." So for a certificate imported without ActiveDate/InactiveDate whose X509 validity window contains the current time, Status is ACTIVE; the recorded response confirms HTTP 200 with $.Certificate.Status == "ACTIVE".
  Emulator: HTTP 200 but $.Certificate.Status == "INACTIVE" — the status is not derived from the active/inactive window at all, it is a fixed (or default) value, so a freshly imported, in-window certificate never becomes ACTIVE.
  Why: a client that waits for or gates on an ACTIVE certificate before wiring it into an AS2 profile/connector (Usage SIGNING/ENCRYPTION/TLS) either loops forever or refuses to use a certificate AWS considers usable; any workflow that filters certificates by Status sees an empty set.
  Fix: the Status derivation in the certificate state/DescribeCertificate serializer of the transfer service crate — compute it from ActiveDate/InactiveDate when set, otherwise from the stored NotBeforeDate/NotAfterDate of the imported X509, rather than storing a literal.

Traces: transfer-access-mapping-requires-directory-idp#task#2 (Cause 1), transfer-certificate-blob-preserved#task#2 (Cause 2).

No open Transfer issue covers either cause: the closest open items are "Transfer input validation, authorization and reference data diverge..." (20261022T202037Z) and "Transfer host-key and web-app planes still refuse valid calls..." (20261026T022407Z), neither of which carries these replay keys or touches ListAccesses' IdP guard message or DescribeCertificate's Status derivation.

## Evidence
Replay of trace set simparity-wire-aws-v1 (345 steps, 342 passed, 3 mismatched):

Cause 1 — fixture transfer-access-mapping-requires-directory-idp, step task#2, operation ListAccesses, verdict mismatch, expected_status 400 / actual_status 400:
  detail: "$.Message: value_mismatch (expected 'Cannot list accesses on server with IdP type: SERVICE_MANAGED', got 'Cannot list access on server with IdP type: SERVICE_MANAGED')"
  Expected (AWS): 400, $.Message = "Cannot list accesses on server with IdP type: <identity provider type of the named server>"
  Actual (emulator): 400, $.Message = "Cannot list access on server with IdP type: <type>" (missing the 'es' in 'accesses')

Cause 2 — fixture transfer-certificate-blob-preserved, step task#2, operation DescribeCertificate, verdict mismatch, expected_status 200 / actual_status 200:
  detail: "$.Certificate.Status: value_mismatch (expected 'ACTIVE', got 'INACTIVE')"
  Expected (AWS): 200, $.Certificate.Status = "ACTIVE" for an imported certificate with no ActiveDate/InactiveDate whose X509 NotBefore/NotAfter window contains now (Smithy DescribedCertificate.Status doc; CertificateStatusType = ACTIVE|PENDING_ROTATION|INACTIVE)
  Actual (emulator): 200, $.Certificate.Status = "INACTIVE"

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Cause 1: replay fixture transfer-access-mapping-requires-directory-idp, step task#2 — CreateServer with IdentityProviderType SERVICE_MANAGED, then call ListAccesses with that ServerId. Observe the 400 body's Message reads "Cannot list access on server with IdP type: SERVICE_MANAGED" instead of AWS's "Cannot list accesses on server with IdP type: SERVICE_MANAGED".
3. Cause 2: replay fixture transfer-certificate-blob-preserved, step task#2 — ImportCertificate (Usage SIGNING, certificate body only, no ActiveDate/InactiveDate), then DescribeCertificate with the returned CertificateId. Observe $.Certificate.Status is INACTIVE where AWS reports ACTIVE for an in-window certificate.
4. Re-replay both fixture steps after the fix; both must pass.

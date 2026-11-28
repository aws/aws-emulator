Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.delete-ca-certificate-with-active-certs-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.delete-ca-certificate-with-active-certs-rejected|run#16|AssertionError|assert err == ("CertificateStateException", f"CA Certificate must be deactivated (not ACTIVE) before deletion. Id: {ca_id}"), err
Issue-Commit: f9d4d65d80d6cf2db27f38cfacc82f6ea7294004

# iot DeleteCACertificate's CertificateStateException message drops the "CA " qualifier (says "Certificate must be deactivated..." where AWS says "CA Certificate must be deactivated...")

> _Note: this issue was identified against the code base at commit `f9d4d65d80d6cf2db27f38cfacc82f6ea7294004`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCACertificate

## Description
1 cause.

Cause 1 of 1: refusing a DeleteCACertificate on a CA certificate that is still ACTIVE, the emulator returns the right error class but the device-certificate wording instead of the CA-certificate wording — the leading "CA " qualifier is missing from the message's fixed text. Failing check: iot.delete-ca-certificate-with-active-certs-rejected.

AWS: Smithy (com.amazonaws.iot#AWSIotService): DeleteCACertificate DELETE /cacertificate/{certificateId}, certificateId @httpLabel @required @length(min 64, max 64) @pattern ^(0x)?[a-fA-F0-9]+$, empty output; modeled errors include CertificateStateException (client, httpError 406), ResourceNotFoundException (404), InvalidRequestException (400). Deleting a CA certificate whose status is ACTIVE is refused with HTTP 406, error code CertificateStateException and the message "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>". In that message the fixed text is the whole sentence "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: " — the only variable part is the certificateId taken from the request path. The CA plane's wording is deliberately distinct from the device-certificate plane's DeleteCertificate refusal, which omits the "CA " qualifier.

Emulator: the status and the error code are correct (CertificateStateException), but the message is "Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>" — the device-certificate sentence, with the "CA " qualifier absent from the fixed text. The replayed check asserted (code, message) and got ('CertificateStateException', 'Certificate must be deactivated (not ACTIVE) before deletion. Id: <64-hex CA id from the request>').

Why: a client that classifies or logs refusals by their message text (error-classification shims, log scrapers, assertion suites that compare the whole sentence, and anything that distinguishes a CA-certificate state refusal from a device-certificate state refusal because the two are remediated differently — UpdateCACertificate vs UpdateCertificate) cannot tell which plane refused it, and whole-message comparisons against AWS fail outright. The rejection itself is correct, so the defect is purely the wording the client is handed.

Fix: the DeleteCACertificate handler's refusal construction for the still-ACTIVE precondition (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_ca_certificate.rs) — it must emit the CA-plane sentence, not reuse the device-certificate message builder shared with DeleteCertificate.

Related but NOT covering this: open issue 20261118T071347Z-iot-ca-certificate-plane-diverges-updatecacertificate-never-applies.md covers the CA plane's UpdateCACertificate no-op, the delete-leaves-CA-readable cause and the ResourceNotFoundException wording, but says nothing about the CertificateStateException text; open issue 20261118T124115Z-iot-deletecacertificate-stub.md was filed when DeleteCACertificate was an unimplemented scaffold and owns only the sibling check iot.delete-ca-certificate-active-state-rejected's replay key. The operation now answers (it returns a real CertificateStateException), and the replay reports this check as unowned.

Traces: iot.delete-ca-certificate-with-active-certs-rejected (the sibling iot.delete-ca-certificate-active-state-rejected shows the same wording divergence but is already owned, so it is not claimed here).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check iot.delete-ca-certificate-with-active-certs-rejected (verdict: mismatch):

  File "check_iot_delete_ca_certificate_with_active_certs_rejected.py", line 50, in run
    assert err == ("CertificateStateException", f"CA Certificate must be deactivated (not ACTIVE) before deletion. Id: {ca_id}"), err
  AssertionError: ('CertificateStateException', 'Certificate must be deactivated (not ACTIVE) before deletion. Id: 6a6371b17285479e69a83c21bf3ed17d1f8167e643abdd005bdf01af42ea7b71')

Expected (AWS): error code CertificateStateException (HTTP 406) with message
  "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>"
Actual (emulator): error code CertificateStateException with message
  "Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>"

Fixed text vs variable text: the fixed part of AWS's message is "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: "; the only variable part is the certificateId supplied in the request URI (the 64-hex id above is one run's value, not part of the requirement). The emulator's fixed text differs from AWS's by the missing leading "CA ".

Model citation: DeleteCACertificate DELETE /cacertificate/{certificateId} -> empty output; errors CertificateStateException(406), ResourceNotFoundException(404), InvalidRequestException(400), UnauthorizedException(401), ThrottlingException(400), InternalFailureException(500), ServiceUnavailableException(503).

The same check's surrounding legs (register CA SNI_ONLY + setAsActive, register a leaf signed by it, DescribeCACertificate reporting ACTIVE, DescribeCertificate reporting the leaf's caCertificateId) all passed up to this assertion, so the state and the rejection decision are right and only the message text diverges.

## Steps to Reproduce
1. Boot the iot emulator.
2. RegisterCACertificate with a self-signed CA PEM, certificateMode=SNI_ONLY, setAsActive=true, allowAutoRegistration=false; keep the returned certificateId.
3. (Optional, as in the check) RegisterCertificate for a leaf PEM signed by that CA with status=ACTIVE.
4. Confirm DescribeCACertificate reports certificateDescription.status = ACTIVE.
5. Call DeleteCACertificate with that certificateId.
   AWS: HTTP 406, Error.Code 'CertificateStateException', Error.Message 'CA Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>'.
   Emulator: Error.Code 'CertificateStateException', Error.Message 'Certificate must be deactivated (not ACTIVE) before deletion. Id: <certificateId>' — the 'CA ' qualifier is missing.
6. Re-replay the behavioral check iot.delete-ca-certificate-with-active-certs-rejected to confirm the fix.

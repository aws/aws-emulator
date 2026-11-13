Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip|_poll_until#12|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# Connector Endpoint host is not routed at all: a SCEP GetCACert request to the connector's HTTPS SCEP URL answers 404 UnknownService instead of 200 with the configured CA certificate

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCACert

## Description
1 net-new root cause.

Related open issue (NOT marked as owning this check in this replay, and its recorded symptom is a different one — a 60s check timeout with no emulator response quoted): 20261103T120430Z-pca-connector-scep-connector-endpoint-serves-no-scep-native-protocol-getcacert.md. This filing records the concrete, now-reachable emulator response for the same surface: the connector endpoint host is rejected by the emulator's service router with a 404 UnknownService naming the connector id as an unknown service. If that issue is fixed, this one is its verification; if it is still open, the two should be closed together.

Cause 1 of 1: an ACTIVE connector reports a `Connector.Endpoint` HTTPS SCEP URL, but the emulator's front door does not recognise that host as belonging to the pca-connector-scep service at all. A SCEP `GetCACert` GET to that URL is answered with HTTP 404 and a JSON error `{"__type":"UnknownService","message":"no emulated service is registered for '<connector-id>'"}` — the router has parsed the leading label of the endpoint host (the connector's UUID) as a service token and found no service for it. No SCEP response is ever produced, so the poll for a 200 runs to its 90s limit. Failing check id: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip.

AWS: Smithy: `com.amazonaws.pcaconnectorscep#Connector$Endpoint` (target `String`) — "The connector's HTTPS public SCEP URL"; `Connector` doc — "Connector for SCEP is a service that links Amazon Web Services Private Certificate Authority to your SCEP-enabled devices. The connector brokers the exchange of certificates from Amazon Web Services Private CA to your SCEP-enabled devices and mobile device management systems." The SCEP operations are NOT modeled operations of pca-connector-scep — they are the native RFC 8894 protocol served over that Endpoint URL, so for the protocol itself this is recorded behavior only: a GET to `<Endpoint>?operation=GetCACert` on an ACTIVE connector answers HTTP 200 with a non-empty body that is either a single DER certificate (Content-Type `application/x-x509-ca-cert`) or a DER PKCS#7 degenerate certificates-only structure (Content-Type `application/x-x509-ca-ra-cert`), and that body contains the certificate of the private CA named by the connector's `CertificateAuthorityArn` — same subject and same serial number as the certificate ACM PCA holds for that CA. The requirement is "200 plus the configured CA certificate is present"; the concrete endpoint host, the connector id inside it, and the CA certificate's own subject and serial number are per-run values and are not part of the requirement.

Emulator: the control plane works — CreateConnector succeeds, GetConnector settles the connector to ACTIVE and returns an Endpoint. But every GET to `<Endpoint>?operation=GetCACert` is refused by the router with HTTP 404, Content-Type `application/x-amz-json-1.1`, `__type` `UnknownService` and the message `no emulated service is registered for '<connector-id>'` (the recorded run shows the connector's UUID `ff4d96f1-3b7d-46cf-add1-4bb1a32bd399` in that slot; the fixed text is "no emulated service is registered for '" + the host label + "'", and the id is a per-run value from the connector just created). The status 404, the `UnknownService` code and that message form are the divergence: AWS answers 200 with a certificate body, and there is no AWS error code `UnknownService` for this surface at all. Because the response is a stable 404 rather than a transient one, the check's `_poll_until(..., timeout_s=90, interval_s=5)` exhausts its budget and fails with `AssertionError: condition not met within 90s`.

Why: a client that follows the documented flow — create and activate a private CA, share it with the pca-connector-scep service principal, CreateConnector, read `Connector.Endpoint`, then point a SCEP-enabled device or an MDM (Intune, Jamf) at that URL — gets a connector that reports itself healthy and ACTIVE but can never enroll anything. GetCACert, and by extension GetCACaps and PKIOperation, never return a certificate, so no device can obtain the CA cert or complete enrollment. Worse, the error it does get back is an emulator-internal routing error with a non-AWS error code, so SCEP clients and MDM integrations see an unparseable JSON blob where DER/PKCS#7 bytes belong, and any code that retries on a 404 spins until its own deadline instead of failing with a recognisable AWS error.

Fix: this needs a SCEP data-plane surface for the connector endpoint plus the routing seam that reaches it; it is not a control-plane tweak. Two parts: (a) the request-claiming/routing layer must recognise a request whose host is a connector's own Endpoint host as belonging to this service instead of treating the host's leading label as a service token — the service's `can_handle`/`override_operation` seam in the pcaconnectorscep crate's service entry point, together with whatever front-door host-to-service resolution emits `UnknownService`; and (b) a native-protocol handler beside the generated modeled-operation table that implements at least `operation=GetCACert`, resolving the connector by the id embedded in the endpoint host, requiring the connector to be ACTIVE, and serving the certificate currently held by the ACM PCA CA named by that connector's `CertificateAuthorityArn` as DER (or a DER PKCS#7 certificates-only blob) with the matching Content-Type. A connector that is not ACTIVE, or an endpoint host naming no connector, must answer a non-2xx of its own rather than a certificate — and not the generic `UnknownService` routing error.

Traces: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (mismatch, `AssertionError: condition not met within 90s (last=(404, 'application/x-amz-json-1.1', b'{"__type":"UnknownService","message":"no emulated service is registered for ..."}'))`).

## Evidence
Check: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (GROUP native-protocol, tags axis:happy-path, cap:c1, cap:c4, concern:data-plane, dp:endpoint, plane:data). Suite service heading: ### pca-connector-scep. Suite run: 12/21 passed; this is the only failing check in the run not already marked as filed/owned.

Expected (AWS): after CreateConnector against an ACTIVE, shared private CA, `GetConnector(ConnectorArn=...)["Connector"]["Endpoint"]` is the connector's HTTPS public SCEP URL, and `GET <Endpoint>?operation=GetCACert` answers HTTP 200 with a non-empty body; parsed as DER X.509 (plain CA-cert Content-Type) or DER PKCS#7 (Content-Type containing `ca-ra-cert`/`pkcs7`), that body yields a certificate whose subject and serial number equal those of the CA certificate the connector's `CertificateAuthorityArn` refers to.

Actual (emulator): the connector does reach ACTIVE and does report an Endpoint, but every GET to that Endpoint with `?operation=GetCACert` is answered:

    HTTP 404
    Content-Type: application/x-amz-json-1.1
    {"__type":"UnknownService","message":"no emulated service is registered for 'ff4d96f1-3b7d-46cf-add1-4bb1a32bd399'"}

so the check's poll for a 200 never succeeds and it fails with:

    AssertionError: condition not met within 90s (last=(404, 'application/x-amz-json-1.1', b'{"__type":"UnknownService","message":"no emulated service is registered for \'ff4d96f1-3b7d-46cf-add1-4bb1a32bd399\'"}'))

Message anatomy: fixed text `no emulated service is registered for '` ... `'`; the quoted value is the leading label of the connector Endpoint host, i.e. the connector id from the CreateConnector just performed (per-run). The diverging requirements are the HTTP status (404 vs 200), the error code (`UnknownService`, which is not an AWS error for this surface, vs no error at all), and the body (a JSON routing error vs DER/PKCS#7 certificate bytes).

Model citation used: `com.amazonaws.pcaconnectorscep#Connector$Endpoint` — "The connector's HTTPS public SCEP URL" (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json). SCEP's GetCACert is not a modeled operation, so the protocol expectation is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2 in the recording; any region reproduces).
2. acm-pca: CreateCertificateAuthority (ROOT, RSA_2048 / SHA256WITHRSA), wait for PENDING_CERTIFICATE, GetCertificateAuthorityCsr, IssueCertificate with TemplateArn arn:aws:acm-pca:::template/RootCACertificate/V1 and Validity 10 YEARS, GetCertificate, ImportCertificateAuthorityCertificate, wait for ACTIVE. Keep the returned CA certificate PEM.
3. ram: CreateResourceShare for that CA ARN with principal pca-connector-scep.amazonaws.com and permission arn:aws:ram::aws:permission/AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority; wait until the share is ACTIVE.
4. pca-connector-scep: CreateConnector(CertificateAuthorityArn=<ca arn>); poll GetConnector until Connector.Status == ACTIVE and read Connector.Endpoint.
5. Issue an unsigned HTTP GET to `<Endpoint>?operation=GetCACert`, routed to the emulator.
6. Observe HTTP 404 with body {"__type":"UnknownService","message":"no emulated service is registered for '<connector-id>'"} — the endpoint host's leading label was resolved as a service token. Expected: HTTP 200 whose DER or PKCS#7 body contains the certificate of the CA from step 2 (same subject and same serial number).

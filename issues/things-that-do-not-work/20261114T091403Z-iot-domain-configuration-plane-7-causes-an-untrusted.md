Issue-Source: trace-sweep-divergence
Issue-Trace: iot-domain-configuration-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-domain-configuration-lifecycle iot-domain-configuration-self-signed-validation-cert-rejected iot-domain-configuration-with-custom-authorizer iot-domain-configuration-error-paths iot-domain-configuration-inspection iot-domain-configuration-invalid-certificate-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateDomainConfiguration|4xx->2xx|$.message
Issue-Key: iot|CreateDomainConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateDomainConfiguration|4xx->2xx|status
Issue-Key: iot|DescribeDomainConfiguration|4xx->2xx|$.message
Issue-Key: iot|DescribeDomainConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeDomainConfiguration|4xx->2xx|status
Issue-Key: iot|DescribeDomainConfiguration|4xx->4xx|$.message
Issue-Key: iot|DeleteDomainConfiguration|2xx->4xx|$
Issue-Key: iot|DeleteDomainConfiguration|2xx->4xx|status
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.applicationProtocol
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.authenticationType
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.domainConfigurationArn
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.domainConfigurationName
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.domainConfigurationStatus
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.domainName
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.domainType
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.lastStatusChangeDate
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.serverCertificates
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.serviceType
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|$.tlsConfig
Issue-Key: iot|DescribeDomainConfiguration|2xx->4xx|status
Issue-Key: iot|DescribeDomainConfiguration|2xx->2xx|$.applicationProtocol
Issue-Key: iot|DescribeDomainConfiguration|2xx->2xx|$.authenticationType
Issue-Key: iot|DescribeDomainConfiguration|2xx->2xx|$.domainConfigurationArn
Issue-Key: iot|DescribeDomainConfiguration|2xx->2xx|$.domainName
Issue-Key: iot|DescribeDomainConfiguration|2xx->2xx|$.tlsConfig
Issue-Key: iot|DeleteDomainConfiguration|4xx->2xx|status
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot domain-configuration plane: 7 causes — an untrusted validation certificate is accepted, the built-in iot:Data-ATS configuration is absent, and the describe projection drops modeled members

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainConfiguration

## Description
Seven net-new wire divergences in the IoT domain-configuration plane: a create that AWS refuses is accepted, a delete AWS refuses is accepted, the account's built-in data endpoint configuration does not exist, the describe projection omits modeled members and builds the ARN from a UUID, and two refusal paths diverge. Seven causes must be closed.

Cause 1 of 7: CreateDomainConfiguration accepts a validationCertificateArn whose certificate is not signed by a trusted CA.
  AWS: Smithy: CreateDomainConfiguration -> POST /domainConfigurations/{domainConfigurationName}; validationCertificateArn is an AcmCertificateArn; modeled errors include CertificateValidationException (400) and InvalidRequestException (400). Recorded response: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and message 'Validation certificate must be signed by trusted CA' — entirely fixed text, no variable part. AWS validates the certificate's chain before creating the configuration.
  Emulator: 200 and a created domain configuration; no message, no x-amzn-errortype.
  Why: a caller is told a custom domain is live when AWS would have refused it, so the operator never learns the certificate is self-signed/untrusted; and the account holds a configuration AWS would not have created, which is what makes causes 2 and 7 diverge.
  Fix: CreateDomainConfiguration's validation path — a certificate-chain pre-flight ahead of state insertion.
  Traces: iot-domain-configuration-lifecycle#task#4 (representative), plus iot-domain-configuration-self-signed-validation-cert-rejected#task#3 and iot-domain-configuration-with-custom-authorizer#task#1 in the same cluster. Related open issue 20261114T082236Z-iot-control-plane-gaps-9-causes-untrusted-domain.md reports untrusted domain-configuration certificates being accepted from the BEHAVIORAL corpus; it carries no wire fixture key, so this replay step is not gated by it.

Cause 2 of 7: DescribeDomainConfiguration answers 200 for a configuration whose create AWS refused.
  AWS: Smithy: DescribeDomainConfiguration -> GET /domainConfigurations/{domainConfigurationName}, errors include ResourceNotFoundException (404). Recorded response: 404, x-amzn-errortype 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', message 'Could not find domain configuration for given account ID and domain configuration name, AccountId: ' + <caller's account id> + ', DomainConfigurationName: ' + <the requested name>.
  Emulator: 200 with a configuration projection.
  Fix: the DescribeDomainConfiguration handler's lookup path (and cause 1, which is why the record exists).
  Traces: iot-domain-configuration-lifecycle#task#5, iot-domain-configuration-self-signed-validation-cert-rejected#task#4.

Cause 3 of 7: DescribeDomainConfiguration's not-found message is emulator-authored.
  AWS: recorded response: 404 ResourceNotFoundException with 'Could not find domain configuration for given account ID and domain configuration name, AccountId: <account>, DomainConfigurationName: <name>'. Fixed text: 'Could not find domain configuration for given account ID and domain configuration name, AccountId: ' and ', DomainConfigurationName: '; variable parts: the caller's account id and the requested name.
  Emulator: 404 ResourceNotFoundException (status and code correct) with 'Domain configuration with name [<name>] not found'.
  Why: the message is the only part of the refusal naming the account and the resource in AWS's own form; clients matching it do not match, and the account id is dropped.
  Fix: the DescribeDomainConfiguration handler's not-found message.
  Traces: iot-domain-configuration-error-paths#task#1 (cluster of 6: also iot-domain-configuration-error-paths#task#3, iot-domain-configuration-inspection#task#4, iot-domain-configuration-invalid-certificate-rejected#task#2, iot-domain-configuration-lifecycle#task#3, iot-domain-configuration-with-custom-authorizer#task#3). Open issue 20261114T082236Z-iot-refusal-message-fidelity-7-causes-addthingtothinggroup-createstream.md describes the same wording from a behavioral check and carries no wire key for it.

Cause 4 of 7: DeleteDomainConfiguration refuses a name that does not exist instead of answering success.
  AWS: recorded behavior only — Smithy models ResourceNotFoundException (404) on DeleteDomainConfiguration, but the recorded response for a never-created name is 200 with an empty body.
  Emulator: 404 with {'__type': 'ResourceNotFoundException', 'message': 'Domain configuration with name [<name>] not found'}.
  Why: teardown and idempotent-delete code that AWS lets succeed fails against the emulator, so a cleanup path aborts.
  Fix: the DeleteDomainConfiguration handler's unknown-name path.
  Traces: iot-domain-configuration-error-paths#task#2, iot-domain-configuration-with-custom-authorizer#task#2.

Cause 5 of 7: the account's built-in data domain configuration 'iot:Data-ATS' does not exist.
  AWS: Smithy: DescribeDomainConfiguration's input member targets ReservedDomainConfigurationName (pattern ^[\w.:-]+$, which admits the ':' of the reserved names), so the reserved AWS-managed configurations are addressable. Recorded response for 'iot:Data-ATS': 200 with domainConfigurationName 'iot:Data-ATS', domainType 'ENDPOINT', serviceType 'DATA', domainConfigurationStatus 'ENABLED', authenticationType 'DEFAULT', applicationProtocol 'DEFAULT', serverCertificates [], tlsConfig {securityPolicy: 'IoTSecurityPolicy_TLS13_1_2_2022_10'}, a domainConfigurationArn of the form arn:aws:iot:<region>:<account>:domainconfiguration/iot:Data-ATS, a domainName (the account's data endpoint host) and a lastStatusChangeDate.
  Emulator: 404 — every one of those members is absent.
  Why: a client enumerating or inspecting the account's managed data endpoint (to read the active security policy or the endpoint host) gets a not-found for a resource that always exists in a real account.
  Fix: the domain-configuration state — seed the reserved ENDPOINT configurations ('iot:Data-ATS' and siblings) at account initialization, and let DescribeDomainConfiguration resolve reserved names.
  Traces: iot-domain-configuration-inspection#task#3. (The recorded domainName/lastStatusChangeDate are per-environment values; the requirement is that the record exists with the modeled members and the reserved-name ARN form, not those literals.)

Cause 6 of 7: the DescribeDomainConfiguration projection omits applicationProtocol, authenticationType, tlsConfig and domainName, and derives the ARN from a generated UUID instead of the configuration name.
  AWS: Smithy: DescribeDomainConfigurationResponse models domainConfigurationName, domainConfigurationArn, domainName, serverCertificates, authorizerConfig, domainConfigurationStatus, serviceType, domainType, lastStatusChangeDate, tlsConfig, serverCertificateConfig, authenticationType, applicationProtocol, clientCertificateConfig. Recorded response: applicationProtocol 'DEFAULT', authenticationType 'DEFAULT', tlsConfig {securityPolicy: 'IoTSecurityPolicy_TLS13_1_2_2022_10'}, a non-null domainName, and a domainConfigurationArn whose resource part is the domain configuration NAME (arn:aws:iot:<region>:<account>:domainconfiguration/<domainConfigurationName>).
  Emulator: those four members are absent, and domainConfigurationArn is arn:aws:iot:<region>:<account>:domainconfiguration/<a random UUID> — the resource part is a generated identifier, not the name.
  Why: a client reading the effective TLS security policy or authentication type sees nothing, and any code that derives or matches the configuration's ARN from its name (IAM resource statements, tagging) builds an ARN the emulator will never report.
  Fix: the DescribeDomainConfiguration serializer (default the four members on create/describe) and the ARN construction in the domain-configuration state.
  Traces: iot-domain-configuration-invalid-certificate-rejected#task#4.

Cause 7 of 7: DeleteDomainConfiguration succeeds on a configuration that is not disabled.
  AWS: Smithy: DeleteDomainConfiguration -> DELETE /domainConfigurations/{domainConfigurationName}, errors include InvalidRequestException (400). Recorded response: 400 — AWS refuses to delete a domain configuration while its domainConfigurationStatus is ENABLED; it must be set to DISABLED with UpdateDomainConfiguration first. The recording asserts the status only (no message body was captured), so the requirement here is the 400 refusal class.
  Emulator: 200, deleting the ENABLED configuration.
  Why: a client is never told it must disable before deleting, so a flow that relies on that guard (and on the two-step disable/delete) silently diverges, and a live custom domain can be removed in one call.
  Fix: the DeleteDomainConfiguration handler's status guard.
  Traces: iot-domain-configuration-invalid-certificate-rejected#teardown#6.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. All seven clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-domain-configuration-lifecycle#task#4 (CreateDomainConfiguration):
  expected status 400, got 200
  $.message: missing (expected 'Validation certificate must be signed by trusted CA', got None)
  $headers.x-amzn-errortype: missing (expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got None)
  also affected: iot-domain-configuration-self-signed-validation-cert-rejected#task#3, iot-domain-configuration-with-custom-authorizer#task#1

Cause 2 — iot-domain-configuration-lifecycle#task#5 (DescribeDomainConfiguration):
  expected status 404, got 200
  $.message: missing (expected 'Could not find domain configuration for given account ID and domain configuration name, AccountId: <account>, DomainConfigurationName: <name>', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', got None)
  also affected: iot-domain-configuration-self-signed-validation-cert-rejected#task#4

Cause 3 — iot-domain-configuration-error-paths#task#1 (DescribeDomainConfiguration), status 404 as expected:
  $.message: expected 'Could not find domain configuration for given account ID and domain configuration name, AccountId: <account>, DomainConfigurationName: <name>', got 'Domain configuration with name [<name>] not found'
  cluster of 6 steps across 6 fixtures.

Cause 4 — iot-domain-configuration-error-paths#task#2 (DeleteDomainConfiguration):
  expected status 200, got 404
  $: expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'Domain configuration with name [<name>] not found'}
  also affected: iot-domain-configuration-with-custom-authorizer#task#2

Cause 5 — iot-domain-configuration-inspection#task#3 (DescribeDomainConfiguration on 'iot:Data-ATS'):
  expected status 200, got 404; every member missing: applicationProtocol 'DEFAULT', authenticationType 'DEFAULT', domainConfigurationArn 'arn:aws:iot:<region>:<account>:domainconfiguration/iot:Data-ATS', domainConfigurationName 'iot:Data-ATS', domainConfigurationStatus 'ENABLED', domainName (the account data endpoint host), domainType 'ENDPOINT', lastStatusChangeDate, serverCertificates [], serviceType 'DATA', tlsConfig {'securityPolicy': 'IoTSecurityPolicy_TLS13_1_2_2022_10'}

Cause 6 — iot-domain-configuration-invalid-certificate-rejected#task#4 (DescribeDomainConfiguration), status 200:
  $.applicationProtocol: missing (expected 'DEFAULT')
  $.authenticationType: missing (expected 'DEFAULT')
  $.tlsConfig: missing (expected {'securityPolicy': 'IoTSecurityPolicy_TLS13_1_2_2022_10'})
  $.domainName: missing (expected a shaped value)
  $.domainConfigurationArn: shape mismatch — got 'arn:aws:iot:us-west-2:<account>:domainconfiguration/22f1df95-6c6e-4fe5-b20a-f14c1fa73810' where the recorded shape is .../domainconfiguration/<domainConfigurationName>

Cause 7 — iot-domain-configuration-invalid-certificate-rejected#teardown#6 (DeleteDomainConfiguration):
  expected status 400, got 200

Account ids, regions, hostnames, the UUID and the probe-generated configuration names above are per-run values; the requirements are the statuses, error codes, the fixed text of each message with the request/state values it interpolates, the modeled members, and the name-based ARN form.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — CreateDomainConfiguration (POST /domainConfigurations/{domainConfigurationName}; CertificateValidationException 400, InvalidRequestException 400), DescribeDomainConfiguration (GET; input ReservedDomainConfigurationName pattern ^[\w.:-]+$; output members listed above; ResourceNotFoundException 404), DeleteDomainConfiguration (DELETE; InvalidRequestException 400, ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: CreateDomainConfiguration(domainConfigurationName=D, domainName=<custom host>, validationCertificateArn=<an ACM cert that is self-signed / not chained to a trusted CA>) — expect 400 InvalidRequestException 'Validation certificate must be signed by trusted CA'.
3. Cause 2: after that refusal, DescribeDomainConfiguration(D) — expect 404.
4. Cause 3: DescribeDomainConfiguration(<a name never created>) and compare Error.Message with AWS's text.
5. Cause 4: DeleteDomainConfiguration(<a name never created>) — expect 200 and an empty body.
6. Cause 5: DescribeDomainConfiguration('iot:Data-ATS') — expect 200 with domainType ENDPOINT, serviceType DATA, status ENABLED, tlsConfig.securityPolicy and the reserved-name ARN.
7. Cause 6: create a domain configuration that AWS would accept, then DescribeDomainConfiguration on it and check applicationProtocol, authenticationType, tlsConfig, domainName and the ARN's resource part.
8. Cause 7: with that configuration in ENABLED status, call DeleteDomainConfiguration — expect 400.
9. Re-replay the representative step iot-domain-configuration-lifecycle#task#4 and every fixture#step in trace_ids.

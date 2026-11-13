Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates
Issue-Key: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates|_poll_request#44|Timeout|time.sleep(interval_s)
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# acm__apigateway__s3: the mutual-TLS regional custom domain never becomes observable — the check is killed at the replay budget with no verdict

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
1 net-new root cause. Two earlier causes of this same check were filed and resolved (20261109T132410Z-acm-apigateway-s3-a-dns-validated-acm-certificate.md — the DNS-validated certificate ended FAILED; 20261111T070023Z-acm-apigateway-s3-a-regional-custom-domain-publishes.md — regionalDomainName was published on a real-AWS host). Those are resolved and cover nothing here: the check now gets further and stops producing any answer at all.

Cause 1 of 1: the mutual-TLS custom-domain flow never reaches a terminal observation inside the run budget.
AWS: a REGIONAL custom domain created with MutualTlsAuthentication (a truststore object in S3) becomes reachable, presents the ACM certificate for its DomainName, completes a TLS handshake with a client certificate the truststore trusts, and REFUSES a handshake that presents none — with alert TLSV13_ALERT_CERTIFICATE_REQUIRED (TLS 1.3) or SSLV3_ALERT_HANDSHAKE_FAILURE (TLS 1.2). Every one of those steps answers (success or refusal) rather than hanging. Smithy: CreateDomainName input MutualTlsAuthentication{TruststoreUri, TruststoreVersion}, EndpointConfiguration; DomainName members DomainNameStatus, RegionalDomainName. The handshake outcomes are recorded behavior only.
Emulator: the check is killed by the 60 s replay budget ('Check timed out after 60.0s'), so no status, certificate or handshake result is ever observed. The check's own waits are budget-aware (they would report 'the run budget ended first; not graded'), so the budget was consumed inside a call or socket operation that never returned — the domain's TLS listener accepts but never completes or refuses a handshake, or a status poll never converges (recorded behavior only).
Why: a client that creates a mutual-TLS custom domain and then connects to it neither gets bytes nor an error — it hangs. A hung TLS endpoint is worse than a refusal: nothing distinguishes 'not ready' from 'client certificate rejected', so the truststore's whole purpose is unobservable.
Fix: the regional custom-domain data plane in the apigateway service — the domain must reach a terminal DomainNameStatus and its listener must complete a trusted-client handshake and refuse an untrusted/absent client certificate with a TLS alert instead of leaving the connection open.
Traces: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates.

## Evidence
Cause 1: Error: Check timed out after 60.0s — with no status, no handshake result and no refusal recorded. The check's polls use budget-aware helpers that raise 'the run budget ended first; not graded' instead of being killed, so the 60 s was spent inside a single blocking step.

## Steps to Reproduce
Cause 1: request and validate an ACM certificate for a domain in the account's DNS baseline zone; upload a CA bundle to S3 as a truststore; CreateDomainName with EndpointConfiguration REGIONAL, that certificate and MutualTlsAuthentication{TruststoreUri}; then open a TLS connection to the published regional domain with (a) a client certificate the truststore trusts and (b) no client certificate, and observe that neither completes nor is refused.

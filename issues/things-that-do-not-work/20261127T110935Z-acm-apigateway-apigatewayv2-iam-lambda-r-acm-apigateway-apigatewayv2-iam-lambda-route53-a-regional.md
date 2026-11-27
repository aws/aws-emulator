Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway__apigatewayv2__iam__lambda__route53.rest-and-http-api-custom-domains-route-mapped-paths-to-iam-authorized-routes
Issue-Key: acm__apigateway__apigatewayv2__iam__lambda__route53.rest-and-http-api-custom-domains-route-mapped-paths-to-iam-authorized-routes|_poll_until#44|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# acm__apigateway__apigatewayv2__iam__lambda__route53: a REGIONAL custom domain's regionalDomainName is an IP:port literal, not a DNS name, so nothing can be pointed at the regional endpoint

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Cause 1 of 1: the regional endpoint host published for a custom domain is an address literal with a port instead of a resolvable hostname.

AWS: apigateway CreateDomainName/GetDomainName for a REGIONAL endpoint returns `regionalDomainName`, documented in the model as "The domain name associated with the regional endpoint for this custom domain name. You set up this association by adding a DNS record that points the custom domain name to this regional domain name. The regional domain name is returned by API Gateway when you create a regional endpoint." (Smithy: apigateway DomainName.regionalDomainName; apigatewayv2 DomainNameConfiguration.ApiGatewayDomainName, "A domain name for the API", is the same kind of value.) It is therefore a DNS hostname — no port, no address literal — that lives in a zone which answers it, so a caller can create the CNAME/alias AWS instructs it to create and can resolve the regional endpoint.

Emulator: `regionalDomainName` is returned as an address-and-port literal of the form `<ip>:<port>` (recorded: '127.0.0.1:45057'). That is not a domain name: it is in no zone, so a lookup of the zone serving it never succeeds and a DNS query for it can never be formed. A client that waits for the regional host to become resolvable before using the custom domain waits forever; the recorded run exhausts its budget inside the bounded poll "the name servers of '<host>''s zone" and raises TimeoutError. The host's address and port are per-environment values, but a member the model declares to be a domain name must be a hostname, not an address with a port.

Why: the published regional domain name is the one piece of data AWS gives a caller to wire a custom domain up. A CNAME or Route 53 alias target may not be an IP address and may not carry a port, so no caller can perform the step the model documents; REST and HTTP API custom domains, their base-path/API mappings, and anything resolved through them cannot be exercised at all, and a client ported from AWS fails at DNS rather than at an API call it can diagnose.

Fix: the apigateway CreateDomainName/GetDomainName handlers (and apigatewayv2 GetDomainName's DomainNameConfigurations[].ApiGatewayDomainName) together with the DNS responder — publish a hostname that the emulator's DNS plane answers for the regional endpoint, and carry the listening port out of band rather than inside the domain-name member.

Related: open issue 20261126T022300Z-acm-apigateway-apigatewayv2-iam-lambda-r-acm-apigateway-apigatewayv2-iam-lambda-route53-a-signed.md covers the same scenario family (a signed GET through a REGIONAL REST custom domain's base path mapping is never served) but does not carry this check's replay key.

Traces: acm__apigateway__apigatewayv2__iam__lambda__route53.rest-and-http-api-custom-domains-route-mapped-paths-to-iam-authorized-routes

## Evidence
Expected: apigateway GetDomainName(...)['regionalDomainName'] is a DNS hostname for the regional endpoint (AWS shape: d-<id>.execute-api.<region>.amazonaws.com) — no port, resolvable, served by a zone; the same for apigatewayv2's DomainNameConfigurations[0].ApiGatewayDomainName.
Actual: the value is an address-and-port literal; the run recorded '127.0.0.1:45057'. Looking up the zone that serves it never returns, and the bounded poll raises verbatim:
  TimeoutError: the name servers of '127.0.0.1:45057''s zone: the run budget ended first; not graded
raised from _wait_published -> _poll_until. The preceding control-plane calls succeeded: the ACM certificate reached ISSUED, the REST domain was created and the HTTP API custom domain reached DomainNameStatus AVAILABLE. The specific IP and port are environment values; the divergence is that a domain-name member carries an address and a port and is in no zone.

## Steps to Reproduce
1. acm RequestCertificate for a domain and get it to ISSUED.
2. apigateway CreateDomainName with endpointConfiguration types=['REGIONAL'] and that certificate; apigatewayv2 CreateDomainName likewise and wait for DomainNameStatus AVAILABLE.
3. Read apigateway GetDomainName(...)['regionalDomainName'] and apigatewayv2 GetDomainName(...)['DomainNameConfigurations'][0]['ApiGatewayDomainName'].
4. Observe a value of the form <ip>:<port>. Try to find the hosted zone that serves that host, or to use it as a CNAME/alias target as the model instructs: neither is possible. AWS returns a resolvable execute-api hostname with no port.

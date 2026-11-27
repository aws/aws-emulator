Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__ec2__elbv2.vpc-link-integration-load-balancer-arn-rejected-listener-arn-serves
Issue-Key: apigatewayv2__ec2__elbv2.vpc-link-integration-load-balancer-arn-rejected-listener-arn-serves|_refusal#26|AssertionError|raise AssertionError(f"the call was expected to be refused, but it returned {result!r}")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# apigatewayv2__ec2__elbv2: CreateIntegration accepts a load balancer ARN as a VPC_LINK IntegrationUri instead of refusing it with BadRequestException

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Cause 1 of 1: a VPC_LINK private integration whose IntegrationUri names a load balancer (not a listener) is accepted.
AWS: documented HTTP API private-integration rule (http-api-develop-integrations-private) — for ConnectionType VPC_LINK the IntegrationUri must be an ELB *listener* ARN or a Cloud Map service ARN. A load balancer ARN is refused with HTTP 400 BadRequestException and the message `For VpcLink VPC_LINK, integration uri should be a valid ELB listener ARN or a valid Cloud Map service ARN.` (that text is fixed; nothing from the request is interpolated). The refused call leaves the API with no integration, and the same call naming the listener ARN is accepted and serves the listener's response through the HTTP API.
Emulator: the call succeeds with HTTP 201 and stores the integration with IntegrationUri set to the load balancer ARN (ConnectionType VPC_LINK, IntegrationType HTTP_PROXY), so GetIntegrations then lists an integration AWS would never have created.
Why: a client wiring a private integration gets a silent acceptance of a URI AWS rejects; the mistake surfaces only in production, and the client's negative control for this misconfiguration passes against the emulator.
Fix: the CreateIntegration handler's IntegrationUri validation for ConnectionType VPC_LINK (same validation applies on UpdateIntegration).
Traces: apigatewayv2__ec2__elbv2.vpc-link-integration-load-balancer-arn-rejected-listener-arn-serves.

## Evidence
`AssertionError: the call was expected to be refused, but it returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'ConnectionType': 'VPC_LINK', 'IntegrationType': 'HTTP_PROXY', 'IntegrationUri': 'arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/<name>/<id>', 'PayloadFormatVersion': '1.0', 'TimeoutInMillis': 30000}`. Expected: HTTP 400, code BadRequestException, message `For VpcLink VPC_LINK, integration uri should be a valid ELB listener ARN or a valid Cloud Map service ARN.` (the ARNs and ids above are per-run values).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create an internal ALB with an HTTP listener serving a fixed response, create an apigatewayv2 VPC link over the same subnets, and create an HTTP API.
3. Call CreateIntegration(ApiId=<api>, IntegrationType='HTTP_PROXY', IntegrationMethod='GET', ConnectionType='VPC_LINK', ConnectionId=<link>, IntegrationUri=<load balancer ARN>, PayloadFormatVersion='1.0') and observe HTTP 201 instead of a 400 BadRequestException; GetIntegrations then lists the integration.
4. Repeat with IntegrationUri=<listener ARN> as the positive control.

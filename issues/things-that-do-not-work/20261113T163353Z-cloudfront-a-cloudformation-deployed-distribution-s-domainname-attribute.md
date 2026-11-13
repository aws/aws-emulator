Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deploy-cloudfront-distribution-serves-origin-content cloudfront.deploy-distribution-cfn-serves-live-traffic
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deploy-cloudfront-distribution-serves-origin-content|run#10|PublishedUrlRejected|edge, route = ctx.endpoint_route(domain_name)
Issue-Key: cloudfront.deploy-distribution-cfn-serves-live-traffic|run#8|PublishedUrlRejected|edge, route = ctx.endpoint_route(domain)
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# cloudfront: a CloudFormation-deployed distribution's DomainName attribute is published as <id>.cloudfront.net, a host the emulator does not serve, so the deployed edge is unreachable

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution created through a CloudFormation stack publishes its DomainName on the AWS production host suffix `.cloudfront.net`, which this emulator does not serve, so the viewer request that is the only way to exercise the deployed distribution can never be issued. Failing checks: cloudfront.deploy-cloudfront-distribution-serves-origin-content, cloudfront.deploy-distribution-cfn-serves-live-traffic.

AWS: Smithy (cloudfront 2020-05-31) models Distribution$DomainName — "The distribution's CloudFront domain name. For example: d111111abcdef8.cloudfront.net." CloudFormation's AWS::CloudFront::Distribution exposes that same value as the `DomainName` attribute (Fn::GetAtt), and a viewer uses a distribution by issuing HTTP(S) to that domain; the edge then applies the distribution config (S3 origin with OriginAccessControl signing, DefaultRootObject, CachePolicy, ViewerProtocolPolicy) and returns the origin object or CloudFront's own refusal. Recorded behavior only for the host form an emulator must publish: an address the emulator announces has to be an address it is reached on. Both recordings succeed against AWS by fetching the stack's DomainName output over HTTPS and reading back the exact bytes put into the S3 origin.

Emulator: the stack reaches CREATE_COMPLETE and its DomainName output is a generated `<id>.cloudfront.net` name (observed `https://d95fd401ff0f1b.cloudfront.net` and `https://dc8f3aab40eeb9.cloudfront.net` — the id differs per run and is not part of the requirement), which resolves to nothing the emulator listens on. The client is refused at egress before any request is sent: "simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" (raised in context.py endpoint_route/_published_target). The fixed text of that refusal is the harness's; the diverging value is the published host.

Why: a client that deploys a distribution by template and then reads Fn::GetAtt DomainName — the documented and only way to address the edge — gets an address it can never reach. Nothing downstream of the deploy is observable: origin fetch and body equality, DefaultRootObject resolution, OAC-signed S3 origin access, cache-policy keying and the redirect-to-https / allow-all viewer protocol policies. A caller also cannot distinguish "CloudFront refused the request" from "CloudFront is not there".

Fix: the DomainName member produced by the cloudfront CreateDistribution/UpdateDistribution/GetDistribution serializers and surfaced by the CloudFormation AWS::CloudFront::Distribution resource provider's DomainName attribute must publish a host the emulator actually serves, and the CloudFront edge data plane must answer viewer requests on that host for the deployed distribution.

Related open issues that the replay does NOT mark as owning these two checks (so they do not cover them): 20261109T152222Z-apigateway-cloudfront-s3-a-distribution-s-domainname-is.md (same family, apigateway__cloudfront__s3 grouping) and 20261109T120741Z-cloudfront-distribution-and-connection-group-responses-publish-aws-production.md.

Traces: cloudfront.deploy-cloudfront-distribution-serves-origin-content, cloudfront.deploy-distribution-cfn-serves-live-traffic.

## Evidence
Expected (AWS): the stack output DomainName names a host that serves the distribution, so GET https://<DomainName>/<key> returns 200 with exactly the bytes uploaded to the S3 origin (deploy-cloudfront-distribution-serves-origin-content asserts the fetched body equals the uploaded payload; deploy-distribution-cfn-serves-live-traffic asserts status 200 and body == b"sim2real-deploy-check-original-content").

Actual (emulator), identically in both checks: the fetch never leaves the client —
  simparity.published.PublishedUrlRejected: endpoint_url was given https://d95fd401ff0f1b.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:45421/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
  simparity.published.PublishedUrlRejected: endpoint_url was given https://dc8f3aab40eeb9.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:40373/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The generated distribution id and the loopback port differ per run; the defect is the published host suffix (.cloudfront.net) being an address the emulator does not serve.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::S3::Bucket, AWS::CloudFront::OriginAccessControl (OriginAccessControlOriginType s3, SigningBehavior always, SigningProtocol sigv4), AWS::CloudFront::Distribution over that bucket's RegionalDomainName with a managed CachePolicyId, and AWS::S3::BucketPolicy granting cloudfront.amazonaws.com s3:GetObject; export Outputs BucketName: !Ref <bucket> and DomainName: !GetAtt <distribution>.DomainName.
2. Wait for CREATE_COMPLETE and read the DomainName output from DescribeStacks.
3. PutObject an object into the bucket (e.g. key index.txt).
4. Issue HTTP(S) GET to https://<DomainName>/index.txt.
Expected: 200 with the object's bytes, served by the emulator's CloudFront edge. Actual: DomainName is <generated-id>.cloudfront.net, a host the emulator does not serve, so the request is refused at egress and no edge behaviour can be observed.

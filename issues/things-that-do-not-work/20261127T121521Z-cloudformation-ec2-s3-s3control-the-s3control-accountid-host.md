Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__s3__s3control.cfn-s3-access-point-adding-vpc-configuration-replaces-it-under-a-new-name cloudformation__ec2__s3__s3control.cfn-s3-access-point-empty-vpc-id-rejected-while-a-real-vpc-id-deploys
Issue-Key: cloudformation__ec2__s3__s3control.cfn-s3-access-point-adding-vpc-configuration-replaces-it-under-a-new-name|run#11|EgressRefused|before = s3control.get_access_point(AccountId=ctx.account, Name=first)
Issue-Key: cloudformation__ec2__s3__s3control.cfn-s3-access-point-empty-vpc-id-rejected-while-a-real-vpc-id-deploys|run#14|EgressRefused|ok = s3control.get_access_point(AccountId=ctx.account, Name=ok_ap)
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# cloudformation__ec2__s3__s3control: the s3control {AccountId}. host prefix is unserved, so both CFN AWS::S3::AccessPoint + VPC verification checks never reach s3control (1 cause)

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
Both cross-service checks under this heading deploy an AWS::S3::AccessPoint (with and without VpcConfiguration) and then verify the deployed resource through the s3control control plane. Neither verification call is ever delivered. This bundle has 1 cause; it must be closed.

Cause 1 of 1: the emulator serves/announces no address for the account-id-prefixed s3control host that the s3control API model mandates, so a compliant SDK request for GetAccessPoint cannot be delivered at all.
Failing check ids: cloudformation__ec2__s3__s3control.cfn-s3-access-point-adding-vpc-configuration-replaces-it-under-a-new-name, cloudformation__ec2__s3__s3control.cfn-s3-access-point-empty-vpc-id-rejected-while-a-real-vpc-id-deploys.

AWS: Smithy (com.amazonaws.s3control#AWSS3ControlServiceV20180820, restXml, sigv4Name 's3'): every account-scoped operation carries @endpoint(hostPrefix: "{AccountId}.") and binds AccountId as both @httpHeader("x-amz-account-id") and @hostLabel (target AccountId, pattern ^\d{12}$) — e.g. CreateAccessPoint (PUT /v20180820/accesspoint/{Name}, endpoint.hostPrefix "{AccountId}.") and GetAccessPoint. An AWS SDK therefore prepends the caller's 12-digit account id and a dot to the resolved endpoint authority: against real AWS the request goes to <account-id>.s3-control.<region>.amazonaws.com, and against a custom endpoint authority to <account-id>.<that authority>, which AWS answers with the operation's modeled 200 body (Bucket, NetworkOrigin, VpcConfiguration, Alias) or its modeled refusal. The account id is fixed text only in its position; its value comes from the request's AccountId member.

Emulator: the emulator offers only the bare authority it is reached on. When the SDK applies the modeled host prefix, the address is refused before any HTTP request is made — 'CapabilityNotOffered: the target serves no address for <account-id>.<authority>' / simparity.egress.EgressRefused: "egress refused: '<account-id>.<authority>' is not the endpoint under test (<authority>)". No HTTP status, no error code and no body is produced. (In this run the host was 000000000000.127.0.0.1; the account id and the loopback authority are per-environment values — the requirement is that '<12-digit account id>.<served authority>' is served, not those literals.)

Why: any SDK/CLI client that deploys an access point with CloudFormation and then reads it back through s3control gets a name-resolution/connection failure instead of a response, so the whole deploy-then-verify flow (Internet-origin vs VPC-origin access points, replacement on VpcConfiguration change) is unusable and the service reads as entirely broken.

Fix: the emulator's HTTP front door — the hostnames/authorities it announces and its host-based request dispatch — must accept and route the modeled '{AccountId}.' host prefix for the s3control endpoint (stripping and validating it against the x-amz-account-id header and the AccountId @hostLabel pattern ^\d{12}$) so a host-prefixed request lands on the same s3control handlers as a bare-authority request. The open issues 20261116T222947Z-s3control-the-account-id-host-prefix-every-operation.md, 20261117T130148Z-cloudformation-s3control-the-s3control-accountid-host-prefix-is.md and 20261116T222947Z-s3-s3control-the-s3control-account-id-host-prefix.md describe the same front-door defect for their own service headings but do not carry these two check ids, so these checks are not covered by them.

Traces: the two failing check ids listed above and in trace_ids.

## Evidence
Behavioral suite: 0/59 passed; cloudformation__ec2__s3__s3control 0/3 (the third failure is already owned by 20261109T211344Z-cloudformation-ec2-s3-s3control-no-aws-s3express-resource.md and is not re-filed).

Both unowned checks report the identical pre-request failure:

  Error: CapabilityNotOffered: the target serves no address for 000000000000.127.0.0.1
    File "egress.py", line 417, in resolve -> refuse(...)
    File "egress.py", line 317, in refuse -> raise EgressRefused(
    simparity.egress.EgressRefused: egress refused: '000000000000.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on

Expected: s3control.get_access_point(AccountId=<account>, Name=<deployed name>) reaches the emulator at '<account-id>.<authority>' (per @endpoint(hostPrefix "{AccountId}.")) and answers the modeled 200 body — for the Internet-origin deploy (Bucket=<bucket>, NetworkOrigin='Internet', no VpcConfiguration) and, after the VpcConfiguration change set is executed, a VPC-origin access point under a new name.

Actual: nothing is served on that host, so the call fails at address resolution: no HTTP status, no error code, no body. The account id (000000000000) and authority (127.0.0.1) in the message are per-run values; the fixed requirement is that the authority prefixed with '<12-digit account id>.' is served.

## Steps to Reproduce
1. Clone the repo and boot the emulator.
2. With boto3, create a cloudformation client and deploy a template containing an AWS::S3::Bucket, an AWS::EC2::VPC and an AWS::S3::AccessPoint on that bucket; wait for CREATE_COMPLETE.
3. Create an s3control client against the emulator's endpoint and call get_access_point(AccountId=<12-digit account id>, Name=<the stack's access point name>).
4. Observe botocore applies the modeled @endpoint(hostPrefix "{AccountId}.") and addresses '<account id>.<emulator authority>'; the request never reaches the emulator — under the harness it is refused with 'CapabilityNotOffered: the target serves no address for <account id>.<authority>'.
5. After the fix the same call must be routed to the s3control handlers (host prefix accepted and validated against x-amz-account-id and ^\d{12}$) and answer the modeled GetAccessPoint body.
6. Re-replay both check ids in trace_ids.

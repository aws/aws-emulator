Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3control.cfn-s3-access-point-bucket-change-replaces-it-and-routes-writes-to-the-new-bucket cloudformation__s3__s3control.cfn-s3-access-point-tag-value-over-256-characters-rejected cloudformation__s3__s3control.cfn-s3-access-point-two-character-bucket-passes-schema-and-is-rejected-at-create cloudformation__s3__s3control.deploy-access-point-name-held-by-a-stack-rejected-for-a-direct-create cloudformation__s3__s3control.deploy-s3-access-point-carries-stack-system-tags-and-writes-through-to-its-bucket cloudformation__s3__s3control.deploy-access-point-alias-delete-bucket-rejected cloudformation__s3__s3control.deploy-access-point-alias-of-longest-name-serves-bucket-objects cloudformation__s3__s3control.deploy-access-point-bucket-account-id-change-replaces-while-tags-update-in-place cloudformation__s3__s3control.deploy-access-point-tagged-at-create-lists-its-tags-and-serves-the-bucket
Issue-Key: cloudformation__s3__s3control.cfn-s3-access-point-bucket-change-replaces-it-and-routes-writes-to-the-new-bucket|run#13|EgressRefused|assert s3c.get_access_point(AccountId=ctx.account, Name=old)["Bucket"] == first, (
Issue-Key: cloudformation__s3__s3control.cfn-s3-access-point-tag-value-over-256-characters-rejected|_label#12|EgressRefused|arn = s3c.get_access_point(AccountId=account, Name=point)["AccessPointArn"]
Issue-Key: cloudformation__s3__s3control.cfn-s3-access-point-two-character-bucket-passes-schema-and-is-rejected-at-create|run#11|EgressRefused|ap = s3control.get_access_point(AccountId=ctx.account, Name=good_ap)
Issue-Key: cloudformation__s3__s3control.deploy-access-point-name-held-by-a-stack-rejected-for-a-direct-create|run#15|EgressRefused|fronted = s3c.get_access_point(AccountId=acct, Name=point)["Bucket"]
Issue-Key: cloudformation__s3__s3control.deploy-s3-access-point-carries-stack-system-tags-and-writes-through-to-its-bucket|run#12|EgressRefused|seen = s3c.get_access_point(AccountId=ctx.account, Name=point)
Issue-Key: cloudformation__s3__s3control.deploy-access-point-alias-delete-bucket-rejected|run#8|KeyError|out = {o["OutputKey"]: o["OutputValue"]
Issue-Key: cloudformation__s3__s3control.deploy-access-point-alias-of-longest-name-serves-bucket-objects|run#11|KeyError|alias = {o["OutputKey"]: o["OutputValue"] for o in outputs}.get("DoorAlias")
Issue-Key: cloudformation__s3__s3control.deploy-access-point-bucket-account-id-change-replaces-while-tags-update-in-place|_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__s3__s3control.deploy-access-point-tagged-at-create-lists-its-tags-and-serves-the-bucket|run#16|AssertionError|assert arn == want_arn, f"GetAtt Arn gave {arn!r}, expected the access point ARN for {point!r}"
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# cloudformation__s3__s3control: s3control {AccountId}. host prefix unserved, AWS::S3::AccessPoint Fn::GetAtt Alias yields an Output with no OutputValue, and GetAtt Arn uses 'access-point/' instead of 'accesspoint/' (3 causes)

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
All nine checks under this heading deploy an AWS::S3::AccessPoint with CloudFormation and then read it back, through s3control or through its CloudFormation outputs. They fail for three distinct reasons. This bundle has 3 causes; all three must be closed.

Cause 1 of 3: the s3control account-id host prefix is unserved, so no s3control verification call is delivered.
Failing check ids: cloudformation__s3__s3control.cfn-s3-access-point-bucket-change-replaces-it-and-routes-writes-to-the-new-bucket, cloudformation__s3__s3control.cfn-s3-access-point-tag-value-over-256-characters-rejected, cloudformation__s3__s3control.cfn-s3-access-point-two-character-bucket-passes-schema-and-is-rejected-at-create, cloudformation__s3__s3control.deploy-access-point-name-held-by-a-stack-rejected-for-a-direct-create, cloudformation__s3__s3control.deploy-s3-access-point-carries-stack-system-tags-and-writes-through-to-its-bucket.
AWS: Smithy (com.amazonaws.s3control#AWSS3ControlServiceV20180820, restXml, sigv4Name 's3'): every account-scoped operation carries @endpoint(hostPrefix: "{AccountId}.") and binds AccountId as both @httpHeader("x-amz-account-id") and @hostLabel (target AccountId, pattern ^\d{12}$) — e.g. CreateAccessPoint (PUT /v20180820/accesspoint/{Name}) and GetAccessPoint. An SDK therefore addresses '<12-digit account id>.<endpoint authority>' (against real AWS <account-id>.s3-control.<region>.amazonaws.com), and AWS answers there with the operation's modeled body or modeled refusal. The account id is fixed only in its position; its value comes from the request's AccountId member.
Emulator: only the bare authority is announced, so the host-prefixed address is refused before any HTTP request — 'CapabilityNotOffered: the target serves no address for <account-id>.<authority>' / simparity.egress.EgressRefused: "egress refused: '<account-id>.<authority>' is not the endpoint under test (<authority>)". No status, no error code, no body. (Observed host in this run: 000000000000.127.0.0.1 — both parts are per-environment values.)
Why: a client that deploys an access point and verifies it through s3control gets a connection failure instead of a response; the whole control-plane surface of s3control is unreachable from any SDK/CLI.
Fix: the emulator's HTTP front door (announced authorities + host-based dispatch) must accept, strip and validate the modeled '{AccountId}.' prefix for the s3control endpoint against x-amz-account-id and the ^\d{12}$ @hostLabel pattern, routing to the same handlers as a bare-authority request.

Cause 2 of 3: a stack Output whose Value is Fn::GetAtt [<AWS::S3::AccessPoint logical id>, "Alias"] comes back from DescribeStacks as an Output member with an OutputKey but NO OutputValue.
Failing check ids: cloudformation__s3__s3control.deploy-access-point-alias-delete-bucket-rejected, cloudformation__s3__s3control.deploy-access-point-alias-of-longest-name-serves-bucket-objects, cloudformation__s3__s3control.deploy-access-point-bucket-account-id-change-replaces-while-tags-update-in-place.
AWS: AWS::S3::AccessPoint exposes the readonly attribute Alias (Smithy: s3control CreateAccessPoint output member Alias, target Alias, @length max 63, @pattern ^[0-9a-z\\-]{63}$), so Fn::GetAtt on it resolves to the access point's alias string. CloudFormation's DescribeStacks returns, for every declared Output of a CREATE_COMPLETE stack, an Output structure carrying both OutputKey and OutputValue; a successfully created stack never reports an Output with a key and no value.
Emulator: DescribeStacks returns the Output entry for the GetAtt-Alias output with OutputKey present and OutputValue absent, so every client reading outputs as a key->value map raises KeyError: 'OutputValue' ({o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}). The stack itself reports CREATE_COMPLETE, so the resource is claimed to exist while its attribute is unresolved.
Why: any client that templates an access point and consumes its alias downstream (object calls through the alias, comparison against s3control GetAccessPoint's Alias, passing it to another stack) crashes or sees a missing value, and CloudFormation's contract that a complete stack's outputs are fully resolved is broken.
Fix: the CloudFormation resource provider for AWS::S3::AccessPoint must resolve its GetAtt attributes (at minimum Alias, alongside Arn and the Ref/physical id) when the resource is created, and the DescribeStacks output serializer must never emit an Output without OutputValue — an unresolvable attribute is a stack failure, not a half-populated Output.

Cause 3 of 3: Fn::GetAtt [<access point>, "Arn"] renders the ARN resource segment as 'access-point/<name>' instead of AWS's 'accesspoint/<name>'.
Failing check id: cloudformation__s3__s3control.deploy-access-point-tagged-at-create-lists-its-tags-and-serves-the-bucket.
AWS: an S3 access point ARN is 'arn:aws:s3:<region>:<account-id>:accesspoint/<access-point-name>' — the resource type segment is the single token 'accesspoint', unhyphenated (recorded behavior and AWS's documented access point ARN form; the s3control model constrains only the ARN's length/prefix: S3ResourceArn @pattern ^arn:[^:]+:s3(express)?:[^:]). Region, account id and name are values from the request and the caller's identity; 'arn:aws:s3:' and ':accesspoint/' are the fixed text.
Emulator: GetAtt Arn returns 'arn:aws:s3:<region>:<account-id>:access-point/<name>' (observed: arn:aws:s3:us-west-2:000000000000:access-point/<name>, where region, account and name are per-run values and only the 'access-point/' token is the defect).
Why: that ARN is used verbatim as an S3 bucket identifier on data-plane calls and as ResourceArn for s3control TagResource/ListTagsForResource; a hyphenated resource segment is not a valid access point ARN, so every downstream call built from a stack output is rejected or routed wrongly, and ARN comparisons against s3control responses never match.
Fix: the ARN construction used by the AWS::S3::AccessPoint resource provider's Arn attribute (and any shared access-point ARN formatter it uses with the s3control state) must emit the 'accesspoint/<name>' resource segment.

Traces: the nine failing check ids listed above and in trace_ids.

## Evidence
Behavioral suite: 0/59 passed; cloudformation__s3__s3control 0/9, all nine unowned.

Cause 1 — five checks, identical pre-request failure:
  Error: CapabilityNotOffered: the target serves no address for 000000000000.127.0.0.1
    File "egress.py", line 417, in resolve -> refuse(...)
    simparity.egress.EgressRefused: egress refused: '000000000000.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator.
  Expected: the s3control call (get_access_point / create_access_point / list_tags_for_resource with AccountId=<account>) reaches the emulator at '<account id>.<authority>' and answers its modeled response. Actual: no HTTP status, no error code, no body — the address is refused.

Cause 2 — three checks, identical crash while reading a CREATE_COMPLETE stack's outputs:
  deploy-access-point-alias-delete-bucket-rejected:
    out = {o["OutputKey"]: o["OutputValue"] for o in cfn.describe_stacks(...)["Stacks"][0].get("Outputs", [])}
    KeyError: 'OutputValue'
  deploy-access-point-alias-of-longest-name-serves-bucket-objects:
    alias = {o["OutputKey"]: o["OutputValue"] for o in outputs}.get("DoorAlias")
    KeyError: 'OutputValue'
  deploy-access-point-bucket-account-id-change-replaces-while-tags-update-in-place:
    return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
    KeyError: 'OutputValue'
  The templates declare, besides {"Ref": <access point>}, an output {"Value": {"Fn::GetAtt": [<access point logical id>, "Alias"]}}.
  Expected: DescribeStacks returns each declared Output with both OutputKey and OutputValue, the Alias output holding the access point's alias (the same string s3control GetAccessPoint reports in Alias). Actual: the Output entry has no OutputValue at all.

Cause 3 — one check:
  AssertionError: GetAtt Arn gave 'arn:aws:s3:us-west-2:000000000000:access-point/sim2real-xsvc-aptag-a11bee124ab0', expected the access point ARN for 'sim2real-xsvc-aptag-a11bee124ab0'
  Expected form: arn:aws:s3:<region>:<account-id>:accesspoint/<name>. Actual form: arn:aws:s3:<region>:<account-id>:access-point/<name>. Only the resource-type token differs; region, account id and name are per-run values.

## Steps to Reproduce
Cause 1:
1. Boot the emulator; deploy a stack with an AWS::S3::Bucket and an AWS::S3::AccessPoint on it.
2. Call s3control.get_access_point(AccountId=<12-digit account id>, Name=<the deployed name>); observe botocore addresses '<account id>.<authority>' per @endpoint(hostPrefix "{AccountId}.") and the request is never delivered.
3. After the fix the host-prefixed request must route to the s3control handlers and answer the modeled body.

Cause 2:
1. Deploy a stack whose Resources are an AWS::S3::Bucket and an AWS::S3::AccessPoint on it, with Outputs {"HatchName": {"Value": {"Ref": "Hatch"}}, "HatchAlias": {"Value": {"Fn::GetAtt": ["Hatch", "Alias"]}}}; wait for CREATE_COMPLETE.
2. Call describe_stacks(StackName=<stack id>) and inspect Stacks[0].Outputs.
3. Observe the HatchAlias entry carries OutputKey but no OutputValue (building a key->value dict raises KeyError: 'OutputValue').
4. After the fix every Output of the complete stack must carry an OutputValue, the alias one equal to the access point's alias as s3control GetAccessPoint reports it.

Cause 3:
1. Deploy a stack with an AWS::S3::AccessPoint and an Output {"Value": {"Fn::GetAtt": [<ap>, "Arn"]}}.
2. Read that output after CREATE_COMPLETE.
3. Observe the resource segment is 'access-point/<name>'; it must be 'accesspoint/<name>' (full form arn:aws:s3:<region>:<account-id>:accesspoint/<name>).

Finally re-replay every check id listed in trace_ids.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-vpc-origin-denies-internet-read-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-vpc-origin-denies-internet-read-rejected|run#22|AssertionError|assert code == "AccessDenied", (
Issue-Commit: 1d22722e0ee3e790fd3b09872735fa1c06442f07

# s3control: an internet-borne read of a VPC-origin access point answers NoSuchBucket instead of AccessDenied (VPC network origin is never enforced)

> _Note: this issue was identified against the code base at commit `1d22722e0ee3e790fd3b09872735fa1c06442f07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint / GetAccessPoint (VpcConfiguration) plus S3 GetObject addressed by the access point ARN

## Description
One net-new root cause on the s3control access-point surface, grounded in the one failing behavioral check of this suite that is neither marked ALREADY FILED nor OWNED by an open issue. Related open issues exist but do NOT cover it: 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md (its cause 3 covers access-point ARN addressing as a routing gap for the plain Internet-origin data path, and its recorded replay key for this very check was the now-fixed KeyError on GetAccessPoint's Endpoints map, which is why this check now progresses past GetAccessPoint and fails later and differently) and 20261027T001031Z-s3-7-net-new-root-causes-access-point.md (the S3-side ARN addressing cluster). Neither states the VPC-origin network restriction, which is the requirement below. 20261029T192817Z-s3control-access-point-reads-listings-and-refusals-are.md is already FIXED and its fix is visible here: NetworkOrigin=='VPC', VpcConfiguration=={'VpcId': ...} and Endpoints['ipv4'] all read back correctly now, so this is not a reopen of it — a different call is diverging.

Cause 1 of 1: a VPC-origin access point does not restrict the network it answers on, and the access-point data path refuses with the wrong error entirely.

AWS: Smithy CreateAccessPoint/GetAccessPoint carry VpcConfiguration (member VpcId, required, @length 1..1024) whose documentation states the rule verbatim: "If this field is specified, this access point will only allow connections from the specified VPC ID." NetworkOrigin is an enum of Internet|VPC. Smithy binds no errors to these operations, so the refusal itself is recorded behavior only: a GetObject addressed to arn:aws:s3:<region>:<account>:accesspoint/<name> for an access point whose NetworkOrigin is VPC, arriving from outside that VPC, is refused with HTTP 403, error code AccessDenied and message "User: <caller-arn> is not authorized to perform: s3:GetObject on resource: \"<access-point-arn>/object/<key>\" with an explicit deny in a resource-based policy". The fixed text of that message is "User: ", " is not authorized to perform: s3:GetObject on resource: ", the surrounding double quotes around the resource, and " with an explicit deny in a resource-based policy"; the variable parts are the caller's identity ARN (from the signing identity of the request), the access point ARN and the object key (both from the request). The deny is implicit in the VPC origin and is NOT liftable: after PutAccessPointPolicy stores an explicit Allow for Principal {"AWS": "*"} on s3:GetObject over <ap-arn>/object/*, the same internet-borne read is still refused with the identical AccessDenied code and message. The positive control in the same scenario is that an Internet-origin access point over the same bucket serves the object's exact bytes with 200.

Emulator: the internet-borne GetObject through the VPC-origin access point ARN is refused with error code NoSuchBucket (the check reports: "an internet-borne read of a VPC-origin access point returned 'NoSuchBucket', expected AccessDenied"), i.e. the ARN form is treated as a literal bucket name that does not exist. Because the request never resolves to an access point, the VPC network-origin restriction recorded on it is never consulted at all, the AccessDenied message above is never produced, the "an explicit Allow for AWS:* does not lift the VPC deny" step is unobservable, and the Internet-origin positive control read behind it cannot run either.

Why: a client that confines a bucket's data path to one VPC by creating a VPC-origin access point gets no evidence the confinement exists — the emulator accepts and reports the VpcConfiguration faithfully but enforces nothing, so a negative test for "an off-VPC caller is denied" cannot distinguish enforcement from a dead code path, and a client branching on AccessDenied/403 (retryable=false, authorization failure) instead receives NoSuchBucket, which ordinary S3 clients treat as "the bucket is gone" and often as a reason to create or re-provision it.

Fix: access-point ARN/alias resolution in the S3 data-plane bucket resolver so an accesspoint ARN resolves against the s3control access-point registry rather than being hashed as a bucket name, plus a network-origin gate in that resolved path: when the resolved access point's NetworkOrigin is VPC and the request did not arrive via the bound VpcId, refuse with AccessDenied/403 and the message template above, evaluated BEFORE (and not overridable by) the access point's resource policy. The AccessDenied message construction belongs in the shared S3 authorization-refusal serializer so the resource segment is rendered as <ap-arn>/object/<key>.

Traces: s3control.access-point-vpc-origin-denies-internet-read-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 — s3control 15/34 passed (44%).

Failing check s3control.access-point-vpc-origin-denies-internet-read-rejected [mismatch], at check_s3control_access_point_vpc_origin_denies_internet_read_rejected.py:62:

    AssertionError: an internet-borne read of a VPC-origin access point returned 'NoSuchBucket', expected AccessDenied

The assertion is `assert code == "AccessDenied"` on the error raised by `s3.get_object(Bucket=vpc_arn, Key="private.txt")`, where `vpc_arn` is the AccessPointArn returned by `s3control.create_access_point(AccountId=..., Name=..., Bucket=..., VpcConfiguration={"VpcId": <an available VPC>})`.

The three assertions immediately preceding it PASSED, which pins the divergence to the data path and not to the control-plane projection:
  - GetAccessPoint reported NetworkOrigin == 'VPC';
  - GetAccessPoint echoed VpcConfiguration == {'VpcId': <vpc-id>};
  - GetAccessPoint reported an Endpoints['ipv4'] matching [A-Za-z0-9.-]+(:\d+)? — the check notes AWS advertises the same regional hostnames for both origins, so enforcement cannot come from the endpoint set.

Expected on that call: error code AccessDenied with message
    User: <caller identity ARN> is not authorized to perform: s3:GetObject on resource: "<AccessPointArn>/object/private.txt" with an explicit deny in a resource-based policy
Got: error code NoSuchBucket.

Two further requirements of the same check were never reached because the first one aborted it: (a) after PutAccessPointPolicy with an explicit Allow for Principal {"AWS": "*"} on s3:GetObject over <ap-arn>/object/*, the internet-borne read must STILL be AccessDenied with the same message; (b) an Internet-origin access point created over the same bucket must serve the object bytes (b"vpc-only").

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly the check in trace_ids (s3control.access-point-vpc-origin-denies-internet-read-rejected), or by hand:

1. ec2 DescribeVpcs(Filters=[{Name: state, Values: [available]}]) and take any available VpcId.
2. s3 CreateBucket (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1); s3 PutObject(Key="private.txt", Body=b"vpc-only").
3. s3control CreateAccessPoint(AccountId, Name=<vpc-ap>, Bucket=<bucket>, VpcConfiguration={"VpcId": <that VpcId>}); keep the returned AccessPointArn.
4. s3control GetAccessPoint(AccountId, Name=<vpc-ap>) — confirm NetworkOrigin=='VPC' and VpcConfiguration=={'VpcId': ...} (these already pass today).
5. s3 GetObject(Bucket=<AccessPointArn>, Key="private.txt") from outside that VPC. Expect HTTP 403 / AccessDenied with message `User: <caller arn> is not authorized to perform: s3:GetObject on resource: "<AccessPointArn>/object/private.txt" with an explicit deny in a resource-based policy`. Observed: NoSuchBucket.
6. s3control PutAccessPointPolicy(AccountId, Name=<vpc-ap>, Policy={Allow, Principal {"AWS":"*"}, Action s3:GetObject, Resource "<AccessPointArn>/object/*"}) and repeat step 5 — the refusal must be unchanged (the VPC-origin deny is not liftable by a resource policy Allow).
7. Positive control: s3control CreateAccessPoint(AccountId, Name=<net-ap>, Bucket=<same bucket>) with no VpcConfiguration, then s3 GetObject(Bucket=<that AccessPointArn>, Key="private.txt") — expect 200 and the exact bytes b"vpc-only".

## Resolution

The VPC network origin is now ENFORCED on the data path: the VPC an access point is bound to
is published with its data path (`service-com-amazonaws-s3control`'s `state::confining_vpc` ->
`state::access_point_routes::publish`), and a request that reaches that path from outside the
VPC is refused 403 `AccessDenied` with IAM's resource-policy-deny sentence over the resource
the access point names, ahead of and not liftable by any policy — for the credentialed caller
(`service-com-amazonaws-s3`'s `service::request_authorization::denial`) and the anonymous one
(`state::Bucket::anonymous_object_read_refusal`) alike. Proven end to end over every address an
access point is reached by, both key forms, the read/HEAD/listing actions, four access point
policy documents and the unsigned caller, by
`core/router/tests/all/s3_access_point_vpc_network_origin.rs`.

The recorded check `s3control.access-point-vpc-origin-denies-internet-read-rejected` STILL
answers `NoSuchBucket` for its own read, and not because the rule above is missing: against the
emulator's IP-literal endpoint the SDK degrades the access-point ARN away entirely. The read
arrives as `GET /private.txt` with the bare endpoint authority as its `Host` — no bucket in the
path and no access point in the host — so it identifies neither, and the emulator answers it as
AWS answers a path-style read of a bucket that does not exist. That endpoint form is the open
issue `20261029T083133Z-s3-virtual-hosted-style-bucket-addressing-is-unreachable.md`, whose fix
is the listener/endpoint announcement in the read-only `vendor/` tree; this check cannot pass
until an access-point address can reach the emulator at all.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2c7d38992bc517da436f6aedf2c705099043ad63

# arn_partition names the commercial partition for a eusc-de-* Region, so ARNs are minted in a partition their own pattern rejects and disagree with the control plane that owns the Outpost

_Filed: 2026-10-29T09:41:14Z_
_Operation: ListOutpostsWithS3 | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2c7d38992bc517da436f6aedf2c705099043ad63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::arn_partition` is this crate's region-to-partition function, and its prefix table omits `eusc-de-`. Any Region with that prefix therefore falls through the table to the literal `"aws"` — which `ARN_PARTITIONS` DOES admit — so `admitted_arn_partition` answers `Some("aws")` and `OutpostRecord::outpost_arn`, `OutpostRecord::s3_outpost_arn` and `EndpointRecord::endpoint_arn` all mint an ARN whose partition field (`aws`) and Region field (`eusc-de-…`) describe two different places.

That is exactly what the function's own doc says it exists to prevent, and it is the same defect the crate already fixed for the `us-isof-` / `eu-isoe-` prefixes: for a Region whose partition the members' closed alternation `(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b)` does not list, the crate's stated rule is that the optional ARN member carries NOTHING. `aws-eusc` is not in that alternation, so `Outpost$OutpostArn`, `Outpost$S3OutpostArn`, `Endpoint$EndpointArn` and `CreateEndpointResult$EndpointArn` should all be absent for such a Region — instead they carry a commercial-partition string.

The divergence is not hypothetical and is not a claim about AWS: the Outposts whose Regions this service reports come out of the co-hosted AWS Outposts control plane, which this crate imports directly (`src/state.rs:96`) and reads per call (`outpost_inventory` / `s3_provisioned_outpost`, which takes the Region straight off `outposts_state::OutpostRecord::region`). That dependency's own `partition_for` maps `eusc-de-` to `aws-eusc` and has a test pinning it. So for one Outpost record held in `eusc-de-east-1`, the emulator reports TWO different `OutpostArn`s for the same resource: `arn:aws-eusc:outposts:eusc-de-east-1:…` from `outposts`, and `arn:aws:outposts:eusc-de-east-1:…` from `ListOutpostsWithS3`.

`is_region` admits `eusc-de-east-1` (lowercase/digits/hyphen only), so such a record is storable both through `S3_OUTPOSTS_PROVISIONED_OUTPOSTS` and through the control plane, and nothing in this crate refuses it.

This is a DIFFERENT gap from the fixed ListOutpostsWithS3 issue about Outpost-id branches (`OUTPOST_ARN_BRANCHES` / `S3_OUTPOST_ARN_BRANCHES`), which gates the id segment; the segment at fault here is the partition. It also does not reverse the fixed `aws-iso-f` / `aws-iso-e` fix — it extends the very admission gate that fix introduced to the one prefix the table still misses.

## Evidence

The rule this crate states for its own partition function — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:858-871:

    /// An ARN names its resource in full, so its partition field and its Region
    /// field describe the same place: a Region of the China partition can only be
    /// named by an `aws-cn` ARN, a GovCloud Region only by an `aws-us-gov` one, and
    /// so on.
    ...
    /// The prefixes and partition names are AWS's own. The isolated Regions AWS
    /// added after this service's patterns were published get their own partition
    /// too — the Region decides, and naming the commercial partition for a Region
    /// outside it is the very disagreement this function exists to prevent.

The code under that doc, with no `eusc-de-` entry, falling through to the commercial partition — src/state.rs:877-890:

    pub fn arn_partition(region: &str) -> &'static str {
        const PARTITION_PREFIXES: &[(&str, &str)] = &[
            ("cn-", "aws-cn"),
            ("us-gov-", "aws-us-gov"),
            ("us-isob-", "aws-iso-b"),
            ("us-isof-", "aws-iso-f"),
            ("eu-isoe-", "aws-iso-e"),
            ("us-iso-", "aws-iso"),
        ];
        PARTITION_PREFIXES
            .iter()
            .find(|(prefix, _)| region.starts_with(prefix))
            .map_or("aws", |(_, partition)| *partition)
    }

so the admission gate the previous fix added passes it through — src/state.rs:901 and :913-916:

    pub const ARN_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov", "aws-iso", "aws-iso-b"];
    ...
    pub fn admitted_arn_partition(region: &str) -> Option<&'static str> {
        let partition = arn_partition(region);
        arn_admits_partition(partition).then_some(partition)
    }

and the three ARN builders interpolate it — src/state.rs:293-300, :309-316, :371-378:

    pub fn outpost_arn(&self) -> Option<String> {
        let partition = admitted_arn_partition(&self.region)?;
        ...
            format!("arn:{partition}:outposts:{}:{}:outpost/{}", self.region, self.owner_id, self.outpost_id)

The closed alternation these members publish, which does not list `aws-eusc` (smithy_query query="shape" shape="Outpost,Endpoint"):

    Outpost$OutpostArn   pattern: ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)$
    Outpost$S3OutpostArn pattern: ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|\d{12})/s3$
    Endpoint$EndpointArn pattern: ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):s3-outposts:[a-z\-0-9]*:[0-9]{12}:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$

The other half of the contradiction, in the crate this one imports and reads its Outposts out of — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:96:

    use ::service_com_amazonaws_outposts_outpostsolafservice::state as outposts_state;

and that crate's own partition function — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:4726-4741:

    /// AWS's published region-to-partition prefixes: `cn-*` is `aws-cn`, `us-gov-*` is
    /// `aws-us-gov`, `eusc-de-*` is `aws-eusc`, the ISO regions carry their own
    /// partitions (`us-iso-*` -> `aws-iso`, `us-isob-*` -> `aws-iso-b`, `eu-isoe-*` ->
    /// `aws-iso-e`, `us-isof-*` -> `aws-iso-f`), and every other (commercial) region is
    /// `aws`.
    pub fn partition_for(region: &str) -> &'static str {
        const PARTITIONS: &[(&str, &str)] = &[
            ("cn-", "aws-cn"),
            ("us-gov-", "aws-us-gov"),
            ("eusc-de-", "aws-eusc"),
            ...

pinned there by services/service-com-amazonaws-outposts-outpostsolafservice/tests/all/arn_partition.rs:26,40:

    /// for the European Sovereign Cloud (`eusc-de-*`), and the four ISO partitions
    ...
        ("eusc-de-east-1", "aws-eusc"),

The Region is storable here: src/state.rs:848-853 (`is_region`) admits `eusc-de-east-1`, and src/state.rs:766-784 (`s3_provisioned_outpost`) passes the control plane's own `record.region` into `OutpostRecord::provisioned(..)` unchanged.

Test coverage of this seam: NONE, so the fix leaves every existing test passing. services/service-com-amazonaws-s3outposts-s3outposts/tests/all/arn_partitions.rs:52-113 enumerates one row per partition and has no `eusc-de-*` row at all; its two "outside the alternation" rows are only:

    Row { case: "an isolated partition the ARN patterns' alternation does not list", region: "us-isof-east-1", partition: None },
    Row { case: "the European isolated partition, also outside the alternation", region: "eu-isoe-west-1", partition: None },

and tests/all/arn_partition_admission.rs:71,76 covers the same two Regions. A grep for `eusc` across the whole s3outposts crate returns nothing.

## Steps to Reproduce

1. Stand up one emulator hosting BOTH services over one shared engine, the arrangement tests/all/control_plane_inventory.rs:104-118 builds (s3-outposts plus outposts on one engine).
2. Put one S3-capable Outpost in a European Sovereign Cloud Region — either through the AWS Outposts API signed for eusc-de-east-1 (create site, create Outpost, order S3 capacity, as control_plane_inventory.rs does), or simply run with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248:<emulator-account>:eusc-de-east-1".
3. GET /S3Outposts/ListOutpostsWithS3?maxResults=100 signed for eusc-de-east-1.
4. Observed: OutpostArn="arn:aws:outposts:eusc-de-east-1:<account>:outpost/op-0a1b2c3d4e5f60718" and S3OutpostArn="arn:aws:s3-outposts:eusc-de-east-1:<account>:outpost/op-0a1b2c3d4e5f60718/s3" — the partition field says the commercial partition while the Region field says a eusc-de Region.
5. Read the same Outpost through the co-hosted control plane that owns it (GetOutpost signed for eusc-de-east-1): its OutpostArn is "arn:aws-eusc:outposts:eusc-de-east-1:<account>:outpost/op-0a1b2c3d4e5f60718" (outposts/src/state.rs:4736, pinned by outposts/tests/all/arn_partition.rs:40) — two different ARNs for one resource.
6. Expected per this crate's own rule (src/state.rs:858-871 and the ARN_PARTITIONS admission gate): aws-eusc is not in the members' alternation, so both ARN members carry nothing at all, exactly as they already do for us-isof-east-1 and eu-isoe-west-1 (tests/all/arn_partitions.rs:103-112, partition: None).
7. Repeat on the endpoint side: POST /S3Outposts/CreateEndpoint signed for eusc-de-east-1 with {"OutpostId":"op-0a1b2c3d4e5f60718","SubnetId":"subnet-0a1b2c3d4e5f60718","SecurityGroupId":"sg-0a1b2c3d4e5f60718"} answers EndpointArn="arn:aws:s3-outposts:eusc-de-east-1:<account>:outpost/op-0a1b2c3d4e5f60718/endpoint/<id>", and ListEndpoints / ListSharedEndpoints repeat it.

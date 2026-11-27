Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values cloudfront-create-distribution-origin-config-bounds-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|UpdateDistribution|4xx->2xx|status
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig
Issue-Key: cloudfront|GetDistribution|2xx->2xx|$.Distribution.DistributionConfig.OriginGroups.Items.OriginGroup.SelectionCriteria
Issue-Key: cloudfront|ListDistributions|2xx->2xx|$.DistributionList.Items.DistributionSummary[*]
Issue-Key: cloudfront|GetDistributionConfig|2xx->2xx|$.DistributionConfig.DefaultCacheBehavior.CachePolicyId
Issue-Commit: 9035f96a24b7bc6854e584ea99175d352b190dfa

# CloudFront distribution writes are unvalidated and the stored config is not read back faithfully

> _Note: this issue was identified against the code base at commit `9035f96a24b7bc6854e584ea99175d352b190dfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
Five net-new wire divergences on the CloudFront distribution write path. The shared fault is that a submitted DistributionConfig is neither fully validated nor faithfully persisted: a config AWS refuses is accepted (and then shows up in state and in listings), and members the caller did submit are missing from the create echo and from both readback paths. Related open issues 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md (CreateDistribution validation) and 20261105T231732Z-cloudfront-readback-omits-defaulted-members-and-reports-stale.md (other dropped/stale readback members) carry their own replay keys and do NOT cover the steps below.

Cause 1 of 5: UpdateDistribution accepts a DefaultCacheBehavior that specifies neither a cache policy nor a complete set of deprecated forwarded values, answering 200 instead of refusing it.
AWS: Smithy: DefaultCacheBehavior documents "A DefaultCacheBehavior must include either a CachePolicyId or ForwardedValues"; when the deprecated ForwardedValues path is used the deprecated TTL members (MinTTL in particular) are required alongside it. UpdateDistribution (PUT /2020-05-31/distribution/{Id}/config, code 200) models InvalidArgument with httpError 400. The recorded refusal is status 400 with an ErrorResponse body whose Error carries Type 'Sender', Code 'InvalidArgument' and Message 'The parameter MinTTL is required.' — the whole sentence is fixed text except the member name ('MinTTL'), which is the request member that is missing; RequestId is per-run and not asserted.
Emulator: answers 200 with a Distribution body and no ErrorResponse at all, applying the invalid config.
Why: a client that mis-assembles a cache behavior gets a success and believes the distribution is configured, when AWS would have rejected the call and left the distribution untouched; the emulator's distribution is now in a state AWS cannot represent, and the caller never sees the InvalidArgument its error handling is written around.
Fix: UpdateDistribution's validation — reject a DefaultCacheBehavior lacking CachePolicyId unless a complete ForwardedValues plus TTL set is present, with 400 InvalidArgument and AWS's "The parameter <member> is required." wording, before any state mutation.
Traces: cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values#task#5.

Cause 2 of 5: GetDistributionConfig drops DefaultCacheBehavior.CachePolicyId.
AWS: Smithy: DefaultCacheBehavior.CachePolicyId is "The unique identifier of the cache policy that is attached to the default cache behavior"; AWS stores the attached policy id and returns it in the DistributionConfig on every read (recorded: the managed cache-policy id that the distribution was created with).
Emulator: omits $.DistributionConfig.DefaultCacheBehavior.CachePolicyId from the 200 body.
Why: the cache policy attachment is invisible to a reader. A read-modify-write round trip (GetDistributionConfig, edit, UpdateDistribution) re-submits a default cache behavior with no CachePolicyId, which AWS then refuses (Cause 1) — so the emulator leads clients into exactly the request it also fails to validate.
Fix: the DistributionConfig serializer / distribution state — persist and emit the attached CachePolicyId.
Traces: cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values#task#6.

Cause 3 of 5: ListDistributions enumerates a distribution that AWS never created.
AWS: ListDistributions (GET /2020-05-31/distribution, code 200) returns DistributionList.Items with one DistributionSummary per existing distribution. In the recording the preceding CreateDistribution in this fixture is refused with 400 InvalidArgument, so no distribution exists and the listing has no summary at that position.
Emulator: the 200 body carries an extra $.DistributionList.Items.DistributionSummary[0] element that the recording does not have.
Why: the listing is a client's inventory of its distributions; it reports a resource that the service said it would not create, so reconciliation loops, teardown sweeps and "does it already exist?" checks all act on a phantom distribution.
Hypothesis: this is the state consequence of the accepted-but-invalid create (the create step itself is clustered under 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md); the listing diverges because the refused create was committed to state. The listing's own replay key is unowned, and it must be re-replayed after the validation fix.
Fix: CreateDistribution's apply-after-validate ordering (no state write on a refused create), which is what ListDistributions projects.
Traces: cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values#task#2.

Cause 4 of 5: CreateDistribution's 201 echo of DistributionConfig does not match the config that was submitted.
AWS: Smithy: CreateDistribution returns the created Distribution with Distribution.DistributionConfig (@required); AWS echoes the submitted configuration, with server-defaulted members materialised — including every origin's CustomOriginConfig timeouts and ConnectionAttempts/ConnectionTimeout, the OriginGroups block and OriginGroup.SelectionCriteria (recorded echo includes OriginGroups.Items.OriginGroup.SelectionCriteria 'default').
Emulator: the echoed $.Distribution.DistributionConfig differs from the recorded one.
Why: the create response is the only view of the resource a client has before its first read; if the echo is not the submitted-plus-defaulted configuration, a client that caches it (or diffs it to confirm what the service accepted) operates on a configuration the service does not hold.
Hypothesis: the diverging member is the same one the immediately following read drops (Cause 5, OriginGroup.SelectionCriteria on the same distribution), i.e. the echo is produced from a state record that never materialised the origin-group selection criteria.
Fix: the CreateDistribution response serializer / distribution state default materialisation, shared with GetDistribution.
Traces: cloudfront-create-distribution-origin-config-bounds-rejected#task#6.

Cause 5 of 5: GetDistribution omits OriginGroups.Items.OriginGroup.SelectionCriteria.
AWS: Smithy: OriginGroup.SelectionCriteria targets the enum OriginGroupSelectionCriteria with values [default, media-quality-based]; AWS materialises it for every origin group and serializes it on readback (recorded: 'default'), status 200.
Emulator: the member is absent from $.Distribution.DistributionConfig.OriginGroups.Items.OriginGroup.
Why: a client cannot tell whether an origin group is doing plain failover selection or media-quality-based selection, and an SDK read-modify-write of the origin group re-submits the group without SelectionCriteria.
Fix: the DistributionConfig serializer / origin-group state — materialise and emit SelectionCriteria ('default' when the caller did not choose one).
Traces: cloudfront-create-distribution-origin-config-bounds-rejected#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 94.6%, 32 divergence clusters, all cloudfront; 1442 steps, 76 mismatched).

Cause 1 — cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values#task#5 UpdateDistribution: status expected 400, got 200; $.ErrorResponse missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidArgument', 'Message': 'The parameter MinTTL is required.'}, 'RequestId': <not asserted>}, got None).
Cause 2 — same fixture #task#6 GetDistributionConfig (200/200): $.DistributionConfig.DefaultCacheBehavior.CachePolicyId missing (expected the managed cache-policy id '658327ea-f89d-4fab-a63d-7e88639e58f6' the distribution was created with, got None).
Cause 3 — same fixture #task#2 ListDistributions (200/200): $.DistributionList.Items.DistributionSummary[0] unexpected_present — the emulator emits a summary element the recording does not have.
Cause 4 — cloudfront-create-distribution-origin-config-bounds-rejected#task#6 CreateDistribution (201/201): $.Distribution.DistributionConfig value_mismatch against the recorded echo (recorded echo includes OriginGroups.Items.OriginGroup.SelectionCriteria 'default' and the full CustomOriginConfig/ConnectionAttempts/ConnectionTimeout materialisation).
Cause 5 — same fixture #task#7 GetDistribution (200/200): $.Distribution.DistributionConfig.OriginGroups.Items.OriginGroup.SelectionCriteria missing (expected 'default', got None).
The CreateDistribution 400->201 clusters in these fixtures (#task#1..#task#5) are already owned by 20261022T025800Z-cloudfront-createdistribution-accepts-configurations-aws-rejects.md and are NOT part of this issue.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-default-cache-behavior-requires-cache-policy-or-forwarded-values: at task#5 PUT a DistributionConfig whose DefaultCacheBehavior has neither CachePolicyId nor a complete ForwardedValues+MinTTL set and confirm 400 InvalidArgument 'The parameter MinTTL is required.'; at task#6 confirm GetDistributionConfig returns the attached CachePolicyId; at task#2 confirm ListDistributions contains no summary for the distribution whose create was refused.
4. Replay fixture cloudfront-create-distribution-origin-config-bounds-rejected: at task#6 diff the 201 Distribution.DistributionConfig echo against the recorded one, and at task#7 confirm GetDistribution emits OriginGroups.Items.OriginGroup.SelectionCriteria.
5. All five causes must match the recording before closing.

Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-generic-package-publish-and-reads
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-generic-package-publish-and-reads codeartifact-generic-publish-validation-envelopes codeartifact-package-version-status-transitions
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|ListPackageVersions|2xx->2xx|$.versions[*].publishedTime
Issue-Commit: ed3ed06301643834f5ccb593614cfd988c817634

# CodeArtifact ListPackageVersions omits publishedTime from every PackageVersionSummary

> _Note: this issue was identified against the code base at commit `ed3ed06301643834f5ccb593614cfd988c817634`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPackageVersions

## Description
Cause 1 of 1: ListPackageVersions returns each package version WITHOUT its publish instant.

AWS: recorded behavior only for the member's presence. The on-disk Smithy model is STALE here: PackageVersionSummary models exactly `version` (required), `revision`, `status` (required) and `origin` and declares no `publishedTime` (smithy_query query="shape" shape="PackageVersionSummary" on services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/smithy-model/com-amazonaws-codeartifact-codeartifactcontrolplaneservice.json), while the member IS modelled on PackageVersionDescription as `publishedTime` (target Timestamp, "A timestamp that contains the date and time the package version was published"). The recorded AWS responses for ListPackageVersions (HTTP 200, restJson1) carry a `publishedTime` on EVERY entry of `versions`, serialized as this protocol's default epoch-seconds number with sub-second precision (the recording shows a value of the form 1789080921.643 — the digits are that run's clock and are not part of the requirement; what is required is that each listed version reports the instant at which that version was published in this emulator's own state, i.e. the same instant DescribePackageVersion reports for the same version). The operation's whole surface is otherwise as modelled: POST /v1/package/versions, 200, output members defaultDisplayVersion/format/namespace/package/versions/nextToken.

Emulator: the 200 body omits the member entirely on every summary. Replay reports `$.versions[0].publishedTime: missing (expected 1789080921.643, got None)` for three fixtures across generic publish/read, publish-validation and status-transition flows. The omission is deliberate in the code: PackageVersion::summary() (services/.../src/state.rs:1909) documents that "the members only a description carries (the coordinate format, namespace, packageName, and the publish instant publishedTime) are NOT repeated per summary" and builds PackageVersionSummary from version/revision/status/origin only, even though the record it projects already holds the instant (`PackageVersion::published_time`, state.rs:1753, which PackageVersionDescription does emit at state.rs:1881). So the gap is the summary projection (and the stale model entry behind the generated shape), not missing state.

Why: a client that lists a package's versions and then orders, ages out, displays or caches them by publish date gets no date at all. Every such caller must either re-read each version with DescribePackageVersion (N extra calls, and a different code path) or silently treat the field as absent — and an SDK/CLI that renders `publishedTime` for `list-package-versions` prints nothing. Sorting is especially misleading: the emulator accepts and honours sortBy=PUBLISHED_TIME (state.rs:1984) yet returns a page whose ordering key the client cannot see.

Fix: the ListPackageVersions response projection — PackageVersion::summary() in the codeartifact service's state, plus the PackageVersionSummary shape it fills (the on-disk model's PackageVersionSummary needs the `publishedTime` Timestamp member added the same way this crate already overrides a stale model entry for ExternalConnectionStatus, so the generated serializer can emit it as an epoch-seconds timestamp). Populate it from the stored `published_time` so a summary and a description agree for the same version. No open issue covers this: the open codeartifact issues concern failedVersions errorMessage text (20261108T214908Z-codeartifact-batch-package-version-operations-report-a-failedversions.md), publish digest-mismatch / GetRepositoryEndpoint envelopes (20261119T162524Z-codeartifact-wire-envelopes-diverge-digest-mismatch-publish-refused.md), AssociateExternalConnection 404 text and external-connection ingestion.

Traces: codeartifact-generic-package-publish-and-reads#task#7 (representative), codeartifact-generic-publish-validation-envelopes#task#7, codeartifact-package-version-status-transitions#task#5.

## Evidence
Replay cluster service=codeartifact|ListPackageVersions|status=200->200|keys=$.versions[0].publishedTime, verdict=mismatch, expected_status 200, actual_status 200, affected_count 3.

Expected (recorded AWS, 200): versions[0] includes publishedTime (epoch-seconds number, e.g. 1789080921.643) alongside version/revision/status/origin.
Actual (emulator, 200): versions[0] has no publishedTime member at all — detail: "$.versions[0].publishedTime: missing (expected 1789080921.643, got None)".

Source of the omission: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/state.rs:1900-1920 (PackageVersion::summary() omits the publish instant by design); the instant is stored at state.rs:1753 and emitted by the description projection at state.rs:1881. Model slice used: PackageVersionSummary members = version, revision, status, origin (no publishedTime); PackageVersionDescription.publishedTime = Timestamp.

## Steps to Reproduce
1. Replay fixture codeartifact-generic-package-publish-and-reads step task#7 against the emulator (also reproduced by codeartifact-generic-publish-validation-envelopes#task#7 and codeartifact-package-version-status-transitions#task#5).
2. Equivalent by hand: CreateDomain + CreateRepository, PublishPackageVersion for a generic package version, then POST /v1/package/versions?domain=<d>&repository=<r>&format=generic&namespace=<ns>&package=<p> (ListPackageVersions).
3. Inspect the 200 JSON body: each element of `versions` has version/revision/status/origin but no `publishedTime`. Compare with DescribePackageVersion for the same version, which does report publishedTime.

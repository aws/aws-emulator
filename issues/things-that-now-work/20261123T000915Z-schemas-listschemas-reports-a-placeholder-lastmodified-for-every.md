Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: d63c3d5740b378bc296b81bcfe5423c7f307026f

# schemas ListSchemas reports a placeholder LastModified for every AWS-managed catalogue schema

> _Note: this issue was identified against the code base at commit `d63c3d5740b378bc296b81bcfe5423c7f307026f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
Cause 1 of 1: every `SchemaSummary.LastModified` that `ListSchemas` serves for the service-owned `aws.events` catalogue is one hard-coded fallback instant instead of the publication instant the catalogue actually publishes for that entry. This single cause accounts for all 52 `ListSchemas` divergence clusters of fixture `schemas-catalog-list-schemas` (representative `schemas-catalog-list-schemas#task#1`; ~207 of the fixture's replayed steps), because every page of the managed registry contains at least one entry whose published instant is not the fallback.

AWS: Smithy `com.amazonaws.schemas#SchemaSummary$LastModified` -> `__timestampIso8601`, `@timestampFormat("date-time")`, documented as "The date and time that schema was modified." `ListSchemas` is `GET /v1/registries/name/{RegistryName}/schemas`, HTTP 200, `@paginated(items: Schemas)`. The model states nothing about the VALUE for AWS-managed entries, so for the value this is recorded behavior only: the recorded responses show that AWS reports, per catalogue entry, a STABLE historical publication instant that is NOT uniform across the catalogue and NOT derived from the entry's name — entries written by one publication pass share one whole-second instant, while entries of different passes report instants minutes, months or years apart (the recording shows most entries on a single pass-window of one day and outliers such as an ACM entry on an earlier date entirely). The same request replayed returns the same instants, i.e. it is catalogue data, not a clock reading of the request.

Emulator: the handler renders `last_modified` straight from the seeded `Schema.last_modified` (`services/service-com-amazonaws-schemas-schemas/src/ops/list_schemas.rs`, the `SchemaSummary { last_modified: Some(schema.last_modified.into()), .. }` map), and the managed-catalogue seed assigns a per-entry instant only to the handful of names listed in `MANAGED_PUBLICATIONS`; every other entry falls back to the single constant `CATALOGUE_PUBLICATION_INSTANT = "2019-12-01T00:00:00Z"` (`src/state.rs`, `managed_last_modified`, and the seed's `last_modified: entry.last_modified`). So the emulator answers HTTP 200 with the right `SchemaName`, `VersionCount` and `tags` but with one identical `LastModified` for essentially the whole catalogue, which no recorded page agrees with. Status and body shape are correct; only this member's value diverges.

Why: a client that reads `LastModified` to decide whether a published schema changed since it last synced (cache invalidation, code-binding regeneration, "newest managed schemas" ordering, drift detection in a registry mirror) sees one flat timestamp shared by thousands of entries. Against the emulator, ordering by `LastModified` is arbitrary, every entry looks equally stale, and a sync that skips entries older than its watermark skips the entire managed catalogue (or re-fetches all of it) — behavior that never reproduces what the same code does against AWS. The inverse also bites: nothing in the emulator ever distinguishes two publication passes, so a test asserting that two differently-published entries report different instants cannot pass.

Fix: the service's state — the managed-catalogue seed in `services/service-com-amazonaws-schemas-schemas/src/state.rs` that supplies `ManagedSchema::last_modified`. The per-entry publication instant has to be seeded as catalogue data for the whole published catalogue (the mechanism `MANAGED_PUBLICATIONS` already provides, grouping the names of one publication pass under one whole-second instant), rather than letting the bulk of the catalogue collapse onto a single `CATALOGUE_PUBLICATION_INSTANT` fallback. `ListSchemas`'s handler and serializer need no change: they already render whatever the store holds, and the version layout that walks backwards from the schema's instant (`MANAGED_VERSION_INTERVAL`) should keep deriving from the corrected per-entry instant so `DescribeSchema`/`ListSchemaVersions` stay consistent with the listing.

Traces: fixture `schemas-catalog-list-schemas`, steps `task#1` (representative) plus `task#2`, `task#3`, `task#4`, `task#5`, `task#6`, `task#10`, `task#14`, `task#64`, `task#130` as further cluster representatives; 52 clusters in total over this one fixture, every one of them a `$.Schemas[i]` set-difference whose only differing member is `LastModified`. Re-replay the whole fixture to confirm, not just the named steps.

## Evidence
Replay verdict: mismatch, expected status 200, actual status 200 — body only.

Representative cluster (fixture `schemas-catalog-list-schemas`, step `task#1`, 10 affected steps, divergence key `service=schemas|ListSchemas|status=200->200|keys=$.Schemas[0]..$.Schemas[7]`):
  $.Schemas[0]: expected {'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'LastModified': '2026-07-08T18:12:15Z', 'VersionCount': 1, 'tags': {}}
                got      {'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'LastModified': '2019-12-01T00:00:00Z', 'VersionCount': 1, 'tags': {}}
  $.Schemas[1]: expected LastModified '2026-07-08T18:11:51Z' for 'aws.a4b@AWSConsoleActionviaCloudTrail', got '2019-12-01T00:00:00Z'
  $.Schemas[4]: expected LastModified '2026-07-08T18:03:08Z' for 'aws.a4b@AWSNetworkActivityEventviaCloudTrail', got '2019-12-01T00:00:00Z'

Other clusters show the same single-member difference with DIFFERENT expected instants, which is what rules out a uniform offset and shows the value is per-entry catalogue data:
  step `task#6`: 'aws.acm@ACMCertificateApproachingExpiration' expected '2026-07-08T18:19:48Z'; 'aws.acm@ACMCertificateRevokedByCA' expected '2026-03-12T21:12:27Z' — two entries of the SAME source, months apart; emulator answers '2019-12-01T00:00:00Z' for both.
  step `task#64`: 'aws.ds@DirectoryServiceDomainControllerDiscoveryUpdate' expected '2026-07-08T18:19:48Z', got '2019-12-01T00:00:00Z'.
  step `task#130`: 'aws.maester@DocumentCreated' expected '2026-07-08T18:16:03Z', 'aws.maester@DocumentDeleted' expected '2026-07-08T18:24:53Z', 'aws.maester@DocumentUpdated' expected '2026-07-08T18:03:07Z' — three instants within one entry group; emulator answers one value for all three.

In every cluster `SchemaName`, `VersionCount` and `tags` match exactly and the element is reported as both 'missing' and 'extra' purely because `LastModified` differs. The exact recorded instants above are the recording's catalogue data filling the template, not the requirement; the requirement is that the instant is seeded per catalogue entry (one instant per publication pass, stable across requests) instead of one constant for the whole catalogue.

Source of the constant: `CATALOGUE_PUBLICATION_INSTANT = "2019-12-01T00:00:00Z"` in `services/service-com-amazonaws-schemas-schemas/src/state.rs`, used by `managed_last_modified` for every name not listed in `MANAGED_PUBLICATIONS`.

Not covered by an existing issue: the three other open schemas wire issues own different replay keys — `20261029T203553Z-schemas-empty-body-204-responses-omit-the-content-length.md` (204 Content-Length framing), `20261118T060835Z-schemas-response-payload-framing-still-diverges-from-aws.md` (GetCodeBindingSource/TagResource framing) and `20261122T071708Z-schemas-aws-managed-cloudtrail-document-still-diverges-on.md` (DescribeSchema/ExportSchema document text). No open issue carries a `schemas-catalog-list-schemas` key, and the replay reports `owned_by: []` for all 52 clusters here.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/CLI at it.
2. List the service-owned catalogue registry, paging through it:
   aws schemas list-schemas --registry-name aws.events --limit 10 --endpoint-url <emulator>
   (equivalently GET /v1/registries/name/aws.events/schemas?limit=10, then follow nextToken).
3. Inspect `Schemas[*].LastModified` on any page: every entry reports the identical instant 2019-12-01T00:00:00Z (only the few names seeded in `MANAGED_PUBLICATIONS` differ).
4. Compare with the recorded AWS response for the same page: each entry carries its own published instant, with entries of one publication pass sharing a whole-second instant and entries of different passes minutes/months apart.
5. Deterministic replay: replay fixture `schemas-catalog-list-schemas`, step `task#1` (and `task#2`..`task#130` listed above). 52 clusters mismatch, all with expected status 200 / actual status 200 and `$.Schemas[i]` differing only in `LastModified`.

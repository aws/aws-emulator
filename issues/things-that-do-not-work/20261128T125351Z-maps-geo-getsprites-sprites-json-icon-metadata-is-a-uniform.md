Issue-Source: trace-sweep-divergence
Issue-Trace: geo-maps-sprites-style-color-variants
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-maps-sprites-style-color-variants geo-maps-vector-basemap-style-workflow
Issue-Shape-Id: com.amazonaws.geomaps#MapsService
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-AU:S.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-HU:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-HU:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:JP.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:JP.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:JP.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:national.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:prefectural.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:prefectural.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-JP:prefectural.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway-long.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway-long.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway-long.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:expressway.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:national.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-KR:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL-ring.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL-ring.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL:binnenstedelijke_ring.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL:binnenstedelijke_ring.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-NL:binnenstedelijke_ring.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PH:E.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PH:E.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:motorway.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:motorway.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:motorway.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:national.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-PK:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-SA:national.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-SA:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-SA:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway-long.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway-long.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TR:motorway.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:expressway.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:expressway.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:expressway.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:freeway.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:freeway.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:freeway.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:provincial.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:provincial.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-TW:provincial.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-VE:T.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-VE:T.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-VE:T.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-ch:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-ch:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-cy:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-cy:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-md:national.height
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-md:national.width
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-md:national.x
Issue-Key: maps.geo|GetSprites|2xx->2xx|$.shield-md:national.y
Issue-Related: 20261125T151416Z-maps-geo-serves-emulator-authored-basemap-assets-instead.md
Issue-Commit: 405f53c3c47d4a5551f1b3a13eff3cb64fdeab94

# GetSprites sprites.json icon metadata is a uniform 20x20 grid instead of per-icon packed atlas geometry

> _Note: this issue was identified against the code base at commit `405f53c3c47d4a5551f1b3a13eff3cb64fdeab94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSprites

## Description
The emulator answers GetSprites for the JSON sprite index (FileName 'sprites.json'/'sprites@2x.json') with a synthesized index in which EVERY icon entry carries the same width/height and an x offset taken from a fixed stride, instead of the real per-icon dimensions and the packed offsets of the sprite atlas the accompanying PNG actually contains. Both causes below are the same defect reached through two different fixtures/styles; they are bundled because one fix (making the sprite index describe the atlas that is served) closes both.

Cause 1 of 2 - geo-maps-sprites-style-color-variants#task#3 (GetSprites, sprites JSON index for a style/color-scheme variant):
  AWS: recorded behavior only. Smithy: GetSprites input FileName @pattern ^sprites(@2x)?\.(png|json)$ (httpLabel), Style/ColorScheme/Variant httpLabels; output is an opaque httpPayload Blob plus Content-Type/Cache-Control/ETag headers, and the model says nothing about the body's structure - so the requirement is the recorded one. The recording shows a MapLibre sprite index: an object keyed by icon name where each entry's width/height are that icon's own pixel dimensions and x/y are its position inside the packed atlas, so entries differ from one another (e.g. the recorded index gives distinct heights such as 19, 23, 28 and 30 and distinct widths such as 19, 21, 24, 25 and 28 for the various road-shield icons, with x offsets that advance by each preceding icon's real width). Status 200.
  Emulator: answers 200, but every icon entry is emitted as a fixed 20x20 box on a regular stride: the replay reports width/height 20 where the recording has 19/21/24/25/28 (width) and 19/22/23/28/30 (height), and x offsets that are off by a growing amount (1, 2, 3, ... up to 14+ px) because each entry advances by the constant placeholder width rather than by the real one. The whole icon/shield name space is affected in one response (several hundred diverging members in this one step).
  Why: a client loads sprites.json together with sprites.png and uses width/height/x/y to crop each icon out of the atlas. With a uniform grid, every crop is taken from the wrong rectangle, so icon rendering for markers, one-way arrows and road shields is garbled or blank, and any client that validates the index against the atlas size sees inconsistent geometry. Code that merely fetches the sprite sheet still 'works', which makes the breakage silent.
  Fix: the GetSprites handler/serializer for the JSON sprite index must emit the index of the atlas it serves - the per-icon width/height and the packed x/y of the actual sprite sheet asset - rather than generating entries from a constant tile size and stride.
  Traces: geo-maps-sprites-style-color-variants#task#3.

Cause 2 of 2 - geo-maps-vector-basemap-style-workflow#task#3 (GetSprites, sprites JSON index fetched as part of a full basemap style workflow):
  AWS: recorded behavior only (same modeled surface as above: opaque Blob payload, structure fixed by the recording). The recorded index again gives per-icon dimensions and packed offsets, including entries whose width/height differ from the common case and whose y offset is on a later atlas row.
  Emulator: answers 200 with the same uniform 20x20 / fixed-stride index; the replay reports width/height 20 against recorded 19/21/24/25/28-wide and 22/23/28/30-high icons and x drift of up to ~14 px, plus y divergences on the later rows of the atlas.
  Why: same client impact as cause 1, reached through the ordinary 'get style descriptor, then fetch glyphs/sprites/tiles' sequence a map SDK performs, so the defect shows up on the normal basemap path and not only when sprites are requested directly.
  Fix: same component as cause 1 - the sprite-index generation behind GetSprites must be derived from the served atlas instead of a synthetic grid; the fix must hold for every Style/ColorScheme/Variant combination the corpus exercises, not just the one in cause 1.
  Traces: geo-maps-vector-basemap-style-workflow#task#3.

Relationship to already-open work: the open issue 20261125T151416Z-maps-geo-serves-emulator-authored-basemap-assets-instead.md owns the GetGlyphs/GetSprites binary payload divergences (__wire_sha256__/__wire_length__) and the GetStyleDescriptor divergences, and its replay keys cover a subset of the sprite-index members (marker*/shield-BR/shield-US-* and similar). It does NOT cover the members these two steps diverge on (for example $.shield-AU:S.height, $.shield-HU:national.width/x, $.shield-JP:*, $.shield-KR:*, $.shield-TW:*, $.shield-hgv-*), which is why the replay reports these two clusters as unowned. They must be closed on their own merit.

## Evidence
Replay of trace set simparity-wire-aws-v1 (12 steps, 0 passed, 12 mismatched, bootstrap_signal false, emulator_up true).

Cluster 1 - fixture geo-maps-sprites-style-color-variants, step task#3, service maps.geo, operation GetSprites, verdict mismatch, expected status 200, actual status 200, owned_by [] (net-new), stub_path empty.
  Sample of the reported member divergences (expected -> actual):
    $.shield-AU:S.height: 19 -> 20
    $.shield-Emirates routes.width: 21 -> 20 ; .height: 30 -> 20
    $.shield-HU:national.width: 21 -> 20 ; .x: 1765 -> 1764
    $.shield-IN-National.width: 19 -> 20 ; .height: 30 -> 20 ; .x: 1814 -> 1811
    $.shield-IN-National-wide.width: 28 -> 27 ; .height: 30 -> 20 ; .x: 1786 -> 1784
    $.shield-JP:JP.width: 24 -> 20 ; .height: 23 -> 20 ; .x: 1852 -> 1851
    $.shield-JP:prefectural.width: 25 -> 20 ; .height: 22 -> 20 ; .x: 1900 -> 1891
    $.shield-KR:expressway.width: 24 -> 20 ; .height: 28 -> 20 ; .x: 1953 -> 1938
    $.shield-KR:expressway-long.x: 1925 -> 1911
  The cluster's key list continues across the whole icon namespace ($.shield-MX-*, $.shield-MY, $.shield-NL*, $.shield-PK:*, $.shield-SA:national, $.shield-TR:*, $.shield-TW:*, $.shield-US-*, $.shield-hgv-* and the plain POI icons $.shopping/$.ski/$.stadium/$.taxi/$.train/$.truck-* on .x/.y). Pattern: every actual width/height is the constant 20 and every actual x/y lags the recorded packed offset by the accumulated difference between real and placeholder widths.

Cluster 2 - fixture geo-maps-vector-basemap-style-workflow, step task#3, service maps.geo, operation GetSprites, verdict mismatch, expected status 200, actual status 200, owned_by [] (net-new), stub_path empty.
  Same shape of divergence over the same icon namespace, e.g. $.shield-AU:S.height 19 -> 20, $.shield-JP:national.height 23 -> 20 and .x 1876 -> 1871, $.shield-KR:expressway-long.height 28 -> 20 and .x 1925 -> 1911, $.shield-truck-parking.width/height/x/y, $.shield-rectangle-*/$.shield-kite-*/$.shield-sadc on width/height/x/y.

The two clusters are distinct replay clusters (different fixtures and different exact key sets) but one emulator behavior: the sprite index is generated from a constant tile size and stride rather than read off the served atlas.

No environment-specific value is asserted here: hosts, ports, request ids and timestamps are excluded; only icon geometry (width/height/x/y), which is a property of the served asset, is cited.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: replay fixture geo-maps-sprites-style-color-variants, step task#3 - GET /v2/styles/{Style}/{ColorScheme}/{Variant}/sprites/sprites.json for the style/color-scheme variant that step uses. Parse the 200 response body as a MapLibre sprite index and compare each entry's width/height/x/y with the recorded response.
   Observed: every entry has width=20 and height=20 and x values on a fixed stride; recorded entries have per-icon dimensions (19/21/24/25/28 wide, 19/22/23/28/30 high) and packed offsets.
3. Cause 2: replay fixture geo-maps-vector-basemap-style-workflow, step task#3 - the sprites.json fetch performed inside the basemap workflow (GetStyleDescriptor, then glyphs/sprites/tiles). Compare the same members; the same uniform-grid index is returned.
4. Cross-check against the atlas: fetch the matching sprites.png for the same Style/ColorScheme/Variant and crop using the index the emulator returned - the crops do not line up with the icons in the image, confirming the index does not describe the served atlas.
5. The fix is confirmed when both geo-maps-sprites-style-color-variants#task#3 and geo-maps-vector-basemap-style-workflow#task#3 replay with no member divergences.

Issue-Source: trace-sweep-divergence
Issue-Trace: location-map-sprites-missing-map
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-map-sprites-missing-map location-map-style-descriptor-rejects-malformed-map-name location-search-place-index-for-text-missing-index-not-found
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|GetMapSprites|2xx->2xx|$.Special area of interest/Rock or gravel
Issue-Key: location|GetMapSprites|2xx->2xx|$.Water area/Ice mass
Issue-Key: location|GetMapSprites|2xx->2xx|$.Water area/Swamp or marsh
Issue-Key: location|GetMapStyleDescriptor|2xx->2xx|$.sources.esri.attribution
Issue-Key: location|SearchPlaceIndexForText|2xx->2xx|$.Results[*].Place.Label
Issue-Key: location|SearchPlaceIndexForText|2xx->2xx|$.Results[*].Place.Municipality
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# location map and place reference data diverges from AWS: sprite sheet entries, style attribution, text-search result

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMapSprites

## Description
THREE causes, one shared fault: the location crate serves its own synthesised reference data for the three read-only data-plane payloads whose CONTENT is the whole answer (the sprite-offsets document, the style descriptor and a place-index text search), so each 200 response has the right shape but content Amazon Location does not serve. The model describes none of these payload bodies, so each cause below is recorded behavior only.

Cause 1 of 3: the sprites.json offsets document omits the sprite entries the map's style publishes.
AWS: Smithy: GetMapSprites GET /maps/v0/maps/{MapName}/sprites/{FileName}, FileName @pattern ^sprites(@2x)?\.(png|json)$, output Blob as the httpPayload — recorded behavior only for the body. For a sprites.json request, AWS serves the offsets document of the sheet that map style publishes: a JSON object keyed by the style's icon names, each value {x, y, width, height, pixelRatio, sdf}, with one entry per icon in the sheet and the x/y offsets tiling the PNG.
Emulator: a 200 sprites.json whose object is missing the style's icon entries altogether — the recorded keys (e.g. the 'Water area/...' and 'Special area of interest/...' icon families) are absent, so a renderer finds no sprite for them.
Why: a map client reads this document to locate each icon in the sprite sheet; a missing key means the icon is simply not drawn, and nothing in the response signals the omission.
Fix: the sprite-sheet projection in the location crate's state (the map resource's stored style -> sprite sheet derivation that GetMapSprites serves) — publish one offsets entry per icon the style declares, with offsets consistent with the PNG it serves.
Traces: location-map-sprites-missing-map#task#2.

Cause 2 of 3: the style descriptor's source attribution is a placeholder.
AWS: Smithy: GetMapStyleDescriptor GET /maps/v0/maps/{MapName}/style-descriptor, output Blob as the httpPayload — recorded behavior only for the body. The descriptor's sources.<source>.attribution is the provider's full legal attribution string for the style's data source, an HTML anchor to the provider's data-attributions page followed by the comma-separated list of contributing data providers (for the Esri source: an <a style='color:#0073bb;' href='https://www.esri.com/en-us/legal/terms/data-attributions'>Esri</a> link followed by the other named providers, ending with the OpenStreetMap credit).
Emulator: the same descriptor with sources.esri.attribution reduced to the bare placeholder '© Esri'.
Why: the attribution string is the text a map client is required to render over the map; a placeholder means an application built against the emulator displays an attribution that does not satisfy the provider's terms, and any client parsing the anchor for a link gets none.
Fix: the style-descriptor document the location crate's map state serves — carry the provider's full attribution string per source.
Traces: location-map-style-descriptor-rejects-malformed-map-name#task#2.

Cause 3 of 3: SearchPlaceIndexForText does not resolve the request's Text — it answers with a fixed place.
AWS: Smithy: SearchPlaceIndexForText POST /places/v0/indexes/{IndexName}/search/text, Text required (@length 1..200), output Results required — recorded behavior only for the result content. Each result's Place describes the place the REQUEST's Text resolves to: Place.Municipality is that place's municipality and Place.Label is its formatted label, so searching for one city returns that city's name in both, not some other place's.
Emulator: a 200 with a well-shaped Results list whose first Place describes a place unrelated to the query text — the recorded run asked for one city and got a different, apparently canned, locality in both Label and Municipality.
Why: the operation's entire purpose is text -> place resolution; a client that geocodes a user's query receives a confidently-shaped answer for the wrong place, which is worse than an empty result because nothing marks it as unrelated.
Fix: the place-index search path in the location crate (the text query -> stored/synthesised place matching that SearchPlaceIndexForText serves) — derive the result's Place from the request's Text instead of a fixed entry.
Traces: location-search-place-index-for-text-missing-index-not-found#task#2.

## Evidence
Cause 1 — location-map-sprites-missing-map#task#2 (GetMapSprites, 200 both sides): three keys of the sprites.json object were expected and absent: '$.Special area of interest/Rock or gravel' expected {'x': 0, 'y': 0, 'width': 64, 'height': 64, 'pixelRatio': 1, 'sdf': False}, '$.Water area/Ice mass' expected {'x': 64, 'y': 0, ...} and '$.Water area/Swamp or marsh' expected {'x': 128, 'y': 0, ...}; all three got None. (The x/y offsets are one recording's packing of the sheet, not a requirement — the requirement is that an entry exists for every icon the style publishes and that its offsets match the PNG served alongside.)
Cause 2 — location-map-style-descriptor-rejects-malformed-map-name#task#2 (GetMapStyleDescriptor, 200 both sides): $.sources.esri.attribution expected "<a style='color:#0073bb;' href='https://www.esri.com/en-us/legal/terms/data-attributions'>Esri</a>, TomTom, Garmin, FAO, NOAA, USGS, © OpenStreetMap", got '© Esri'.
Cause 3 — location-search-place-index-for-text-missing-index-not-found#task#2 (SearchPlaceIndexForText, 200 both sides): $.Results[0].Place.Label expected 'Spokane, WA, USA' got 'SeaTac, Washington, 98158, USA'; $.Results[0].Place.Municipality expected 'Spokane' got 'SeaTac'. The query text in that step is the city in the expected values; the actual values name a different locality and do not vary with it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay location-map-sprites-missing-map, location-map-style-descriptor-rejects-malformed-map-name and location-search-place-index-for-text-missing-index-not-found.
2. Cause 1: CreateMap, then GetMapSprites(MapName=<map>, FileName='sprites.json'). Parse the JSON body and check that every icon name the map's style publishes has an entry with x/y/width/height/pixelRatio/sdf; the recorded 'Water area/...' and 'Special area of interest/...' keys are missing today.
3. Cause 2: GetMapStyleDescriptor(MapName=<map>) and read sources.<source>.attribution — it must be the provider's full attribution string (anchor to the provider's data-attributions page plus the contributing providers), not '© Esri'.
4. Cause 3: CreatePlaceIndex, then SearchPlaceIndexForText(IndexName=<index>, Text=<a city name>) and compare Results[0].Place.Municipality / Place.Label with the city asked for; repeat with a second, different city and observe the response does not change with the query.

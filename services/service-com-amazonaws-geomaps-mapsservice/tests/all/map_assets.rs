// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Geo Maps map-asset routes.
//!
//! Every case drives a real HTTP request through the router the emulator mounts,
//! so what is asserted is the wire behaviour: status, headers and body bytes.
//!
//! The rules these tests pin, each over a set of inputs rather than one
//! recorded request:
//!
//! * a style descriptor is the stored style record projected through the
//!   request's knobs, and the templates it hands back are routes this service
//!   serves (the MapLibre bootstrap: descriptor -> tiles, sprites, glyphs);
//! * every asset's `ETag` is a hash of the bytes returned, so a repeat request
//!   is byte-identical and carries the same `ETag`;
//! * every asset's `Content-Type`, `Cache-Control` and `PricingBucket` come
//!   from the catalog in engine state, not from the handler;
//! * an address the catalog does not carry is a `ResourceNotFoundException`,
//!   and a parameter outside a documented rule is a `ValidationException`.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use prost::Message as _;
use service_com_amazonaws_geomaps_mapsservice::ops::types::{
    COLOR_SCHEME_VALUES, MAP_STYLE_VALUES, TILE_ADDITIONAL_FEATURE_VALUES, VARIANT_VALUES,
};
use service_com_amazonaws_geomaps_mapsservice::state::assets;
use service_com_amazonaws_geomaps_mapsservice::state::vector_tile::Tile;
use service_com_amazonaws_geomaps_mapsservice::state::{
    route_shield_box, route_shield_width, ColorSchemeKind, Hsla, MapStyleKind, TileFormat,
    VariantKind, CITY_DOT_BOX, CITY_DOT_OPEN_BOX, INCIDENT_ICON_BOX, MARKER_BOX, PLACE_ICON_BOX,
    RESTRICTION_SHIELD_BOX, SHIELD_BASE_WIDTH, SHIELD_DIGIT_WIDTH, SPRITE_SHEET_WIDTH,
};
use service_com_amazonaws_geomaps_mapsservice::{
    ComAmazonawsGeomapsMapsserviceState, EmulatedService,
};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host a `geo-maps` client addresses; its first label is the
/// signing name the router routes on.
pub(crate) const HOST: &str = "geo-maps.us-east-1.amazonaws.com";

/// One emulator: the single shared engine plus the router that serves it.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    pub(crate) router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise the maps routes, not SigV4; the signature path
        // has its own tests in the framework.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The service's asset catalog, as a handler sees it.
    pub(crate) fn catalog(&self) -> engine::ServiceState<ComAmazonawsGeomapsMapsserviceState> {
        self.engine
            .state::<ComAmazonawsGeomapsMapsserviceState>()
    }

    pub(crate) async fn get(&self, path_and_query: &str) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(format!("https://{HOST}{path_and_query}"))
            .header(http::header::HOST, HOST)
            .body(Bytes::new())
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }
}

pub(crate) fn header(response: &http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The media type the catalog says a STORED asset (a glyph range, a sprite
/// file) is delivered with. Read from state rather than written out here, so
/// these cases pin "a stored asset's media type is the object's, not the
/// payload's" rather than one spelling of it.
fn delivered_content_type(emulator: &Emulator) -> String {
    let catalog = emulator.catalog();
    let content_type = catalog.lock().asset_delivery.content_type.clone();
    content_type
}

/// The wire error code of a failed response: restJson1 carries it in
/// `x-amzn-errortype` and in the body's `__type`.
fn error_code(response: &http::Response<Bytes>) -> String {
    let from_header = header(response, "x-amzn-errortype");
    if !from_header.is_empty() {
        return from_header.rsplit('#').next().unwrap_or_default().to_string();
    }
    serde_json::from_slice::<serde_json::Value>(response.body())
        .ok()
        .and_then(|body| {
            body.get("__type")
                .and_then(serde_json::Value::as_str)
                .map(|kind| kind.rsplit('#').next().unwrap_or_default().to_string())
        })
        .unwrap_or_default()
}

/// The body as the UTF-8 text an error message is reported in.
pub(crate) fn body_text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

pub(crate) fn assert_ok(case: &str, response: &http::Response<Bytes>) {
    assert_eq!(
        response.status(),
        200,
        "case {case:?}: expected 200, got {} with body {}",
        response.status(),
        body_text(response)
    );
}

/// `ETag` as this service computes it: a quoted 128-bit prefix of the body's
/// SHA-256. Recomputed here from the bytes on the wire, so the test checks the
/// header against the body it was served with.
pub(crate) fn expected_etag(body: &[u8]) -> String {
    let digest = sha2::Sha256::digest(body);
    format!("\"{}\"", hex::encode(&digest[..16]))
}

/// The path and query of an absolute URL, which is what the router dispatches.
pub(crate) fn path_of(url: &str) -> String {
    let uri: http::Uri = url.parse().unwrap_or_else(|error| {
        panic!("case {url:?}: a style template must be a URL, got {error}");
    });
    uri.path_and_query()
        .map(ToString::to_string)
        .unwrap_or_else(|| uri.path().to_string())
}

pub(crate) fn expand_tile_template(template: &str, z: u32, x: u32, y: u32) -> String {
    template
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
}

/// Decode a PNG into `(width, height, rgba)`.
fn decode_png(case: &str, bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .unwrap_or_else(|error| panic!("case {case:?}: body must be a PNG, got {error}"));
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .unwrap_or_else(|error| panic!("case {case:?}: PNG frame must decode, got {error}"));
    buffer.truncate(info.buffer_size());
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "case {case:?}: expected an RGBA PNG, got {:?}",
        info.color_type
    );
    (info.width, info.height, buffer)
}

fn png_pixel(width: u32, rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    [
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    ]
}

/// Decode a JPEG into `(width, height, rgb)`.
fn decode_jpeg(case: &str, bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    let pixels = decoder
        .decode()
        .unwrap_or_else(|error| panic!("case {case:?}: body must be a JPEG, got {error}"));
    let info = decoder
        .info()
        .unwrap_or_else(|| panic!("case {case:?}: a decoded JPEG must report its dimensions"));
    (u32::from(info.width), u32::from(info.height), pixels)
}

// ---------------------------------------------------------------------------
// GetStyleDescriptor, and the bootstrap fan-out it drives
// ---------------------------------------------------------------------------

/// Every (style, colour scheme) the catalog publishes answers with the stored
/// record's document, and every template in that document resolves to a route
/// this service serves: the MapLibre bootstrap a map viewer performs on load.
#[tokio::test]
async fn a_style_descriptor_bootstraps_tiles_sprites_and_glyphs() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let path = format!(
                "/v2/styles/{}/descriptor?color-scheme={}",
                style.as_wire(),
                scheme.as_wire()
            );
            let response = emulator.get(&path).await;
            assert_ok(&case, &response);
            assert_eq!(
                header(&response, "content-type"),
                "application/json",
                "case {case:?}: a style descriptor is a JSON document"
            );
            assert!(
                !header(&response, "cache-control").is_empty(),
                "case {case:?}: the catalog's cache directive must be reported"
            );
            assert_eq!(
                header(&response, "etag"),
                expected_etag(response.body()),
                "case {case:?}: the ETag must hash the body served"
            );

            let document: serde_json::Value = serde_json::from_slice(response.body())
                .unwrap_or_else(|error| panic!("case {case:?}: body must be JSON, got {error}"));
            assert_eq!(
                document["version"], 8,
                "case {case:?}: the style specification version"
            );
            let expected_name = format!(
                "AmazonLocation-{}-Global-{}-{}",
                style.as_wire(),
                scheme.as_wire(),
                VariantKind::Default.as_wire()
            );
            assert_eq!(
                document["name"].as_str(),
                Some(expected_name.as_str()),
                "case {case:?}: the document must name the stored style record"
            );

            // The sources the catalog holds for this style, followed to GetTile.
            let sources = document["sources"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare sources"));
            assert!(
                !sources.is_empty(),
                "case {case:?}: a style must declare at least one source"
            );
            for (name, source) in sources {
                let template = source["tiles"][0]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: source {name} needs a tile URL"));
                assert!(
                    template.starts_with(&format!("https://{HOST}/v2/tiles/")),
                    "case {case:?}: source {name} must point at this endpoint's GetTile, got {template}"
                );
                let tile = emulator
                    .get(&path_of(&expand_tile_template(template, 3, 4, 5)))
                    .await;
                assert_ok(&format!("{case} source {name}"), &tile);
                assert!(
                    !header(&tile, "x-amz-geo-pricing-bucket").is_empty(),
                    "case {case:?}: source {name}'s tiles must report a pricing bucket"
                );
            }

            // The sprite sheet and its index, followed to GetSprites. Both are
            // stored assets, so both are delivered with the object store's own
            // media type rather than the media type of what is inside them.
            let sprite = document["sprite"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare a sprite URL"));
            let delivered = delivered_content_type(&emulator);
            for suffix in [".png", ".json"] {
                let response = emulator.get(&path_of(&format!("{sprite}{suffix}"))).await;
                assert_ok(&format!("{case} sprite {suffix}"), &response);
                assert_eq!(
                    header(&response, "content-type"),
                    delivered,
                    "case {case:?}: sprite {suffix} must be served as {delivered}"
                );
            }

            // The glyph range, followed to GetGlyphs with the font stack the
            // document's own symbol layers ask for.
            let glyphs = document["glyphs"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a style must declare a glyph URL"));
            for font in font_stacks(&document) {
                let url = glyphs
                    .replace("{fontstack}", &font.replace(' ', "%20"))
                    .replace("{range}", "0-255");
                let response = emulator.get(&path_of(&url)).await;
                assert_ok(&format!("{case} glyphs {font}"), &response);
                assert!(
                    !response.body().is_empty(),
                    "case {case:?}: glyph range {font} must carry glyphs"
                );
            }
        }
    }
}

/// Every template a style document hands back is ABSOLUTE, rooted at the
/// endpoint the request addressed: a client that does not resolve a relative
/// URL against the endpoint it called must still be able to follow them. The
/// scheme is the one the request stated — on the URI or in
/// `x-forwarded-proto` — and `https` when it stated neither, which is how this
/// emulator's own listener is reached.
#[tokio::test]
async fn style_templates_are_absolute_urls_rooted_at_the_request_endpoint() {
    let emulator = Emulator::new();
    // Row = (how the request addressed the emulator, x-forwarded-proto), and
    // the prefix every template in the answer must start with.
    let cases: [(&str, Option<&str>, String); 6] = [
        // An absolute URI states its own scheme, either way.
        (
            "https://geo-maps.us-east-1.amazonaws.com/v2/styles/Standard/descriptor",
            None,
            format!("https://{HOST}/v2/"),
        ),
        (
            "http://geo-maps.us-east-1.amazonaws.com/v2/styles/Standard/descriptor",
            None,
            format!("http://{HOST}/v2/"),
        ),
        // Origin-form, the shape a server really receives: no scheme on the
        // URI, so the templates carry the one this emulator is served over...
        (
            "/v2/styles/Standard/descriptor",
            None,
            format!("https://{HOST}/v2/"),
        ),
        // ...unless a proxy states another.
        (
            "/v2/styles/Standard/descriptor",
            Some("http"),
            format!("http://{HOST}/v2/"),
        ),
        (
            "/v2/styles/Standard/descriptor",
            Some("HTTPS"),
            format!("https://{HOST}/v2/"),
        ),
        // A value that is no scheme is not pasted into the document.
        (
            "/v2/styles/Standard/descriptor",
            Some("javascript"),
            format!("https://{HOST}/v2/"),
        ),
    ];

    for (uri, forwarded, prefix) in cases {
        let case = format!("{uri} forwarded={forwarded:?}");
        let mut builder = http::Request::builder()
            .method(http::Method::GET)
            .uri(uri)
            .header(http::header::HOST, HOST);
        if let Some(proto) = forwarded {
            builder = builder.header("x-forwarded-proto", proto);
        }
        let request = builder
            .body(Bytes::new())
            .expect("a well-formed test request");
        let response = emulator.router.dispatch(request).await;
        assert_ok(&case, &response);
        let document: serde_json::Value = serde_json::from_slice(response.body().as_ref())
            .unwrap_or_else(|error| panic!("case {case:?}: must be JSON, got {error}"));

        let mut templates: Vec<String> = Vec::new();
        for source in document["sources"].as_object().into_iter().flatten() {
            if let Some(tiles) = source.1["tiles"][0].as_str() {
                templates.push(tiles.to_string());
            }
        }
        for key in ["sprite", "glyphs"] {
            if let Some(value) = document[key].as_str() {
                templates.push(value.to_string());
            }
        }
        assert!(
            templates.len() >= 3,
            "case {case:?}: expected tile, sprite and glyph templates, got {templates:?}"
        );
        for template in &templates {
            assert!(
                template.starts_with(&prefix),
                "case {case:?}: expected a template starting {prefix}, got {template}"
            );
        }
    }
}

/// Every font stack the document's symbol layers reference.
fn font_stacks(document: &serde_json::Value) -> BTreeSet<String> {
    let mut stacks = BTreeSet::new();
    for layer in document["layers"].as_array().into_iter().flatten() {
        for font in layer["layout"]["text-font"].as_array().into_iter().flatten() {
            if let Some(name) = font.as_str() {
                stacks.insert(name.to_string());
            }
        }
    }
    stacks
}

/// Each feature knob changes the document the way the catalog says it should,
/// and every layer it adds reads a `source-layer` that really is in the tile
/// the layer's own source template serves.
#[tokio::test]
async fn style_knobs_add_layers_whose_tiles_carry_them() {
    let emulator = Emulator::new();
    // Row = one query string, and what it must put in the document. A layer is
    // drawn because the tiles behind it carry it, so the ids are the tile layer
    // names of the families each knob switches on.
    let cases: [(&str, &[&str]); 7] = [
        ("color-scheme=Dark", &[]),
        ("terrain=Hillshade", &["hillshade"]),
        ("terrain=Terrain3D", &[]),
        ("contour-density=High", &["contour_lines"]),
        (
            "traffic=All",
            &["traffic_flow", "traffic_incidents", "traffic_incident_icons"],
        ),
        (
            "travel-modes=Transit&travel-modes=Truck",
            &["road_truck", "shield_truck-restriction"],
        ),
        (
            "political-view=IND&buildings=Buildings3D",
            &[
                "admin_country",
                "admin_country-disputed",
                "admin_country-disputed-mask",
                "building_extrusion",
            ],
        ),
    ];

    for (query, expected_layers) in cases {
        let case = format!("Standard?{query}");
        let response = emulator
            .get(&format!("/v2/styles/Standard/descriptor?{query}"))
            .await;
        assert_ok(&case, &response);
        let document: serde_json::Value = serde_json::from_slice(response.body())
            .unwrap_or_else(|error| panic!("case {case:?}: body must be JSON, got {error}"));

        let ids: BTreeSet<String> = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|layer| layer["id"].as_str().map(ToString::to_string))
            .collect();
        for expected in expected_layers {
            assert!(
                ids.contains(*expected),
                "case {case:?}: expected a layer {expected}, got {ids:?}"
            );
        }

        // Every layer's source-layer must exist in the tile its source serves.
        for layer in document["layers"].as_array().into_iter().flatten() {
            let (Some(source), Some(source_layer)) = (
                layer["source"].as_str(),
                layer["source-layer"].as_str(),
            ) else {
                continue;
            };
            let template = document["sources"][source]["tiles"][0]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: source {source} needs a tile URL"));
            let tile = emulator
                .get(&path_of(&expand_tile_template(template, 2, 1, 1)))
                .await;
            assert_ok(&format!("{case} tile for {source}"), &tile);
            let decoded = Tile::decode(tile.body().as_ref()).unwrap_or_else(|error| {
                panic!("case {case:?}: source {source} must serve a vector tile, got {error}")
            });
            let names: BTreeSet<&str> = decoded
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect();
            assert!(
                names.contains(source_layer),
                "case {case:?}: layer {} reads source-layer {source_layer}, which tile {source} does not carry: {names:?}",
                layer["id"]
            );
        }
    }
}

/// `PoiDensity: Off` is the one value that removes the POI symbol layer; every
/// other value keeps it and only changes its spacing.
#[tokio::test]
async fn poi_density_off_removes_the_symbol_layer() {
    let emulator = Emulator::new();
    // The model's `PoiDensity` values.
    for density in [
        "Default",
        "Dense",
        "Off",
        "Sparse",
        "VeryDense",
        "VerySparse",
    ] {
        let response = emulator
            .get(&format!(
                "/v2/styles/Standard/descriptor?poi-density={density}"
            ))
            .await;
        assert_ok(density, &response);
        let document: serde_json::Value =
            serde_json::from_slice(response.body()).expect("a JSON style document");
        let has_places = document["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|layer| layer["id"] == "poi_100_food_drink");
        assert_eq!(
            has_places,
            density != "Off",
            "case {density:?}: expected the places layer to be present={}, got {has_places}",
            density != "Off"
        );
    }
}

/// A knob value outside the model's own set is refused, not silently dropped
/// from the document it was meant to change.
#[tokio::test]
async fn unknown_style_knob_values_are_refused() {
    let emulator = Emulator::new();
    for query in [
        "color-scheme=Beige",
        "terrain=Nope",
        "contour-density=Extreme",
        "traffic=Some",
        "buildings=Flat",
        "poi-density=Massive",
        "travel-modes=Hovercraft",
        "poi-categories=Volcanoes",
    ] {
        let response = emulator
            .get(&format!("/v2/styles/Standard/descriptor?{query}"))
            .await;
        assert_eq!(
            response.status(),
            400,
            "case {query:?}: expected 400, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }
    // And a style label the model's own enum does not carry.
    let response = emulator.get("/v2/styles/Nope/descriptor").await;
    assert_eq!(
        response.status(),
        400,
        "case \"Style=Nope\": expected 400, got {} with body {}",
        response.status(),
        body_text(&response)
    );
}

// ---------------------------------------------------------------------------
// GetTile
// ---------------------------------------------------------------------------

/// Every tileset in the catalog serves the format, media type, cache directive
/// and pricing bucket its record carries, and a repeated request is
/// byte-identical with the same `ETag`.
#[tokio::test]
async fn tiles_are_served_from_the_tileset_record_and_are_stable() {
    let emulator = Emulator::new();
    let tilesets: Vec<_> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .tilesets
            .values()
            .map(|record| {
                (
                    record.id.clone(),
                    record.format,
                    record.content_type.clone(),
                    record.cache_control.clone(),
                    catalog.pricing_bucket(record.pricing_bucket.as_ref()),
                    record.min_zoom,
                    record.max_zoom,
                )
            })
            .collect()
    };
    assert!(
        tilesets.len() >= 4,
        "the catalog must publish the documented tilesets, got {}",
        tilesets.len()
    );

    for (id, format, content_type, cache_control, pricing_bucket, min_zoom, max_zoom) in tilesets {
        // Rows: the lowest zoom, a middle zoom, and the highest the record has.
        for (z, x, y) in [
            (min_zoom, 0, 0),
            ((min_zoom + max_zoom) / 2, 3, 5),
            (max_zoom, 1, 2),
        ] {
            let case = format!("{id} {z}/{x}/{y}");
            let path = format!("/v2/tiles/{id}/{z}/{x}/{y}");
            let first = emulator.get(&path).await;
            assert_ok(&case, &first);
            assert_eq!(
                header(&first, "content-type"),
                content_type,
                "case {case:?}: the media type must come from the tileset record"
            );
            assert_eq!(
                header(&first, "cache-control"),
                cache_control,
                "case {case:?}: the cache directive must come from the tileset record"
            );
            assert_eq!(
                header(&first, "x-amz-geo-pricing-bucket"),
                pricing_bucket,
                "case {case:?}: the pricing bucket must come from the catalog"
            );
            assert_eq!(
                header(&first, "etag"),
                expected_etag(first.body()),
                "case {case:?}: the ETag must hash the body served"
            );
            assert!(
                !first.body().is_empty(),
                "case {case:?}: a tile must carry bytes"
            );

            // The bytes really are the format the record claims.
            match format {
                TileFormat::VectorMvt => {
                    let tile = Tile::decode(first.body().as_ref()).unwrap_or_else(|error| {
                        panic!("case {case:?}: must decode as a vector tile, got {error}")
                    });
                    assert!(
                        !tile.layers.is_empty(),
                        "case {case:?}: a vector tile must carry layers"
                    );
                    // The tile's own attributes report the address it answers.
                    let layer = &tile.layers[0];
                    let coordinate = |key: &str| {
                        let index = layer.keys.iter().position(|k| k == key)?;
                        let feature = layer.features.first()?;
                        let position = feature
                            .tags
                            .chunks(2)
                            .find(|pair| pair[0] as usize == index)?;
                        layer.values.get(position[1] as usize)?.int_value
                    };
                    assert_eq!(
                        (coordinate("z"), coordinate("x"), coordinate("y")),
                        (Some(i64::from(z)), Some(i64::from(x)), Some(i64::from(y))),
                        "case {case:?}: the tile must carry the address it was asked for"
                    );
                }
                TileFormat::RasterJpeg => {
                    let (width, height, _) = decode_jpeg(&case, first.body());
                    assert_eq!(
                        (width, height),
                        (256, 256),
                        "case {case:?}: a raster tile is 256x256"
                    );
                }
                TileFormat::RasterDemPng => {
                    let (width, height, _) = decode_png(&case, first.body());
                    assert_eq!(
                        (width, height),
                        (256, 256),
                        "case {case:?}: a terrain tile is 256x256"
                    );
                }
            }

            // Repeat: the same address answers with the same bytes.
            let second = emulator.get(&path).await;
            assert_eq!(
                first.body(),
                second.body(),
                "case {case:?}: a repeated request must be byte-identical"
            );
            assert_eq!(
                header(&first, "etag"),
                header(&second, "etag"),
                "case {case:?}: a repeated request must carry the same ETag"
            );
        }
    }
}

/// Neighbouring addresses answer with different tiles: the bytes are a function
/// of the address, not one blob served for everything.
#[tokio::test]
async fn neighbouring_tiles_differ() {
    let emulator = Emulator::new();
    for tileset in ["vector.basemap", "raster.satellite", "raster.dem"] {
        let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
        for (z, x, y) in [(4, 8, 5), (4, 9, 5), (4, 8, 6), (5, 8, 5)] {
            let case = format!("{tileset} {z}/{x}/{y}");
            let response = emulator
                .get(&format!("/v2/tiles/{tileset}/{z}/{x}/{y}"))
                .await;
            assert_ok(&case, &response);
            assert!(
                seen.insert(response.body().to_vec()),
                "case {case:?}: this address answered with bytes another address already served"
            );
        }
    }
}

/// A terrain tile carries a continuous elevation field: the column where two
/// horizontally adjacent tiles meet agrees to within a metre or two, and every
/// elevation is a plausible one.
#[tokio::test]
async fn terrain_tiles_are_a_continuous_elevation_field() {
    let emulator = Emulator::new();
    for (z, x, y) in [(2, 1, 1), (6, 20, 24), (10, 500, 300)] {
        let case = format!("raster.dem {z}/{x}/{y}");
        let left = emulator.get(&format!("/v2/tiles/raster.dem/{z}/{x}/{y}")).await;
        let right = emulator
            .get(&format!("/v2/tiles/raster.dem/{z}/{}/{y}", x + 1))
            .await;
        assert_ok(&case, &left);
        assert_ok(&case, &right);
        let (left_width, _, left_pixels) = decode_png(&case, left.body());
        let (right_width, _, right_pixels) = decode_png(&case, right.body());

        for row in [0_u32, 77, 255] {
            let here = assets::decode_terrarium(png_pixel(
                left_width,
                &left_pixels,
                left_width - 1,
                row,
            ));
            let across = assets::decode_terrarium(png_pixel(right_width, &right_pixels, 0, row));
            // The two samples are one pixel apart across the seam, so a
            // continuous field differs by at most its gradient times that step.
            // The field's steepest term is bounded by ~150 m per degree, and a
            // pixel at zoom z spans 360 / (256 * 2^z) degrees.
            let degrees_per_pixel = 360.0 / (256.0 * 2_f64.powi(z));
            let tolerance = 150.0 * degrees_per_pixel + 1.0;
            assert!(
                (here - across).abs() <= tolerance,
                "case {case:?} row {row}: the seam must be continuous within {tolerance} m, got {here} and {across}"
            );
            assert!(
                (-11_000.0..9_000.0).contains(&here),
                "case {case:?} row {row}: {here} is not a plausible elevation"
            );
        }
    }
}

/// `AdditionalFeatures` adds exactly the layers the model's values name.
#[tokio::test]
async fn additional_features_add_their_own_tile_layers() {
    let emulator = Emulator::new();
    // EVERY value the model declares for `TileAdditionalFeature`, taken from the
    // generated constant rather than written out here, so a value the model adds
    // later is covered by this test without editing it. The layer each one adds
    // is the member name in snake_case, which is what the handler derives.
    let cases: Vec<(&str, String)> = TILE_ADDITIONAL_FEATURE_VALUES
        .iter()
        .map(|feature| {
            let layer = assets::additional_feature_layer(feature)
                .unwrap_or_else(|| panic!("case {feature:?}: a modeled feature must map to a layer"));
            (*feature, layer)
        })
        .collect();
    assert!(
        !cases.is_empty(),
        "case \"model\": TileAdditionalFeature must declare at least one value, got {:?}",
        TILE_ADDITIONAL_FEATURE_VALUES
    );
    for (feature, layer) in &cases {
        let response = emulator
            .get(&format!(
                "/v2/tiles/vector.basemap/4/8/5?additional-features={feature}"
            ))
            .await;
        assert_ok(feature, &response);
        let tile = Tile::decode(response.body().as_ref())
            .unwrap_or_else(|error| panic!("case {feature:?}: must decode, got {error}"));
        let names: BTreeSet<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
        assert!(
            names.contains(layer.as_str()),
            "case {feature:?}: expected the {layer} layer, got {names:?}"
        );
    }

    // All of them at once, up to the modeled `len ..4` maximum on the list.
    let query = cases
        .iter()
        .take(4)
        .map(|(feature, _)| format!("additional-features={feature}"))
        .collect::<Vec<_>>()
        .join("&");
    let response = emulator
        .get(&format!("/v2/tiles/vector.basemap/4/8/5?{query}"))
        .await;
    assert_ok("every feature at once", &response);
    let tile = Tile::decode(response.body().as_ref()).expect("a vector tile");
    let names: BTreeSet<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
    for (_, layer) in cases.iter().take(4) {
        assert!(
            names.contains(layer.as_str()),
            "case \"every feature at once\": expected the {layer} layer, got {names:?}"
        );
    }

    // A value the model does NOT declare is refused, and the refusal is the
    // modeled `ValidationException` rather than a tile missing a layer.
    for unmodeled in ["Contourlines", "Hillshade%20", "Weather", ""] {
        let response = emulator
            .get(&format!(
                "/v2/tiles/vector.basemap/4/8/5?additional-features={unmodeled}"
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {unmodeled:?}: expected 400, got {} / {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {unmodeled:?}: expected ValidationException, got {}",
            body_text(&response)
        );
    }
}

/// An address the catalog does not carry is a `ResourceNotFoundException`, and
/// a coordinate outside the tileset's own grid is a `ValidationException`.
#[tokio::test]
async fn tile_addresses_outside_the_catalog_are_refused() {
    let emulator = Emulator::new();
    // Row = (path, expected status, expected error code).
    let cases: [(&str, u16, &str); 9] = [
        ("/v2/tiles/no-such-tileset/3/4/5", 404, "ResourceNotFoundException"),
        ("/v2/tiles/vector.basemap/999/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/18/0/0", 400, "ValidationException"),
        ("/v2/tiles/raster.dem/15/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/abc/0/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/-1/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/8/0", 400, "ValidationException"),
        ("/v2/tiles/vector.basemap/3/0/8", 400, "ValidationException"),
        (
            "/v2/tiles/vector.basemap/3/0/0?additional-features=NoSuchFeature",
            400,
            "ValidationException",
        ),
    ];
    for (path, status, code) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {path:?}: expected {code}, got body {}",
            body_text(&response)
        );
    }

    // The zoom just inside each tileset's range is served, which is what makes
    // the refusals above about the range and not about the route.
    for (path, _, _) in [
        ("/v2/tiles/vector.basemap/17/0/0", 0, ""),
        ("/v2/tiles/raster.dem/14/0/0", 0, ""),
        ("/v2/tiles/raster.satellite/19/0/0", 0, ""),
    ] {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
    }
}

// ---------------------------------------------------------------------------
// GetSprites
// ---------------------------------------------------------------------------

/// The JSON index describes the icons of the PNG it is paired with: every icon
/// the catalog holds has an entry, every entry's rectangle is inside the sheet,
/// and the pixel at the centre of each rectangle is that icon's own colour.
#[tokio::test]
async fn the_sprite_index_describes_the_sheet_it_is_paired_with() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let icons: Vec<(String, [u8; 4])> = {
                let catalog = emulator.catalog();
                let catalog = catalog.lock();
                catalog
                    .sprites
                    .get(&(style, scheme, VariantKind::Default))
                    .expect("the catalog publishes a sheet per style and scheme")
                    .icons
                    .iter()
                    .map(|icon| (icon.name.clone(), icon.tint))
                    .collect()
            };

            // Rows: both pixel ratios the modeled file-name pattern admits.
            for (file, ratio) in [("sprites", 1_u32), ("sprites@2x", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let base = format!(
                    "/v2/styles/{}/{}/Default/sprites/{file}",
                    style.as_wire(),
                    scheme.as_wire()
                );
                let sheet = emulator.get(&format!("{base}.png")).await;
                let index = emulator.get(&format!("{base}.json")).await;
                assert_ok(&case, &sheet);
                assert_ok(&case, &index);
                assert_eq!(
                    header(&sheet, "content-type"),
                    delivered_content_type(&emulator),
                    "case {case:?}: a stored sprite file reports the object store's media type"
                );
                assert_eq!(
                    header(&index, "content-type"),
                    delivered_content_type(&emulator),
                    "case {case:?}: a stored sprite file reports the object store's media type"
                );
                assert_eq!(
                    header(&sheet, "etag"),
                    expected_etag(sheet.body()),
                    "case {case:?}: the ETag must hash the body served"
                );

                let (width, height, pixels) = decode_png(&case, sheet.body());
                let described: serde_json::Value = serde_json::from_slice(index.body())
                    .unwrap_or_else(|error| panic!("case {case:?}: index must be JSON: {error}"));
                let described = described
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: index must be a JSON object"));

                assert_eq!(
                    described.len(),
                    icons.len(),
                    "case {case:?}: the index must describe every icon in the catalog"
                );
                for (name, tint) in &icons {
                    let entry = described
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: index must describe {name}"));
                    assert_eq!(
                        entry["pixelRatio"].as_u64(),
                        Some(u64::from(ratio)),
                        "case {case:?}: {name} must report the requested pixel ratio"
                    );
                    let read = |key: &str| {
                        entry[key].as_u64().unwrap_or_else(|| {
                            panic!("case {case:?}: {name} must report a numeric {key}")
                        }) as u32
                    };
                    let (x, y, w, h) = (read("x"), read("y"), read("width"), read("height"));
                    assert!(
                        x + w <= width && y + h <= height,
                        "case {case:?}: {name} at {x},{y} {w}x{h} falls outside the {width}x{height} sheet"
                    );
                    let centre = png_pixel(width, &pixels, x + w / 2, y + h / 2);
                    assert_eq!(
                        centre, *tint,
                        "case {case:?}: the pixel at the centre of {name} must be its catalog colour"
                    );
                }
            }

            // `@2x` is the same sheet at twice the size.
            let base = format!(
                "/v2/styles/{}/{}/Default/sprites/sprites",
                style.as_wire(),
                scheme.as_wire()
            );
            let one = emulator.get(&format!("{base}.json")).await;
            let two = emulator.get(&format!("{base}@2x.json")).await;
            let one: serde_json::Value = serde_json::from_slice(one.body()).expect("JSON");
            let two: serde_json::Value = serde_json::from_slice(two.body()).expect("JSON");
            for (name, entry) in one.as_object().expect("a JSON object") {
                let case = format!("{}/{}/{name}@2x", style.as_wire(), scheme.as_wire());
                assert_eq!(
                    two[name]["width"].as_u64(),
                    entry["width"].as_u64().map(|value| value * 2),
                    "case {case:?}: an @2x icon is twice as wide"
                );
                assert_eq!(
                    two[name]["x"].as_u64(),
                    entry["x"].as_u64().map(|value| value * 2),
                    "case {case:?}: an @2x icon sits at twice the offset"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GetGlyphs
// ---------------------------------------------------------------------------

/// A glyph range is a decodable SDF `.pbf` for the stack and range requested,
/// with each glyph's bitmap the size its own metrics declare.
#[tokio::test]
async fn glyph_ranges_decode_for_every_stack_and_range() {
    let emulator = Emulator::new();
    let stacks = [
        "Amazon Ember Regular",
        "Amazon Ember Bold",
        "Amazon Ember Bold,Noto Sans Bold",
        "Not A Published Font",
    ];
    // Rows: the first range, a later one, and the last range of the BMP.
    let ranges = [(0_u32, 255_u32), (256, 511), (65280, 65535)];

    for stack in stacks {
        for (start, end) in ranges {
            let case = format!("{stack} {start}-{end}");
            let response = emulator
                .get(&format!(
                    "/v2/glyphs/{}/{start}-{end}.pbf",
                    stack.replace(' ', "%20")
                ))
                .await;
            assert_ok(&case, &response);
            assert_eq!(
                header(&response, "content-type"),
                delivered_content_type(&emulator),
                "case {case:?}: a stored glyph range reports the object store's media type"
            );
            assert_eq!(
                header(&response, "etag"),
                expected_etag(response.body()),
                "case {case:?}: the ETag must hash the body served"
            );

            let decoded =
                service_com_amazonaws_geomaps_mapsservice::state::glyph_pbf::Glyphs::decode(
                    response.body().as_ref(),
                )
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: must decode as a glyph range, got {error}")
                });
            assert_eq!(
                decoded.stacks.len(),
                1,
                "case {case:?}: a range carries exactly one font stack"
            );
            let fontstack = &decoded.stacks[0];
            assert_eq!(
                fontstack.name, stack,
                "case {case:?}: the stack must be the one requested"
            );
            assert_eq!(
                fontstack.range,
                format!("{start}-{end}"),
                "case {case:?}: the range must be the one requested"
            );
            assert!(
                !fontstack.glyphs.is_empty(),
                "case {case:?}: a range must carry glyphs"
            );
            for glyph in &fontstack.glyphs {
                assert!(
                    (start..=end).contains(&glyph.id),
                    "case {case:?}: glyph {} is outside the range", glyph.id
                );
                let bitmap = glyph
                    .bitmap
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {case:?}: glyph {} needs a bitmap", glyph.id));
                // The SDF format keeps a 3px buffer on every side.
                assert_eq!(
                    bitmap.len(),
                    (glyph.width as usize + 6) * (glyph.height as usize + 6),
                    "case {case:?}: glyph {}'s bitmap must match its metrics",
                    glyph.id
                );
                assert!(
                    glyph.advance > 0,
                    "case {case:?}: glyph {} must advance the cursor",
                    glyph.id
                );
            }
        }
    }

    // Two different published stacks measure differently, so the response is
    // the stack's own metrics and not one blob for every font.
    let regular = emulator
        .get("/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf")
        .await;
    let condensed = emulator
        .get("/v2/glyphs/Amazon%20Ember%20Condensed%20RC%20Regular/0-255.pbf")
        .await;
    assert_ne!(
        regular.body(),
        condensed.body(),
        "case \"two stacks\": a condensed face must not answer with the regular face's glyphs"
    );
}

/// A glyph range must be aligned to multiples of 256, which the member
/// documents and the modeled pattern cannot express.
#[tokio::test]
async fn misaligned_glyph_ranges_are_refused() {
    let emulator = Emulator::new();
    // Rows: unaligned start, wrong width, reversed, a start past the Unicode
    // range (whose aligned end would overflow a u32), and the aligned control.
    let cases: [(&str, u16); 8] = [
        ("/v2/glyphs/Amazon%20Ember%20Regular/1-256.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-127.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-511.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/512-255.pbf", 400),
        ("/v2/glyphs/Amazon%20Ember%20Regular/65536-65791.pbf", 400),
        (
            "/v2/glyphs/Amazon%20Ember%20Regular/4294967040-4294967295.pbf",
            400,
        ),
        ("/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf", 200),
        ("/v2/glyphs/Amazon%20Ember%20Regular/512-767.pbf", 200),
    ];
    for (path, status) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        if status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {path:?}: a misaligned range is a validation failure"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetStaticMap
// ---------------------------------------------------------------------------

/// Every documented way of framing a static map renders an image of the
/// requested size, in the catalog's media type, with the required pricing
/// bucket — and a repeat of the same request is byte-identical.
#[tokio::test]
async fn static_maps_render_every_documented_framing() {
    let emulator = Emulator::new();
    // Rows: each framing form, each file name (pixel ratio), and the styling
    // knobs that change the render.
    let cases: [(&str, u32, u32, u32); 7] = [
        ("/v2/static/map?width=600&height=400&center=-123.1,49.3&zoom=10", 600, 400, 1),
        ("/v2/static/map?width=256&height=256&center=0,0&radius=5000", 256, 256, 1),
        (
            "/v2/static/map?width=800&height=600&bounding-box=-123.17,49.26,-123.08,49.31",
            800,
            600,
            1,
        ),
        (
            "/v2/static/map?width=512&height=512&bounded-positions=97.17,78.03,99.04,27.17",
            512,
            512,
            1,
        ),
        ("/v2/static/map@2x?width=320&height=240&center=13.4,52.5&zoom=12", 320, 240, 2),
        (
            "/v2/static/map?width=640&height=480&center=2.35,48.85&zoom=11&style=Satellite&color-scheme=Dark&label-size=Large&scale-unit=Miles&pois=Disabled&padding=20",
            640,
            480,
            1,
        ),
        (
            "/v2/static/map?width=400&height=400&center=-0.12,51.5&zoom=9&geojson-overlay=%7B%22type%22%3A%22Point%22%2C%22coordinates%22%3A%5B-0.12%2C51.5%5D%7D",
            400,
            400,
            1,
        ),
    ];

    let (content_type, pricing_bucket) = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        (
            catalog.static_maps.content_type.clone(),
            catalog.pricing_bucket(catalog.static_maps.pricing_bucket.as_ref()),
        )
    };

    for (path, width, height, ratio) in cases {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
        assert_eq!(
            header(&response, "content-type"),
            content_type,
            "case {path:?}: the media type must come from the catalog"
        );
        assert_eq!(
            header(&response, "x-amz-geo-pricing-bucket"),
            pricing_bucket,
            "case {path:?}: the required pricing bucket must come from the catalog"
        );
        assert_eq!(
            header(&response, "etag"),
            expected_etag(response.body()),
            "case {path:?}: the ETag must hash the body served"
        );

        let (decoded_width, decoded_height, _) = decode_jpeg(path, response.body());
        assert_eq!(
            (decoded_width, decoded_height),
            (width * ratio, height * ratio),
            "case {path:?}: the image must be the requested size at the requested pixel ratio"
        );

        let repeat = emulator.get(path).await;
        assert_eq!(
            response.body(),
            repeat.body(),
            "case {path:?}: a repeated request must be byte-identical"
        );
    }
}

/// Framing changes the image: two different views of the world are two
/// different images, and the same view is one image.
#[tokio::test]
async fn static_map_framing_changes_the_image() {
    let emulator = Emulator::new();
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let cases = [
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=11",
        "/v2/static/map?width=256&height=256&center=13.4,52.5&zoom=10",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&radius=20000",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10&style=Satellite",
        "/v2/static/map?width=256&height=256&center=-123.1,49.3&zoom=10&color-scheme=Dark",
    ];
    for path in cases {
        let response = emulator.get(path).await;
        assert_ok(path, &response);
        assert!(
            seen.insert(response.body().to_vec()),
            "case {path:?}: this request answered with an image another request already served"
        );
    }
}

/// The framing rules the model's own member documentation states, and the WGS 84
/// bounds a position has to satisfy.
#[tokio::test]
async fn static_map_refuses_requests_it_cannot_frame() {
    let emulator = Emulator::new();
    // Row = (path, expected status). Every refusal is a ValidationException.
    let cases: [(&str, u16); 17] = [
        // No framing parameter at all.
        ("/v2/static/map?width=600&height=400", 400),
        // `Center` requires either Zoom or Radius.
        ("/v2/static/map?width=600&height=400&center=-123.1,49.3", 400),
        // `Zoom` cannot be used with `Radius`.
        (
            "/v2/static/map?width=600&height=400&center=-123.1,49.3&zoom=10&radius=500",
            400,
        ),
        // A bounding box frames the image itself.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31&zoom=10",
            400,
        ),
        (
            "/v2/static/map?width=600&height=400&bounded-positions=1,2,3,4&radius=500",
            400,
        ),
        // A bounding box is exactly two pairs.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31,-122.0,48.0",
            400,
        ),
        // Exactly one framing parameter frames the image.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-123.17,49.26,-123.08,49.31&center=0,0",
            400,
        ),
        // Padding is bounded by the documented min(width, height) / 4.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&padding=200",
            400,
        ),
        // An enum-valued knob outside the model's set is refused, not defaulted.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&label-size=Huge",
            400,
        ),
        // A path segment the modeled FileName pattern refuses is a validation
        // failure, not an unknown route.
        ("/v2/static/notmap?width=600&height=400&center=0,0&zoom=5", 400),
        // Longitude and latitude are bounded.
        (
            "/v2/static/map?width=600&height=400&bounding-box=-200.0,49.26,-123.08,49.31",
            400,
        ),
        ("/v2/static/map?width=600&height=400&center=10.0,95.0&zoom=5", 400),
        // `Radius` takes any value >= 1.
        ("/v2/static/map?width=600&height=400&center=0,0&radius=0", 400),
        // A GeoJSON overlay has to be GeoJSON.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&geojson-overlay=not-json",
            400,
        ),
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&geojson-overlay=%7B%22type%22%3A%22Nope%22%7D",
            400,
        ),
        // An overlay this emulator cannot draw is refused, not silently
        // dropped from the image.
        (
            "/v2/static/map?width=600&height=400&center=0,0&zoom=5&compact-overlay=line%3A1,2%7C3,4",
            400,
        ),
        // The control: the same request with a framing it can use.
        ("/v2/static/map?width=600&height=400&center=0,0&zoom=5", 200),
    ];
    for (path, status) in cases {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        if status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {path:?}: expected a ValidationException, got body {}",
                body_text(&response)
            );
            let body: serde_json::Value =
                serde_json::from_slice(response.body()).expect("a JSON error envelope");
            assert!(
                body.get("message").is_some(),
                "case {path:?}: a refusal must say what was wrong, got {body}"
            );
        }
    }

    // A position list past the cap the renderer plots is refused rather than
    // drawn, and one well inside it is served: the bound is a real edge, not a
    // blanket refusal of long lists.
    for (pairs, status) in [(200_usize, 200_u16), (1_300, 400)] {
        let positions = (0..pairs)
            .map(|index| format!("{},{}", index % 180, index % 80))
            .collect::<Vec<_>>()
            .join(",");
        let case = format!("bounded-positions with {pairs} pairs");
        let response = emulator
            .get(&format!(
                "/v2/static/map?width=256&height=256&bounded-positions={positions}"
            ))
            .await;
        assert_eq!(
            response.status(),
            status,
            "case {case:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }
}

/// `Width` and `Height` are required and bounded, so a request that omits or
/// exceeds them is refused before any rendering happens — with the modeled
/// error, not merely some 4xx.
#[tokio::test]
async fn static_map_requires_a_modeled_width_and_height() {
    let emulator = Emulator::new();
    for path in [
        "/v2/static/map?center=0,0&zoom=5",
        "/v2/static/map?width=600&center=0,0&zoom=5",
        "/v2/static/map?height=400&center=0,0&zoom=5",
        "/v2/static/map?width=600&height=63&center=0,0&zoom=5",
        "/v2/static/map?width=600&height=1401&center=0,0&zoom=5",
        "/v2/static/map?width=0&height=0&center=0,0&zoom=5",
    ] {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            400,
            "case {path:?}: expected 400, got {} with body {}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {path:?}: expected a ValidationException, got body {}",
            body_text(&response)
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog is where the answers come from
// ---------------------------------------------------------------------------

/// Every catalog key enum covers exactly the values the MODEL declares, so a
/// wire value the model admits is never one the catalog cannot key on (and
/// vice versa). This is the drift check for the `from_wire`/`as_wire` pairs in
/// `state.rs`, which are the only hand-written mapping left between a modelled
/// enum and a catalog key.
#[test]
fn catalog_key_enums_cover_the_modeled_value_sets() {
    let cases: [(&str, &[&str], Vec<String>); 3] = [
        (
            "MapStyle",
            MAP_STYLE_VALUES,
            MapStyleKind::ALL
                .iter()
                .map(|style| style.as_wire().to_string())
                .collect(),
        ),
        (
            "ColorScheme",
            COLOR_SCHEME_VALUES,
            ColorSchemeKind::ALL
                .iter()
                .map(|scheme| scheme.as_wire().to_string())
                .collect(),
        ),
        (
            "Variant",
            VARIANT_VALUES,
            VariantKind::ALL
                .iter()
                .map(|variant| variant.as_wire().to_string())
                .collect(),
        ),
    ];
    for (case, modeled, catalog) in cases {
        let modeled: BTreeSet<&str> = modeled.iter().copied().collect();
        let catalog: BTreeSet<&str> = catalog.iter().map(String::as_str).collect();
        assert_eq!(
            catalog, modeled,
            "case {case:?}: the catalog keys must be exactly the modeled values, got {catalog:?}"
        );
        // And each modelled value really parses back to a catalog key.
        for value in &modeled {
            let parsed = match case {
                "MapStyle" => MapStyleKind::from_wire(value).map(|k| k.as_wire()),
                "ColorScheme" => ColorSchemeKind::from_wire(value).map(|k| k.as_wire()),
                _ => VariantKind::from_wire(value).map(|k| k.as_wire()),
            };
            assert_eq!(
                parsed,
                Some(*value),
                "case {case:?}: {value:?} must round-trip through from_wire, got {parsed:?}"
            );
        }
    }
}

/// Changing the catalog changes the responses: the assets are computed from
/// engine state, so a tileset added to state is served, one removed is a 404,
/// and a record's reported values follow the record.
#[tokio::test]
async fn responses_follow_the_catalog_in_engine_state() {
    let emulator = Emulator::new();

    // A tileset the seed does not carry, added to state.
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        let mut added = catalog
            .tilesets
            .get("vector.basemap")
            .expect("the seeded basemap")
            .clone();
        added.id = "vector.custom".to_string();
        added.layers = vec!["custom_layer".to_string()];
        added.min_zoom = 2;
        added.max_zoom = 4;
        added.cache_control = "max-age=7".to_string();
        // No bucket of its own: it must report the catalog's default.
        added.pricing_bucket = None;
        catalog.tilesets.insert(added.id.clone(), added);
        catalog.default_pricing_bucket = "CustomBucket".to_string();
        // And one the seed does carry, removed.
        catalog.tilesets.remove("vector.traffic");
    }

    let response = emulator.get("/v2/tiles/vector.custom/3/1/1").await;
    assert_ok("vector.custom 3/1/1", &response);
    assert_eq!(
        header(&response, "cache-control"),
        "max-age=7",
        "case \"vector.custom\": the cache directive must follow the added record"
    );
    assert_eq!(
        header(&response, "x-amz-geo-pricing-bucket"),
        "CustomBucket",
        "case \"vector.custom\": a record with no bucket reports the catalog default"
    );
    let tile = Tile::decode(response.body().as_ref()).expect("a vector tile");
    let names: Vec<&str> = tile.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["custom_layer"],
        "case \"vector.custom\": the tile must carry the layers the record names"
    );

    for (path, status) in [
        ("/v2/tiles/vector.custom/1/0/0", 400),
        ("/v2/tiles/vector.custom/5/0/0", 400),
        ("/v2/tiles/vector.traffic/3/1/1", 404),
    ] {
        let response = emulator.get(path).await;
        assert_eq!(
            response.status(),
            status,
            "case {path:?}: expected {status}, got {} with body {}",
            response.status(),
            body_text(&response)
        );
    }

    // A style's stored palette decides the colours in its document.
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        let record = catalog
            .styles
            .get_mut(&(MapStyleKind::Standard, ColorSchemeKind::Light))
            .expect("the seeded Standard Light style");
        record.palette.water = Hsla::new(201, 98, 41);
        record.name = "Catalog Edited".to_string();
    }
    let response = emulator.get("/v2/styles/Standard/descriptor").await;
    assert_ok("edited style", &response);
    let document: serde_json::Value =
        serde_json::from_slice(response.body()).expect("a JSON style document");
    assert_eq!(
        document["name"].as_str(),
        Some("Catalog Edited"),
        "case \"edited style\": the document must report the stored name"
    );
    let water = document["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["id"] == "water_fill")
        .expect("a water layer")["paint"]["fill-color"]
        .as_str()
        .expect("a fill colour")
        .to_string();
    assert_eq!(
        water,
        Hsla::new(201, 98, 41).css(),
        "case \"edited style\": the paint colour must follow the stored palette"
    );
}

/// Two emulators seed the same catalog, so the same request against either one
/// answers with the same bytes: nothing about an asset depends on the process
/// it was rendered in.
#[tokio::test]
async fn two_emulators_serve_identical_assets() {
    let first = Emulator::new();
    let second = Emulator::new();
    for path in [
        "/v2/styles/Standard/descriptor?color-scheme=Dark",
        "/v2/tiles/vector.basemap/5/9/12",
        "/v2/tiles/raster.dem/5/9/12",
        "/v2/styles/Standard/Light/Default/sprites/sprites.png",
        "/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf",
        "/v2/static/map?width=256&height=256&center=0,0&zoom=4",
    ] {
        let left = first.get(path).await;
        let right = second.get(path).await;
        assert_ok(path, &left);
        assert_ok(path, &right);
        assert_eq!(
            left.body(),
            right.body(),
            "case {path:?}: two emulators must serve the same asset"
        );
        assert_eq!(
            header(&left, "etag"),
            header(&right, "etag"),
            "case {path:?}: and therefore the same ETag"
        );
    }
}

// ---------------------------------------------------------------------------
// Asset delivery: the STORED assets, and the object headers they carry
// ---------------------------------------------------------------------------

/// The server-side-encryption algorithm the catalog says the asset bucket
/// reports, read from state for the same reason as [`delivered_content_type`].
fn delivered_encryption(emulator: &Emulator) -> String {
    let catalog = emulator.catalog();
    let encryption = catalog.lock().asset_delivery.server_side_encryption.clone();
    encryption
}

/// Every address of a STORED asset: a glyph range (every font stack and range
/// the catalog answers for) and every sprite file (every style, colour scheme,
/// variant, file and pixel ratio).
fn stored_asset_paths() -> Vec<String> {
    let mut paths = vec![
        // Rows: a published stack, a second one, a fallback LIST, a stack the
        // catalog does not publish; the first range, a middle one and the last
        // range of the BMP.
        "/v2/glyphs/Amazon%20Ember%20Regular/0-255.pbf".to_string(),
        "/v2/glyphs/Amazon%20Ember%20Bold/256-511.pbf".to_string(),
        "/v2/glyphs/Amazon%20Ember%20Bold,Noto%20Sans%20Bold/3072-3327.pbf".to_string(),
        "/v2/glyphs/Not%20A%20Published%20Font/65280-65535.pbf".to_string(),
    ];
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for file in [
                "sprites.png",
                "sprites@2x.png",
                "sprites.json",
                "sprites@2x.json",
            ] {
                paths.push(format!(
                    "/v2/styles/{}/{}/{}/sprites/{file}",
                    style.as_wire(),
                    scheme.as_wire(),
                    VariantKind::Default.as_wire()
                ));
            }
        }
    }
    paths
}

/// A stored asset is served out of the object store, so every one of them —
/// every glyph range and every sprite file — reports the object's own media type
/// and carries the delivery headers of the object version it was read from.
///
/// The version id identifies the stored content: it is the same on a repeat read
/// of one asset, the same for two assets whose bytes are the same, and different
/// as soon as the bytes are.
#[tokio::test]
async fn stored_assets_carry_the_object_store_delivery_headers() {
    let emulator = Emulator::new();
    let delivered = delivered_content_type(&emulator);
    let encryption = delivered_encryption(&emulator);
    // body digest -> the version id served for it, so the one id per content
    // rule is checked across every asset rather than per request.
    let mut ids_by_content: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    for path in stored_asset_paths() {
        let response = emulator.get(&path).await;
        assert_ok(&path, &response);
        assert_eq!(
            header(&response, "content-type"),
            delivered,
            "case {path:?}: a stored asset reports the object store's media type, got {:?}",
            header(&response, "content-type")
        );
        assert_eq!(
            header(&response, "x-amz-server-side-encryption"),
            encryption,
            "case {path:?}: a stored asset reports how the bucket encrypts it, got {:?}",
            header(&response, "x-amz-server-side-encryption")
        );
        let version_id = header(&response, "x-amz-version-id");
        assert!(
            !version_id.is_empty(),
            "case {path:?}: a stored asset must report the object version it came from"
        );
        assert_eq!(
            header(&response, "etag"),
            expected_etag(response.body()),
            "case {path:?}: the ETag must hash the body served"
        );

        // A repeat read of one asset is the same object version.
        let repeat = emulator.get(&path).await;
        assert_eq!(
            header(&repeat, "x-amz-version-id"),
            version_id,
            "case {path:?}: a repeat read must report the same object version"
        );

        let content = expected_etag(response.body());
        match ids_by_content.get(&content) {
            Some(seen) => assert_eq!(
                seen, &version_id,
                "case {path:?}: byte-identical assets must report one version id, got {version_id:?}"
            ),
            None => {
                assert!(
                    !ids_by_content.values().any(|seen| seen == &version_id),
                    "case {path:?}: a different asset must report a different version id, got {version_id:?}"
                );
                ids_by_content.insert(content, version_id);
            }
        }
    }
}

/// The negative half of the same rule: a body this service COMPUTES from the
/// request is not an object of the asset bucket, so it keeps its own media type
/// and carries neither object header.
#[tokio::test]
async fn computed_bodies_carry_no_object_headers() {
    let emulator = Emulator::new();
    // Rows: every operation whose body is computed from the request, with the
    // media type its own catalog record declares.
    let computed: Vec<(String, String)> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        let mut rows = vec![
            (
                "/v2/styles/Standard/descriptor".to_string(),
                catalog
                    .styles
                    .get(&(MapStyleKind::Standard, ColorSchemeKind::Light))
                    .expect("the catalog publishes the Standard Light style")
                    .content_type
                    .clone(),
            ),
            (
                "/v2/static/map?width=256&height=256&center=0,0&zoom=4".to_string(),
                catalog.static_maps.content_type.clone(),
            ),
        ];
        for (id, record) in &catalog.tilesets {
            rows.push((
                format!("/v2/tiles/{id}/4/5/6"),
                record.content_type.clone(),
            ));
        }
        rows
    };

    for (path, expected_type) in computed {
        let response = emulator.get(&path).await;
        assert_ok(&path, &response);
        assert_eq!(
            header(&response, "content-type"),
            expected_type,
            "case {path:?}: a computed body keeps its own media type, got {:?}",
            header(&response, "content-type")
        );
        for object_header in ["x-amz-version-id", "x-amz-server-side-encryption"] {
            assert!(
                response.headers().get(object_header).is_none(),
                "case {path:?}: a computed body carries no {object_header}, got {:?}",
                header(&response, object_header)
            );
        }
    }
}

/// A glyph range is derived from the request: no two (stack, range) addresses
/// answer with the same bytes, so none of them is a constant placeholder.
#[tokio::test]
async fn glyph_ranges_differ_per_stack_and_range() {
    let emulator = Emulator::new();
    // Rows: the same range across stacks with different metrics, and the same
    // stack across ranges.
    let stacks = [
        "Amazon Ember Regular",
        "Amazon Ember Bold",
        "Amazon Ember Condensed RC Regular",
        "Noto Sans Regular",
    ];
    let ranges = ["0-255.pbf", "256-511.pbf", "3072-3327.pbf", "65280-65535.pbf"];
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for stack in stacks {
        for range in ranges {
            let case = format!("{stack} {range}");
            let path = format!("/v2/glyphs/{}/{range}", stack.replace(' ', "%20"));
            let response = emulator.get(&path).await;
            assert_ok(&case, &response);
            let digest = expected_etag(response.body());
            if let Some(other) = seen.get(&digest) {
                panic!("case {case:?}: must not serve the same glyph bytes as {other:?}");
            }
            seen.insert(digest, case);
        }
    }
}

/// The sprite index is the published iconography of the sheet it is paired
/// with: every icon family the catalog carries is in it, under the name a style
/// layer references, with the five members a client reads a rectangle from.
#[tokio::test]
async fn the_sprite_index_lists_the_published_iconography() {
    let emulator = Emulator::new();
    // Rows: one icon of every family the catalog holds, with the box its family
    // occupies at pixelRatio 1 (the constants in `state.rs`). A dot's rank does
    // not change its box; whether the dot is an open ring does.
    let families: [(&str, u32, u32); 8] = [
        ("accommodation", PLACE_ICON_BOX, PLACE_ICON_BOX),
        ("truck-weigh-station", PLACE_ICON_BOX, PLACE_ICON_BOX),
        ("road_closure", INCIDENT_ICON_BOX, INCIDENT_ICON_BOX),
        ("citydot", CITY_DOT_BOX, CITY_DOT_BOX),
        ("citydot_lowrank", CITY_DOT_BOX, CITY_DOT_BOX),
        ("citydot-open", CITY_DOT_OPEN_BOX, CITY_DOT_OPEN_BOX),
        ("marker_transit", MARKER_BOX, MARKER_BOX),
        (
            "shield-hgv-weight_restriction",
            RESTRICTION_SHIELD_BOX,
            RESTRICTION_SHIELD_BOX,
        ),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let response = emulator
                    .get(&format!(
                        "/v2/styles/{}/{}/Default/sprites/{file}",
                        style.as_wire(),
                        scheme.as_wire()
                    ))
                    .await;
                assert_ok(&case, &response);
                let index: serde_json::Value = serde_json::from_slice(response.body())
                    .unwrap_or_else(|error| panic!("case {case:?}: index must be JSON: {error}"));
                let index = index
                    .as_object()
                    .unwrap_or_else(|| panic!("case {case:?}: index must be a JSON object"));

                for (name, width, height) in families {
                    let entry = index.get(name).unwrap_or_else(|| {
                        panic!("case {case:?}: the index must describe {name}")
                    });
                    let entry = entry.as_object().unwrap_or_else(|| {
                        panic!("case {case:?}: {name} must be an object, got {entry}")
                    });
                    let members: BTreeSet<&str> =
                        entry.keys().map(String::as_str).collect();
                    assert_eq!(
                        members,
                        ["height", "pixelRatio", "width", "x", "y"]
                            .into_iter()
                            .collect::<BTreeSet<&str>>(),
                        "case {case:?}: {name} must carry exactly the index members, got {members:?}"
                    );
                    assert_eq!(
                        entry["width"].as_u64(),
                        Some(u64::from(width * ratio)),
                        "case {case:?}: {name} is {width} style pixels wide at ratio {ratio}"
                    );
                    assert_eq!(
                        entry["height"].as_u64(),
                        Some(u64::from(height * ratio)),
                        "case {case:?}: {name} is {height} style pixels tall at ratio {ratio}"
                    );
                    assert_eq!(
                        entry["pixelRatio"].as_u64(),
                        Some(u64::from(ratio)),
                        "case {case:?}: {name} must report the requested pixel ratio"
                    );
                }
            }
        }
    }
}

/// A route shield is as wide as the longest route number its own name says it
/// has to hold: a width word (`-long`, `-xlong`, `-wide`) or a trailing
/// digit-capacity suffix, over the catalog's two-digit base.
#[test]
fn route_shield_boxes_follow_the_digit_capacity_in_the_name() {
    let two = SHIELD_BASE_WIDTH;
    let three = SHIELD_BASE_WIDTH + SHIELD_DIGIT_WIDTH;
    let four = SHIELD_BASE_WIDTH + 2 * SHIELD_DIGIT_WIDTH;
    // Rows: no suffix, each width word, each numeric capacity suffix — and
    // names the catalog does not carry, because this is the rule and not a
    // lookup of the published list.
    let cases: [(&str, u32); 12] = [
        ("shield-ZZ:national", two),
        ("shield-Emirates routes", two),
        ("shield-US-Hwy-1-2", two),
        ("shield-US-Hwy-3", three),
        ("shield-autostrade-2-3", three),
        ("shield-MX-Federal-3-4", four),
        ("shield-rectangle-white-long", three),
        ("shield-rectangle-white-xlong", four),
        ("shield-IN-National-wide", three),
        // Not in the published catalog: the rule still answers.
        ("shield-ZZ", two),
        ("shield-ZZ-long", three),
        ("shield-ZZ-9", four),
    ];
    for (name, expected) in cases {
        let actual = route_shield_width(name);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected a {expected}px box, got {actual:?}"
        );
    }
}

/// The published catalog really is laid out by its footprints: the index
/// reports each shield at its own published plate — the box the digit-capacity
/// rule produces where the catalog gives the shield no plate of its own — and
/// every rectangle it reports lies inside the sheet it is paired with.
#[tokio::test]
async fn the_sheet_holds_every_icon_the_index_points_at() {
    let emulator = Emulator::new();
    let names: Vec<String> = {
        let catalog = emulator.catalog();
        let catalog = catalog.lock();
        catalog
            .sprites
            .get(&(
                MapStyleKind::Standard,
                ColorSchemeKind::Light,
                VariantKind::Default,
            ))
            .expect("the catalog publishes the Standard Light sheet")
            .icons
            .iter()
            .map(|icon| icon.name.clone())
            .collect()
    };
    assert!(
        names.len() > 100,
        "case \"catalog\": a published sheet carries the whole iconography, got {} icons",
        names.len()
    );

    let base = "/v2/styles/Standard/Light/Default/sprites/sprites";
    let sheet = emulator.get(&format!("{base}.png")).await;
    let index = emulator.get(&format!("{base}.json")).await;
    assert_ok("sheet", &sheet);
    assert_ok("index", &index);
    let (width, height, _) = decode_png("sheet", sheet.body());
    let described: serde_json::Value =
        serde_json::from_slice(index.body()).expect("the index must be JSON");
    let described = described.as_object().expect("the index must be an object");

    for name in &names {
        let entry = described
            .get(name)
            .unwrap_or_else(|| panic!("case {name:?}: the index must describe it"));
        let read = |key: &str| {
            entry[key]
                .as_u64()
                .unwrap_or_else(|| panic!("case {name:?}: must report a numeric {key}")) as u32
        };
        let (x, y, w, h) = (read("x"), read("y"), read("width"), read("height"));
        assert!(
            x + w <= width && y + h <= height,
            "case {name:?}: {x},{y} {w}x{h} falls outside the {width}x{height} sheet"
        );
        if let Some(rest) = name.strip_prefix("shield-") {
            if !rest.starts_with("hgv-") {
                assert_eq!(
                    (w, h),
                    route_shield_box(name),
                    "case {name:?}: a route shield is reported at its own published plate, \
                     got {w}x{h}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GetSprites: the published index of a sheet
// ---------------------------------------------------------------------------

/// The index `GetSprites` serves for one sheet, as a name -> entry map.
async fn sprite_index(
    emulator: &Emulator,
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    file: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let path = format!(
        "/v2/styles/{}/{}/Default/sprites/{file}",
        style.as_wire(),
        scheme.as_wire()
    );
    let response = emulator.get(&path).await;
    assert_ok(&path, &response);
    serde_json::from_slice::<serde_json::Value>(response.body())
        .unwrap_or_else(|error| panic!("case {path:?}: the index must be JSON: {error}"))
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("case {path:?}: the index must be a JSON object"))
}

/// One index entry's `x`, `y`, `width` and `height`.
fn entry_rectangle(case: &str, name: &str, entry: &serde_json::Value) -> (u32, u32, u32, u32) {
    let read = |key: &str| {
        entry[key]
            .as_u64()
            .unwrap_or_else(|| panic!("case {case:?}: {name} must report a numeric {key}, got {entry}"))
            as u32
    };
    (read("x"), read("y"), read("width"), read("height"))
}

/// The index is the sheet's STORED geometry, not an atlas computed per
/// request: every entry reports the rectangle the catalog record holds for that
/// icon, scaled by the requested pixel ratio, and the index names exactly the
/// icons that record carries.
#[tokio::test]
async fn the_sprite_index_reports_the_sheets_stored_geometry() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let stored: Vec<(String, u32, u32, u32, u32)> = {
                let catalog = emulator.catalog();
                let catalog = catalog.lock();
                catalog
                    .sprites
                    .get(&(style, scheme, VariantKind::Default))
                    .expect("the catalog publishes a sheet per style and scheme")
                    .icons
                    .iter()
                    .map(|icon| {
                        (
                            icon.name.clone(),
                            icon.x,
                            icon.y,
                            icon.width,
                            icon.height,
                        )
                    })
                    .collect()
            };

            // Rows: both pixel ratios the modeled file-name pattern admits.
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let index = sprite_index(&emulator, style, scheme, file).await;
                assert_eq!(
                    index.len(),
                    stored.len(),
                    "case {case:?}: the index must name exactly the stored sheet's icons, \
                     got {} entries for {} icons",
                    index.len(),
                    stored.len()
                );
                for (name, x, y, width, height) in &stored {
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    let actual = entry_rectangle(&case, name, entry);
                    let expected = (x * ratio, y * ratio, width * ratio, height * ratio);
                    assert_eq!(
                        actual, expected,
                        "case {case:?}: {name} is stored at {expected:?}, index reports {actual:?}"
                    );
                }
            }
        }
    }
}

/// A published icon keeps its OWN footprint: the sheet is drawn art, so an
/// icon is as wide and as tall as its own artwork rather than being forced into
/// one square cell per family.
#[tokio::test]
async fn a_published_icon_keeps_its_own_footprint() {
    let emulator = Emulator::new();
    // Rows: one icon of every footprint the published index reports, including
    // the icons that differ from their family's square, the two city-dot sizes,
    // the whole map-pin family and the two glyphs the index reports larger than
    // their family. Source: the `sprites.json` index the Maps asset endpoint
    // publishes for a sheet.
    let footprints: [(&str, u32, u32); 18] = [
        ("accommodation", 21, 21),
        ("airport", 21, 21),
        ("bicycle", 22, 20),
        ("bus", 20, 20),
        ("entrance", 20, 20),
        ("citydot", 5, 5),
        ("citydot_lowrank", 5, 5),
        ("citydot-open", 6, 6),
        ("citydot-open_lowrank", 6, 6),
        ("accident", 32, 32),
        ("oneway", 50, 29),
        ("other", 32, 32),
        ("marker", 21, 21),
        ("marker_arts_entertainment", 21, 21),
        ("marker_food_drink", 21, 21),
        ("marker_recreation", 21, 21),
        ("marker_stores_services", 21, 21),
        ("marker_transit", 21, 21),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let index = sprite_index(&emulator, style, scheme, file).await;
                for (name, width, height) in footprints {
                    let case = format!("{}/{}/{file}/{name}", style.as_wire(), scheme.as_wire());
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    let (_, _, actual_width, actual_height) = entry_rectangle(&case, name, entry);
                    assert_eq!(
                        (actual_width, actual_height),
                        (width * ratio, height * ratio),
                        "case {case:?}: expected a {}x{} box, got {actual_width}x{actual_height}",
                        width * ratio,
                        height * ratio
                    );
                }
            }
        }
    }
}

/// The sheet is ONE packing of its whole iconography: the entries abut along a
/// row in the order the index lists them, a row is as tall as its tallest icon,
/// and a row ends only where the next icon no longer fits the sheet's width.
/// Nothing is laid out per family, so no family starts a row of its own.
#[tokio::test]
async fn the_iconography_abuts_in_listing_order_and_wraps_at_the_sheet_width() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            // Rows: both pixel ratios, so the packing holds at both scales.
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let index = sprite_index(&emulator, style, scheme, file).await;
                let sheet_width = SPRITE_SHEET_WIDTH * ratio;

                let mut boxes: Vec<(String, u32, u32, u32, u32)> = index
                    .iter()
                    .map(|(name, entry)| {
                        let (x, y, width, height) = entry_rectangle(&case, name, entry);
                        (name.clone(), x, y, width, height)
                    })
                    .collect();
                boxes.sort_by_key(|(_, x, y, _, _)| (*y, *x));

                let mut cursor = 0;
                let mut row_y = 0;
                let mut row_height = 0;
                for (name, x, y, width, height) in &boxes {
                    assert!(
                        x + width <= sheet_width,
                        "case {case:?}: {name} ends at {}, past the {sheet_width}-pixel sheet",
                        x + width
                    );
                    if *y != row_y {
                        assert_eq!(
                            *y,
                            row_y + row_height,
                            "case {case:?}: {name}'s row must follow the row above it at {}, \
                             got {y}",
                            row_y + row_height
                        );
                        assert!(
                            cursor + width > sheet_width,
                            "case {case:?}: {name} still fits the row above at {cursor}, \
                             so it must not start a new row"
                        );
                        row_y = *y;
                        row_height = 0;
                        cursor = 0;
                    }
                    assert_eq!(
                        *x, cursor,
                        "case {case:?}: {name} must abut the icon before it at {cursor}, got {x}"
                    );
                    cursor += width;
                    row_height = row_height.max(*height);
                }
                assert!(
                    row_y > 0,
                    "case {case:?}: the iconography is wider than the sheet, so it must wrap, \
                     got every icon on one row"
                );
            }
        }
    }
}

/// The packing runs the whole iconography through ONE listing order, so an icon
/// the index lists later never sits earlier on the sheet: the road shields are
/// packed between the symbol icons whose names surround them, and the row wraps
/// in the middle of them rather than at a family boundary.
#[tokio::test]
async fn the_packing_follows_the_published_listing_order() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let index = sprite_index(&emulator, style, scheme, "sprites.json").await;
            let names: Vec<String> = index.keys().cloned().collect();
            // A family — every name one root name is a prefix of, such as the
            // four city dots or a shield and the plate that widens it — is
            // listed together, widest plate first, so its members are not part
            // of this walk by name.
            let mut listed: Vec<(String, u32, u32)> = index
                .iter()
                .filter(|(name, _)| !shares_a_family(name, &names))
                .map(|(name, entry)| {
                    let (x, y, _, _) = entry_rectangle(&case, name, entry);
                    (name.clone(), y, x)
                })
                .collect();
            listed.sort_by(|left, right| left.0.cmp(&right.0));
            for pair in listed.windows(2) {
                let (before, after) = (&pair[0], &pair[1]);
                assert!(
                    (before.1, before.2) < (after.1, after.2),
                    "case {case:?}: {} is listed before {} and must not sit after it, got \
                     {},{} then {},{}",
                    before.0,
                    after.0,
                    before.2,
                    before.1,
                    after.2,
                    after.1
                );
            }

            let shield_rows: BTreeSet<u32> = listed
                .iter()
                .filter(|(name, _, _)| name.starts_with("shield-"))
                .map(|(_, y, _)| *y)
                .collect();
            assert!(
                shield_rows.contains(&0),
                "case {case:?}: the shields continue the sheet's first row, got rows \
                 {shield_rows:?}"
            );
            assert!(
                shield_rows.len() > 1,
                "case {case:?}: the shields are wider than one row, so they must wrap, got rows \
                 {shield_rows:?}"
            );
        }
    }
}

/// The index describes a sheet a client can cut icons out of: every rectangle
/// it reports is disjoint from every other, so no icon is drawn over its
/// neighbour.
#[tokio::test]
async fn no_two_published_icons_share_a_pixel() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            // Rows: both pixel ratios, because scaling the sheet must not make
            // two rectangles meet.
            for file in ["sprites.json", "sprites@2x.json"] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let index = sprite_index(&emulator, style, scheme, file).await;
                let boxes: Vec<(String, u32, u32, u32, u32)> = index
                    .iter()
                    .map(|(name, entry)| {
                        let (x, y, width, height) = entry_rectangle(&case, name, entry);
                        (name.clone(), x, y, width, height)
                    })
                    .collect();
                for (index_of_left, left) in boxes.iter().enumerate() {
                    for right in &boxes[index_of_left + 1..] {
                        let overlaps = left.1 < right.1 + right.3
                            && right.1 < left.1 + left.3
                            && left.2 < right.2 + right.4
                            && right.2 < left.2 + left.4;
                        assert!(
                            !overlaps,
                            "case {case:?}: {} at {},{} {}x{} overlaps {} at {},{} {}x{}",
                            left.0,
                            left.1,
                            left.2,
                            left.3,
                            left.4,
                            right.0,
                            right.1,
                            right.2,
                            right.3,
                            right.4
                        );
                    }
                }
            }
        }
    }
}

/// A road shield is published under the name of the NETWORK it signs, written
/// as that network's own namespaced identifier; a few of those networks are
/// published under a short second name as well, and a bare country code is
/// never one of them.
#[tokio::test]
async fn a_road_shield_is_published_under_its_network_name() {
    let emulator = Emulator::new();
    // Rows: network-namespaced shield names the published index carries, one
    // per naming shape it uses — a route class, a numbered class, a `-long`
    // number variant, a lower-case country code and a local network word.
    // Source: the published `sprites.json` index.
    let networked: [&str; 10] = [
        "shield-AE:E-route",
        "shield-AU:NH",
        "shield-CN:expressway",
        "shield-CN:expressway-long",
        "shield-DE:BAB",
        "shield-JP:prefectural",
        "shield-NL:binnenstedelijke_ring",
        "shield-PH:E",
        "shield-ch:national",
        "shield-md:national",
    ];
    // A network whose shield the index carries under a SECOND, short name as
    // well: the route it signs, or the local word for the network. Source: the
    // published `sprites.json` index.
    let aliases: [&str; 3] = ["shield-AE", "shield-BAB", "shield-NL-ring"];
    // A bare country code is not one of those short names: a style's
    // icon-image lookup resolves the network's own name.
    let unpublished: [&str; 3] = ["shield-AU", "shield-DE", "shield-JP"];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for file in ["sprites.json", "sprites@2x.json"] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let index = sprite_index(&emulator, style, scheme, file).await;
                for name in networked.into_iter().chain(aliases) {
                    assert!(
                        index.contains_key(name),
                        "case {case:?}: the index must carry {name}, \
                         got {} entries without it",
                        index.len()
                    );
                }
                for name in unpublished {
                    assert!(
                        !index.contains_key(name),
                        "case {case:?}: {name} is not a published icon name, got {:?}",
                        index.get(name)
                    );
                }
            }
        }
    }
}

/// The index belongs to the sheet it is served for, not to the service: the
/// sheets of this catalog do not all carry the same shield names, so the one
/// name that differs between them differs in the document too.
#[tokio::test]
async fn each_sheet_carries_the_shield_names_it_publishes() {
    let emulator = Emulator::new();
    let by_route = "shield-CA-TransCanada";
    let by_network = "shield-CA:transcanada";
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let index = sprite_index(&emulator, style, scheme, "sprites.json").await;
            // The Standard/Light sheet publishes the Trans-Canada shield under
            // the route it signs; every other sheet publishes it under its
            // network, like the rest of their networked shields.
            let names_the_route =
                style == MapStyleKind::Standard && scheme == ColorSchemeKind::Light;
            assert_eq!(
                index.contains_key(by_route),
                names_the_route,
                "case {case:?}: {by_route} must be carried only by the sheet that publishes it"
            );
            assert_eq!(
                index.contains_key(by_network),
                !names_the_route,
                "case {case:?}: {by_network} must be carried by every other sheet"
            );
        }
    }
}

/// The published offsets: the index places each icon of the first row where the
/// published sheet holds it, which follows from each icon's own footprint and
/// the order the index lists the row in.
#[tokio::test]
async fn the_first_row_reports_the_published_offsets() {
    let emulator = Emulator::new();
    // Rows: the `x` the published index reports for icons spread across the
    // first row — before the city dots, the four dots themselves (the open ring
    // is listed first), the run after them, and the run past the map pins and
    // the two glyphs the index reports larger than their family, whose own
    // footprints every later offset of the row depends on. Source: the
    // published `sprites.json` index.
    let offsets: [(&str, u32); 17] = [
        ("book", 117),
        ("bus", 138),
        ("citydot-open", 305),
        ("citydot-open_lowrank", 311),
        ("citydot", 317),
        ("citydot_lowrank", 322),
        ("cocktail", 327),
        ("coffee", 348),
        ("congestion", 369),
        ("education", 475),
        ("entrance", 496),
        ("ferry", 558),
        ("marker_arts_entertainment", 831),
        ("metro", 936),
        ("oneway", 1062),
        ("other", 1112),
        ("place-of-worship", 1228),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let index = sprite_index(&emulator, style, scheme, file).await;
                for (name, offset) in offsets {
                    let case = format!("{}/{}/{file}/{name}", style.as_wire(), scheme.as_wire());
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    let (x, y, _, _) = entry_rectangle(&case, name, entry);
                    assert_eq!(
                        (x, y),
                        (offset * ratio, 0),
                        "case {case:?}: expected the published offset {},0, got {x},{y}",
                        offset * ratio
                    );
                }
            }
        }
    }
}

/// Whether another icon of the sheet is in `name`'s family: a family is every
/// name one root name is a prefix of, which is how the published index lists a
/// shield together with the plate that widens it.
///
/// Derived here from the names the index reports rather than by calling the
/// catalog's own helper, deliberately: a test that asked the implementation
/// which icons are a family would assert the packing against itself.
fn shares_a_family(name: &str, names: &[String]) -> bool {
    names
        .iter()
        .any(|other| other != name && (name.starts_with(other.as_str()) || other.starts_with(name)))
}

/// The Trans-Canada shield's name on the sheet addressed by `style`/`scheme`:
/// one sheet publishes it after the route it signs, the others after the
/// network it belongs to.
fn transcanada_name(index: &serde_json::Map<String, serde_json::Value>) -> &'static str {
    if index.contains_key("shield-CA-TransCanada") {
        "shield-CA-TransCanada"
    } else {
        "shield-CA:transcanada"
    }
}

/// A published route shield keeps its OWN plate: a published sheet's shields are
/// drawn art, so a shield is as wide and as tall as the plate it signs with
/// rather than being reported at one box for the whole family.
///
/// Every row is a plate the published index reports; the box a shield with no
/// published plate of its own falls back to is the rule
/// `route_shield_boxes_follow_the_digit_capacity_in_the_name` pins, over names
/// no sheet carries.
#[tokio::test]
async fn a_published_route_shield_keeps_its_own_plate() {
    let emulator = Emulator::new();
    // Rows: one shield of every plate shape the published index reports — a
    // tall plate and the same plate under its network name, two plates narrower
    // than the catalog's base box, a square plate, a wide plate and the same
    // plate under its network name, the widest plate and the plate it widens,
    // and two plates of one box under two names. Source: the `sprites.json`
    // index the Maps asset endpoint publishes for a sheet.
    let plates: [(&str, u32, u32); 13] = [
        ("shield-AE", 21, 30),
        ("shield-AE:E-route", 21, 30),
        ("shield-AU:MR", 19, 20),
        ("shield-AU:NH", 19, 20),
        ("shield-AU:T", 21, 22),
        ("shield-BAB", 36, 20),
        ("shield-DE:BAB", 36, 20),
        ("shield-BR", 22, 27),
        ("shield-CN:expressway", 30, 28),
        ("shield-CN:expressway-long", 43, 28),
        ("shield-CR:CR", 23, 26),
        ("shield-CR:national", 23, 26),
        ("shield-hgv-speed_limit", RESTRICTION_SHIELD_BOX, RESTRICTION_SHIELD_BOX),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            // Rows: both pixel ratios the modeled file-name pattern admits, so
            // a plate scales with the sheet rather than being re-derived.
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let index = sprite_index(&emulator, style, scheme, file).await;
                let transcanada = transcanada_name(&index);
                for (name, width, height) in plates
                    .into_iter()
                    .chain([(transcanada, 24, 24)])
                {
                    let case = format!("{}/{}/{file}/{name}", style.as_wire(), scheme.as_wire());
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    let (_, _, actual_width, actual_height) = entry_rectangle(&case, name, entry);
                    assert_eq!(
                        (actual_width, actual_height),
                        (width * ratio, height * ratio),
                        "case {case:?}: expected a {}x{} plate, got {actual_width}x{actual_height}",
                        width * ratio,
                        height * ratio
                    );
                }
            }
        }
    }
}

/// The published offsets of the shields: a shield sits where the plates listed
/// before it on the row leave it, so its `x` is the previous entry's `x` plus
/// that entry's OWN plate width rather than a constant cell pitch.
#[tokio::test]
async fn the_published_shield_offsets_follow_the_plates_before_them() {
    let emulator = Emulator::new();
    // Rows: the `x` the published index reports for the run of shields that
    // opens the shield iconography — across plates of seven different widths,
    // including the two plates reported narrower than the base box and the two
    // widest plates of the run, every later offset of which depends on the
    // footprints before it. Source: the published `sprites.json` index.
    let offsets: [(&str, u32); 13] = [
        ("shield-AE", 1386),
        ("shield-AE:E-route", 1407),
        ("shield-AU:MR", 1428),
        ("shield-AU:NH", 1447),
        ("shield-AU:S", 1466),
        ("shield-AU:T", 1486),
        ("shield-BAB", 1507),
        ("shield-BR", 1543),
        ("shield-CN:expressway-long", 1589),
        ("shield-CN:expressway", 1632),
        ("shield-CR:CR", 1662),
        ("shield-CR:national", 1685),
        ("shield-DE:BAB", 1708),
    ];

    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let index = sprite_index(&emulator, style, scheme, file).await;
                // The Trans-Canada plate sits between them, under whichever of
                // its two names this sheet publishes.
                let transcanada = transcanada_name(&index);
                for (name, offset) in offsets.into_iter().chain([(transcanada, 1565)]) {
                    let case = format!("{}/{}/{file}/{name}", style.as_wire(), scheme.as_wire());
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    let (x, y, _, _) = entry_rectangle(&case, name, entry);
                    assert_eq!(
                        (x, y),
                        (offset * ratio, 0),
                        "case {case:?}: expected the published offset {},0, got {x},{y}",
                        offset * ratio
                    );
                }
            }
        }
    }
}

/// The index lists a FAMILY together, widest plate first: every pair of names
/// one of which is a prefix of the other sits in that order on the sheet, which
/// is why the open city ring is listed before the filled dot and a `-long`
/// number plate before the plate it widens.
#[tokio::test]
async fn the_index_lists_a_familys_widest_plate_first() {
    let emulator = Emulator::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            let case = format!("{}/{}", style.as_wire(), scheme.as_wire());
            let index = sprite_index(&emulator, style, scheme, "sprites.json").await;
            let boxes: Vec<(String, u32, u32, u32)> = index
                .iter()
                .map(|(name, entry)| {
                    let (x, y, width, _) = entry_rectangle(&case, name, entry);
                    (name.clone(), y, x, width)
                })
                .collect();
            // Every family the sheet carries, found from the names themselves
            // rather than from a list: the pairs where one name is a prefix of
            // the other.
            let mut pairs = 0_usize;
            for left in &boxes {
                for right in &boxes {
                    if left.0 == right.0 || !right.0.starts_with(&left.0) {
                        continue;
                    }
                    pairs += 1;
                    // The wider plate is listed first; where the two plates are
                    // the same width the shorter name is.
                    let (first, second) = if left.3 >= right.3 {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    assert!(
                        (first.1, first.2) < (second.1, second.2),
                        "case {case:?}: {} is a {}px plate and {} a {}px one, so {} is listed \
                         first, got {},{} then {},{}",
                        first.0,
                        first.3,
                        second.0,
                        second.3,
                        first.0,
                        first.2,
                        first.1,
                        second.2,
                        second.1
                    );
                }
            }
            assert!(
                pairs > 10,
                "case {case:?}: the sheet carries families to order, got {pairs} pairs"
            );
        }
    }
}

/// The published row break: the index runs the symbol iconography and the
/// shields through the Korean expressway shields on the sheet's first row, and
/// opens the second row with the Korean national shield, because that is where
/// the next plate no longer fits the sheet.
#[tokio::test]
async fn the_shields_wrap_at_the_published_row_break() {
    let emulator = Emulator::new();
    let last_of_the_first_row = "shield-KR:expressway";
    let first_of_the_second_row = "shield-KR:national";
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            // Rows: both pixel ratios, so the break scales with the sheet.
            for (file, ratio) in [("sprites.json", 1_u32), ("sprites@2x.json", 2)] {
                let case = format!("{}/{}/{file}", style.as_wire(), scheme.as_wire());
                let index = sprite_index(&emulator, style, scheme, file).await;
                let rectangle = |name: &str| {
                    let entry = index
                        .get(name)
                        .unwrap_or_else(|| panic!("case {case:?}: the index must describe {name}"));
                    entry_rectangle(&case, name, entry)
                };

                let (x, y, width, _) = rectangle(last_of_the_first_row);
                assert_eq!(
                    y, 0,
                    "case {case:?}: {last_of_the_first_row} closes the first row, got y {y}"
                );
                assert_eq!(
                    x + width,
                    SPRITE_SHEET_WIDTH * ratio,
                    "case {case:?}: {last_of_the_first_row} ends the first row at the sheet's \
                     width, got {}",
                    x + width
                );

                let row_height = index
                    .iter()
                    .map(|(name, entry)| {
                        let (_, y, _, height) = entry_rectangle(&case, name, entry);
                        if y == 0 {
                            height
                        } else {
                            0
                        }
                    })
                    .max()
                    .unwrap_or_default();
                let (x, y, _, _) = rectangle(first_of_the_second_row);
                assert_eq!(
                    (x, y),
                    (0, row_height),
                    "case {case:?}: {first_of_the_second_row} opens the row below a \
                     {row_height}-pixel row, got {x},{y}"
                );
            }
        }
    }
}

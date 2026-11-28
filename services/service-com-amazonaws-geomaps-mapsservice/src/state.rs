// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-geomaps-mapsservice service: the map asset catalog.
//!
//! Geo Maps is a read-only content API — every operation it binds is
//! `@readonly` and there is no CRUD lifecycle — so this service's state is not
//! a resource table but an **asset catalog**: the style records, tilesets,
//! sprite sheets, font stacks, content types, cache directives and pricing
//! buckets that decide what each handler serves.
//!
//! The catalog is built by [`Default`], which is what `engine.state::<T>()`
//! calls the first time any handler touches this service's store. Seeding there
//! rather than from a handler is what keeps every operation a true read: no
//! request ever writes to this state, so what one caller sees is exactly what
//! the next one sees.
//!
//! Asset BYTES are not kept pre-rendered. Each handler derives them from the
//! catalog record plus the request's own addressing — tile coordinates, sprite
//! scale, glyph range, static-map framing — through [`assets`]. The derivation
//! is a pure function of (record, address), which gives the two properties a map
//! client needs: the same request always answers with byte-identical content and
//! therefore the same `ETag`, and changing a catalog record changes every asset
//! derived from it.

pub mod assets;
pub mod boundaries;
pub mod canvas;
pub mod glyph_pbf;
pub mod labels;
pub mod style_layers;
pub mod vector_tile;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use style_layers::StyleLayerSpec;

/// An 8-bit RGBA colour: the one representation the RASTERIZERS work in, since
/// a pixel of a tile or a static map is four bytes.
pub type Rgba = [u8; 4];

/// Opacity of a fully opaque [`Hsla`], in the thousandths the field counts in.
pub const OPAQUE: u16 = 1000;

/// A colour of the published style catalog, in the HSL notation a MapLibre
/// style document states its colours in.
///
/// HSL rather than RGB because that is the notation the published styles are
/// written in, and the two are not interchangeable: an achromatic colour keeps
/// the hue of the ramp it belongs to (`hsl(228, 0%, 87%)`), which a round trip
/// through RGB loses. [`Self::rgba`] converts to pixels for the rasterizers;
/// nothing converts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hsla {
    /// Hue in degrees, `0..360`.
    pub hue: u16,
    /// Saturation, in percent.
    pub saturation: u8,
    /// Lightness, in percent.
    pub lightness: u8,
    /// Opacity in thousandths, so [`OPAQUE`] is fully opaque. Counted in
    /// thousandths rather than held as a float so a published alpha is exact.
    pub alpha: u16,
}

impl Hsla {
    /// An opaque colour.
    pub const fn new(hue: u16, saturation: u8, lightness: u8) -> Self {
        Self {
            hue,
            saturation,
            lightness,
            alpha: OPAQUE,
        }
    }

    /// The same colour at `alpha` thousandths of full opacity.
    pub const fn with_alpha(self, alpha: u16) -> Self {
        Self { alpha, ..self }
    }

    /// The same colour `points` percentage points darker, clamped to black.
    /// How a casing or a shadow is derived from the surface it outlines: the
    /// same hue, less light.
    pub const fn darker(self, points: u8) -> Self {
        Self {
            lightness: self.lightness.saturating_sub(points),
            ..self
        }
    }

    /// Whether this colour is fully opaque.
    pub const fn is_opaque(self) -> bool {
        self.alpha >= OPAQUE
    }

    /// The colour as a MapLibre colour literal: `hsl(...)` when it is opaque,
    /// `hsla(...)` when it is not — the alpha channel is part of the colour, so
    /// a notation that cannot carry it would render translucent fog opaque.
    pub fn css(self) -> String {
        let Self {
            hue,
            saturation,
            lightness,
            ..
        } = self;
        if self.is_opaque() {
            return format!("hsl({hue}, {saturation}%, {lightness}%)");
        }
        let mut alpha = format!("{:.3}", f64::from(self.alpha) / f64::from(OPAQUE));
        while alpha.ends_with('0') {
            alpha.pop();
        }
        if alpha.ends_with('.') {
            alpha.pop();
        }
        format!("hsla({hue}, {saturation}%, {lightness}%, {alpha})")
    }

    /// The colour as a pixel, by the HSL-to-RGB conversion the CSS Color
    /// specification defines. The one direction anything converts in.
    pub fn rgba(self) -> Rgba {
        let hue = f64::from(self.hue % 360) / 60.0;
        let saturation = f64::from(self.saturation) / 100.0;
        let lightness = f64::from(self.lightness) / 100.0;
        let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
        let second = chroma * (1.0 - ((hue % 2.0) - 1.0).abs());
        let (red, green, blue) = match hue as u32 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let base = lightness - chroma / 2.0;
        let channel = |value: f64| ((value + base) * 255.0).round().clamp(0.0, 255.0) as u8;
        let alpha = (f64::from(self.alpha) / f64::from(OPAQUE) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
        [channel(red), channel(green), channel(blue), alpha]
    }
}

/// The value of a style-document paint property.
///
/// A MapLibre paint property is not a scalar: it may be an EXPRESSION, and a
/// published style states a colour that changes with zoom as one. The variant
/// is part of the catalog because a renderer that is handed a string where the
/// style declares an expression cannot vary the property at all.
#[derive(Debug, Clone)]
pub enum PaintValue {
    /// One colour, at every zoom.
    Flat(Hsla),
    /// A linear interpolation over zoom: `(zoom, colour)` stops in ascending
    /// zoom order, which MapLibre writes as an `interpolate` expression.
    ZoomRamp(Vec<(f64, Hsla)>),
}

/// The model's `MapStyle` enum (`Standard | Monochrome | Hybrid | Satellite`),
/// as a catalog key. The wire values are the ones `ops::types::MAP_STYLE_VALUES`
/// lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapStyleKind {
    Standard,
    Monochrome,
    Hybrid,
    Satellite,
}

impl MapStyleKind {
    /// Every value the model declares, in catalog order.
    pub const ALL: [Self; 4] = [
        Self::Standard,
        Self::Monochrome,
        Self::Hybrid,
        Self::Satellite,
    ];

    /// The wire value, or `None` when the string names no modeled style.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Standard" => Some(Self::Standard),
            "Monochrome" => Some(Self::Monochrome),
            "Hybrid" => Some(Self::Hybrid),
            "Satellite" => Some(Self::Satellite),
            _ => None,
        }
    }

    /// This style as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Monochrome => "Monochrome",
            Self::Hybrid => "Hybrid",
            Self::Satellite => "Satellite",
        }
    }
}

/// The model's `ColorScheme` enum (`Light | Dark`), as a catalog key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorSchemeKind {
    Light,
    Dark,
}

impl ColorSchemeKind {
    /// Every value the model declares.
    pub const ALL: [Self; 2] = [Self::Light, Self::Dark];

    /// The wire value, or `None` when the string names no modeled scheme.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Light" => Some(Self::Light),
            "Dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// This scheme as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// The model's `Variant` enum, which declares exactly one value today. It is
/// still a catalog key, because `GetSprites` binds it as its own path label and
/// a second variant would be a second sheet rather than a different style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VariantKind {
    Default,
}

impl VariantKind {
    /// Every value the model declares.
    pub const ALL: [Self; 1] = [Self::Default];

    /// The wire value, or `None` when the string names no modeled variant.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Default" => Some(Self::Default),
            _ => None,
        }
    }

    /// This variant as it appears on the wire and in a URL path.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Default => "Default",
        }
    }
}

/// The colours one (style, colour scheme) pair paints with. Every raster asset
/// and every style-document paint property reads them from here, so a style's
/// look is catalog data rather than a constant in a handler.
#[derive(Debug, Clone)]
pub struct Palette {
    pub land: Hsla,
    pub water: Hsla,
    pub road: Hsla,
    pub building: Hsla,
    pub graticule: Hsla,
    pub label: Hsla,
    pub marker: Hsla,
}

/// One source a style document declares: the tileset it reads and the URL
/// template a client follows to `GetTile` for it.
#[derive(Debug, Clone)]
pub struct StyleSource {
    /// The source name inside the style document (`aws`, `satellite`, ...).
    pub name: String,
    /// The `Tileset` path label `GetTile` takes.
    pub tileset: String,
    /// The `{z}/{x}/{y}` template a client expands to reach `GetTile`.
    pub tile_url_template: String,
}

/// The directional light a style's 3D layers are lit by, as the MapLibre
/// `light` block states it: an anchor the position is measured against and a
/// `[radial, azimuthal, polar]` position.
#[derive(Debug, Clone)]
pub struct LightRecord {
    /// `map` or `viewport`: what the light's position is measured against.
    pub anchor: String,
    /// How strongly the light is cast, `0..=1`.
    pub intensity: f64,
    /// `[radial distance, azimuthal angle, polar angle]`.
    pub position: [f64; 3],
}

/// The sky and atmosphere a style paints above the horizon, as the MapLibre
/// `sky` block states it. The colours come from the style's own palette, so a
/// scheme's sky belongs to the scheme; the blends are how far each band fades
/// into the next, `0..=1`.
#[derive(Debug, Clone)]
pub struct SkyRecord {
    pub sky_color: Hsla,
    pub horizon_color: Hsla,
    pub fog_color: Hsla,
    pub sky_horizon_blend: f64,
    pub horizon_fog_blend: f64,
    pub fog_ground_blend: f64,
    pub atmosphere_blend: f64,
}

/// The stored half of a style descriptor: everything that does not depend on
/// the request's feature knobs.
#[derive(Debug, Clone)]
pub struct StyleRecord {
    pub style: MapStyleKind,
    pub color_scheme: ColorSchemeKind,
    /// The style's canonical identifier, which is what the document reports as
    /// its `name` (see [`canonical_style_name`]).
    pub name: String,
    /// The identifier of this published style document: the canonical name's
    /// scope, lowercased, with the published version appended.
    pub id: String,
    /// The version of the style this catalog publishes, reported in the
    /// document's metadata and carried in [`Self::id`].
    pub version: String,
    /// When this style version was first published, RFC 3339. Catalog data, so
    /// every read of the style reports the same instant.
    pub created: String,
    /// When this style version last changed, RFC 3339.
    pub modified: String,
    /// The light the style's 3D layers are lit by.
    pub light: LightRecord,
    /// The sky the style paints above the horizon.
    pub sky: SkyRecord,
    /// The projection the style is rendered in (`globe`, `mercator`, ...).
    pub projection: String,
    pub palette: Palette,
    /// How the ground is painted. Published as a paint VALUE rather than a
    /// colour, because a style whose ground tightens as the buildings come in
    /// publishes it as a zoom ramp.
    pub background: PaintValue,
    /// The layer catalogue this style draws from, in draw order. Style-catalog
    /// data: many style layers read one tile layer, each drawing a different
    /// slice of it, so the list cannot be derived from the tile layer names. A
    /// document declares the entries whose tile layer a source it declares
    /// really serves, so a style with no vector basemap declares none of them.
    pub layers: &'static [StyleLayerSpec],
    /// The always-on sources, in draw order.
    pub sources: Vec<StyleSource>,
    /// The `GetSprites` URL template, without the `.png`/`.json` suffix a
    /// client appends (the MapLibre sprite convention).
    pub sprite_url_template: String,
    /// The `GetGlyphs` URL template, with the `{fontstack}`/`{range}`
    /// placeholders a client expands.
    pub glyph_url_template: String,
    /// The font stack the style's symbol layers ask for.
    pub font_stack: Vec<String>,
    pub content_type: String,
    pub cache_control: String,
}

/// How a tileset's tiles are encoded. It decides both the bytes a tile carries
/// and the `Content-Type` reported for it, so the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileFormat {
    /// A Mapbox Vector Tile (protobuf), built in [`vector_tile`].
    VectorMvt,
    /// A baseline JPEG raster tile, the encoding photographic imagery is
    /// served in.
    RasterJpeg,
    /// An RGB PNG raster tile whose pixels carry terrain elevation in the
    /// Terrarium encoding (`elevation = (r * 256 + g + b / 256) - 32768`).
    /// Lossless, because a lossy pixel is a wrong elevation.
    RasterDemPng,
}

/// A tileset `GetTile` serves: its addressing rules, its encoding and the
/// pricing bucket requests against it are charged to.
#[derive(Debug, Clone)]
pub struct TilesetRecord {
    pub id: String,
    pub format: TileFormat,
    pub content_type: String,
    /// The credit line a client must display for this tileset's data, which a
    /// style document reports on every source that reads it.
    pub attribution: String,
    /// Lowest zoom this tileset has tiles for; a lower `Z` is out of range.
    pub min_zoom: u32,
    /// Highest zoom this tileset has tiles for; a higher `Z` is out of range.
    pub max_zoom: u32,
    /// The layer names a vector tile from this tileset carries. Empty for a
    /// raster tileset.
    pub layers: Vec<String>,
    /// Which of [`Self::layers`] carry INCIDENT events — an individual
    /// obstruction of the network rather than its continuous state. A client
    /// can ask for the continuous state alone, which is every other layer, so
    /// the catalog states which layers these are rather than leaving it to a
    /// layer name's spelling.
    pub incident_layers: Vec<String>,
    /// Colours a raster tile from this tileset is drawn with.
    pub palette: Palette,
    /// The bucket `GetTile` reports; `None` falls back to
    /// [`ComAmazonawsGeomapsMapsserviceState::default_pricing_bucket`].
    pub pricing_bucket: Option<String>,
    pub cache_control: String,
}

/// The shape a sprite icon is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconShape {
    Disc,
    Square,
    Pin,
}

/// One icon of a published sprite sheet's index: the name a style layer
/// references, the rectangle the icon occupies in that sheet at `pixelRatio` 1,
/// its shape and its colour.
///
/// The rectangle is a width AND a height because a published sheet's icons are
/// not square: a one-way arrow is far wider than it is tall, a route shield is
/// wider than it is tall, and several icons have a footprint of their own
/// inside their family. It carries an `x`/`y` as well because an icon's offset
/// in the sheet is part of the published index rather than something a request
/// recomputes.
#[derive(Debug, Clone)]
pub struct IconSpec {
    pub name: String,
    /// The icon's left edge in the sheet, in style pixels.
    pub x: u32,
    /// The icon's top edge in the sheet, in style pixels.
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub shape: IconShape,
    pub tint: Rgba,
}

impl IconSpec {
    /// One icon of a family, not yet placed on a sheet: its name, its box and
    /// how it is drawn. [`place_icons_on_sheet`] gives it its offset.
    fn new(name: &str, width: u32, height: u32, shape: IconShape, tint: Rgba) -> Self {
        Self {
            name: name.to_string(),
            x: 0,
            y: 0,
            width,
            height,
            shape,
            tint,
        }
    }
}

/// A published sprite sheet: the icons it holds, each at its own place on it.
///
/// [`Self::icons`] IS the published index: `GetSprites` reports each icon's
/// stored rectangle rather than laying the sheet out again per request, and the
/// PNG is drawn from those same rectangles, so the index always describes the
/// sheet it is paired with.
#[derive(Debug, Clone)]
pub struct SpriteSheetRecord {
    pub style: MapStyleKind,
    pub color_scheme: ColorSchemeKind,
    pub variant: VariantKind,
    /// Every icon the sheet carries, in the order the published index lists
    /// them, each with the rectangle it occupies.
    pub icons: Vec<IconSpec>,
    pub cache_control: String,
}

/// A font stack `GetGlyphs` serves, and the metrics its glyph range is built
/// from.
#[derive(Debug, Clone)]
pub struct FontRecord {
    pub name: String,
    /// SDF em size in pixels (the size MapLibre rasterizes glyph ranges at).
    pub em: u32,
    /// Pixels of SDF padding kept on each side of a glyph bitmap.
    pub buffer: u32,
    /// Horizontal advance for an em-wide glyph, in pixels.
    pub advance: u32,
    /// Ascender height in pixels, measured from the baseline.
    pub ascender: i32,
    pub cache_control: String,
}

/// How the asset-delivery plane hands back a STORED map asset — a glyph range
/// or a sprite file.
///
/// Those assets are not built per request: they are objects of a versioned,
/// encrypted asset bucket the endpoint serves its static content from, so each
/// of them reports the object's stored media type and the bucket's object
/// headers rather than the media type of whatever is inside the object. The
/// recorded responses for a glyph range, for `sprites.png` and for
/// `sprites.json` all carry `Content-Type: binary/octet-stream` (S3's type for
/// an object stored without one) together with `x-amz-version-id` and
/// `x-amz-server-side-encryption: AES256`; the responses this service computes
/// from the request — a style descriptor, a tile, a static map — carry none of
/// that and keep their own media type.
#[derive(Debug, Clone)]
pub struct AssetDeliveryRecord {
    /// The media type a stored asset is delivered with.
    pub content_type: String,
    /// The server-side-encryption algorithm the asset bucket reports.
    pub server_side_encryption: String,
}

/// What `GetStaticMap` reports around the image it renders.
#[derive(Debug, Clone)]
pub struct StaticMapRecord {
    pub content_type: String,
    pub cache_control: String,
    /// The bucket `GetStaticMap` reports; `None` falls back to
    /// [`ComAmazonawsGeomapsMapsserviceState::default_pricing_bucket`].
    pub pricing_bucket: Option<String>,
}

/// Per-service state for com-amazonaws-geomaps-mapsservice. A handler reaches it through [`service_state`].
///
/// Seeded by [`Default`] (see the module docs): a handler only ever reads it.
#[derive(Debug)]
pub struct ComAmazonawsGeomapsMapsserviceState {
    /// Style records, keyed the way `GetStyleDescriptor` addresses them.
    pub styles: BTreeMap<(MapStyleKind, ColorSchemeKind), StyleRecord>,
    /// Tilesets, keyed by the `Tileset` path label `GetTile` takes. A label
    /// that is not a key here is a `ResourceNotFoundException`.
    pub tilesets: BTreeMap<String, TilesetRecord>,
    /// Sprite sheets, keyed the way `GetSprites` addresses them.
    pub sprites: BTreeMap<(MapStyleKind, ColorSchemeKind, VariantKind), SpriteSheetRecord>,
    /// Named font stacks `GetGlyphs` has metrics for.
    pub fonts: BTreeMap<String, FontRecord>,
    /// Metrics used for a font stack that is not in [`Self::fonts`].
    /// `GetGlyphs` models no error, so every modeled-valid stack must answer.
    pub fallback_font: FontRecord,
    pub static_maps: StaticMapRecord,
    /// The languages the catalog has label data for, as
    /// `GetStaticMapRequest$Language` documents them. A request that asks for
    /// one of these gets its labels; one that asks for anything else falls back
    /// to [`Self::primary_label_language`].
    pub label_languages: BTreeSet<String>,
    /// The "regional primary language" the `Language` documentation falls back
    /// to, and the language a request that names none gets.
    pub primary_label_language: String,
    /// How a STORED asset (a glyph range, a sprite file) is delivered.
    pub asset_delivery: AssetDeliveryRecord,
    /// The pricing bucket reported for an asset whose own record carries none.
    pub default_pricing_bucket: String,
}

impl Default for ComAmazonawsGeomapsMapsserviceState {
    fn default() -> Self {
        seed()
    }
}

impl ComAmazonawsGeomapsMapsserviceState {
    /// The pricing bucket to report for an asset record, falling back to
    /// [`Self::default_pricing_bucket`]. `PricingBucket` is `@required` on both
    /// `GetTile` and `GetStaticMap`, so this never answers an empty string.
    pub fn pricing_bucket(&self, record: Option<&String>) -> String {
        record
            .cloned()
            .unwrap_or_else(|| self.default_pricing_bucket.clone())
    }

    /// The language an image's labels are written in for a request that asked
    /// for `requested`.
    ///
    /// `Language` is a BCP 47 tag, so the language it selects is its primary
    /// subtag ([`labels::primary_subtag`]); a language this catalog has no
    /// label data for — and a request that named none — is answered in
    /// [`Self::primary_label_language`], which is what the member's own
    /// documentation says happens.
    pub fn label_language(&self, requested: Option<&str>) -> String {
        match requested.map(labels::primary_subtag) {
            Some(language) if self.label_languages.contains(&language) => language,
            _ => self.primary_label_language.clone(),
        }
    }
}

/// Build the seeded catalog.
///
/// Deterministic and allocation-only: no clock, no randomness, no rendering, so
/// two emulators seed identical catalogs and the first request pays almost
/// nothing for it.
fn seed() -> ComAmazonawsGeomapsMapsserviceState {
    let mut tilesets = BTreeMap::new();
    for record in seed_tilesets() {
        tilesets.insert(record.id.clone(), record);
    }

    let mut styles = BTreeMap::new();
    let mut sprites = BTreeMap::new();
    for style in MapStyleKind::ALL {
        for scheme in ColorSchemeKind::ALL {
            styles.insert((style, scheme), seed_style(style, scheme));
            for variant in VariantKind::ALL {
                sprites.insert(
                    (style, scheme, variant),
                    seed_sprite_sheet(style, scheme, variant),
                );
            }
        }
    }

    let mut fonts = BTreeMap::new();
    for record in seed_fonts() {
        fonts.insert(record.name.clone(), record);
    }

    ComAmazonawsGeomapsMapsserviceState {
        styles,
        tilesets,
        sprites,
        fonts,
        fallback_font: font_record("Standard Regular", 24, 18, 20),
        asset_delivery: AssetDeliveryRecord {
            content_type: "binary/octet-stream".to_string(),
            server_side_encryption: "AES256".to_string(),
        },
        static_maps: StaticMapRecord {
            // `GetStaticMapResponse$Blob` documents the body as a jpeg.
            content_type: "image/jpeg".to_string(),
            cache_control: "max-age=300".to_string(),
            pricing_bucket: Some("StaticMaps".to_string()),
        },
        // The label languages are the modeled ones: the set `Language`'s own
        // documentation lists, not a shorter one chosen here.
        label_languages: labels::SUPPORTED_LANGUAGES
            .iter()
            .map(|language| (*language).to_string())
            .collect(),
        primary_label_language: labels::PRIMARY_LANGUAGE.to_string(),
        default_pricing_bucket: "MapAssets".to_string(),
    }
}

/// The palette for one (style, colour scheme) pair, in the HSL notation the
/// published style documents state their colours in.
fn palette(style: MapStyleKind, scheme: ColorSchemeKind) -> Palette {
    match (style, scheme) {
        (MapStyleKind::Standard, ColorSchemeKind::Light) => Palette {
            land: Hsla::new(36, 24, 96),
            water: Hsla::new(210, 73, 78),
            road: Hsla::new(0, 0, 100),
            building: Hsla::new(39, 21, 87),
            graticule: Hsla::new(37, 19, 82),
            label: Hsla::new(0, 0, 20),
            marker: Hsla::new(16, 90, 45),
        },
        (MapStyleKind::Standard, ColorSchemeKind::Dark) => Palette {
            land: Hsla::new(180, 5, 16),
            water: Hsla::new(211, 50, 16),
            road: Hsla::new(213, 7, 25),
            building: Hsla::new(214, 7, 21),
            graticule: Hsla::new(214, 6, 24),
            label: Hsla::new(0, 0, 90),
            marker: Hsla::new(14, 100, 63),
        },
        // The Monochrome styles are a single desaturated ramp, so every entry
        // keeps the ramp's hue and differs only in lightness.
        (MapStyleKind::Monochrome, ColorSchemeKind::Light) => Palette {
            land: Hsla::new(MONOCHROME_HUE, 0, 87),
            water: Hsla::new(MONOCHROME_HUE, 0, 85),
            road: Hsla::new(MONOCHROME_HUE, 0, 100),
            building: Hsla::new(MONOCHROME_HUE, 0, 90),
            graticule: Hsla::new(MONOCHROME_HUE, 0, 80),
            label: Hsla::new(MONOCHROME_HUE, 0, 15),
            marker: Hsla::new(MONOCHROME_HUE, 0, 35),
        },
        (MapStyleKind::Monochrome, ColorSchemeKind::Dark) => Palette {
            land: Hsla::new(MONOCHROME_HUE, 0, 12),
            water: Hsla::new(MONOCHROME_HUE, 0, 18),
            road: Hsla::new(MONOCHROME_HUE, 0, 30),
            building: Hsla::new(MONOCHROME_HUE, 0, 20),
            graticule: Hsla::new(MONOCHROME_HUE, 0, 24),
            label: Hsla::new(MONOCHROME_HUE, 0, 95),
            marker: Hsla::new(MONOCHROME_HUE, 0, 65),
        },
        (MapStyleKind::Hybrid | MapStyleKind::Satellite, ColorSchemeKind::Light) => Palette {
            land: Hsla::new(93, 23, 30),
            water: Hsla::new(211, 55, 23),
            road: Hsla::new(48, 47, 83),
            building: Hsla::new(40, 8, 39),
            graticule: Hsla::new(89, 11, 49),
            label: Hsla::new(0, 0, 100),
            marker: Hsla::new(46, 100, 65),
        },
        (MapStyleKind::Hybrid | MapStyleKind::Satellite, ColorSchemeKind::Dark) => Palette {
            land: Hsla::new(93, 23, 17),
            water: Hsla::new(211, 55, 13),
            road: Hsla::new(47, 21, 65),
            building: Hsla::new(42, 8, 25),
            graticule: Hsla::new(92, 11, 29),
            label: Hsla::new(0, 0, 94),
            marker: Hsla::new(45, 100, 51),
        },
    }
}

/// The hue the Monochrome styles' desaturated ramp is written at. A zero
/// saturation shows no hue, but the published notation still states one, and
/// the colour literal a client reads is the published text.
const MONOCHROME_HUE: u16 = 228;

/// How the ground of one style is painted.
///
/// A Monochrome ground is published as a zoom ramp: the ground darkens by
/// [`MONOCHROME_GROUND_STEP`] percentage points once the building footprints
/// come in, so the buildings stay legible against it. Every other style paints
/// the ground in its land colour at every zoom.
fn background(style: MapStyleKind, palette: &Palette) -> PaintValue {
    match style {
        MapStyleKind::Monochrome => PaintValue::ZoomRamp(vec![
            (MONOCHROME_GROUND_ZOOM, palette.land),
            (
                BUILDING_ZOOM,
                palette.land.darker(MONOCHROME_GROUND_STEP),
            ),
        ]),
        _ => PaintValue::Flat(palette.land),
    }
}

/// The zoom a Monochrome ground starts darkening at, and how far it darkens by
/// the zoom the buildings come in at.
const MONOCHROME_GROUND_ZOOM: f64 = 15.5;
const MONOCHROME_GROUND_STEP: u8 = 2;

/// The zoom the building footprints come in at: the zoom their style layers
/// start drawing at, and the top of the Monochrome ground ramp.
pub const BUILDING_ZOOM: f64 = 16.0;

/// The tilesets the documented `Tileset` values name.
///
/// The value list is `GetTileRequest$Tileset`'s own documentation:
/// `raster.satellite | vector.basemap | vector.traffic | raster.dem`
/// (`smithy_query(query="doc", shape="GetTileRequest", member="Tileset")`).
fn seed_tilesets() -> Vec<TilesetRecord> {
    let basemap_palette = palette(MapStyleKind::Standard, ColorSchemeKind::Light);
    let satellite_palette = palette(MapStyleKind::Satellite, ColorSchemeKind::Light);
    vec![
        TilesetRecord {
            id: "vector.basemap".to_string(),
            format: TileFormat::VectorMvt,
            content_type: "application/vnd.mapbox-vector-tile".to_string(),
            attribution: attribution(),
            min_zoom: 0,
            max_zoom: VECTOR_MAX_ZOOM,
            layers: vec![
                GROUND_LAYER.to_string(),
                WATER_LAYER.to_string(),
                LANDUSE_LAYER.to_string(),
                ROAD_LAYER.to_string(),
                BUILDING_LAYER.to_string(),
                POI_LAYER.to_string(),
                ADMIN_LAYER.to_string(),
            ],
            incident_layers: Vec::new(),
            palette: basemap_palette.clone(),
            pricing_bucket: Some("MapTiles".to_string()),
            cache_control: "max-age=86400".to_string(),
        },
        TilesetRecord {
            id: "vector.traffic".to_string(),
            format: TileFormat::VectorMvt,
            content_type: "application/vnd.mapbox-vector-tile".to_string(),
            attribution: attribution(),
            min_zoom: 0,
            max_zoom: VECTOR_MAX_ZOOM,
            layers: vec![
                TRAFFIC_FLOW_LAYER.to_string(),
                TRAFFIC_INCIDENT_LAYER.to_string(),
            ],
            incident_layers: vec![TRAFFIC_INCIDENT_LAYER.to_string()],
            palette: basemap_palette,
            pricing_bucket: Some("TrafficTiles".to_string()),
            cache_control: "max-age=60".to_string(),
        },
        TilesetRecord {
            id: "raster.satellite".to_string(),
            format: TileFormat::RasterJpeg,
            content_type: "image/jpeg".to_string(),
            attribution: attribution(),
            min_zoom: 0,
            max_zoom: 19,
            layers: Vec::new(),
            incident_layers: Vec::new(),
            palette: satellite_palette,
            pricing_bucket: Some("SatelliteTiles".to_string()),
            cache_control: "max-age=86400".to_string(),
        },
        TilesetRecord {
            id: "raster.dem".to_string(),
            format: TileFormat::RasterDemPng,
            content_type: "image/png".to_string(),
            attribution: attribution(),
            min_zoom: 0,
            max_zoom: 14,
            layers: Vec::new(),
            incident_layers: Vec::new(),
            // A DEM tile's pixels are elevation, not colour; the palette is
            // carried for uniformity and is unused by the Terrarium encoding.
            palette: palette(MapStyleKind::Monochrome, ColorSchemeKind::Light),
            // Deliberately none: this record exercises the
            // `default_pricing_bucket` fallback.
            pricing_bucket: None,
            cache_control: "max-age=86400".to_string(),
        },
    ]
}

/// The style record for one (style, colour scheme) pair.
fn seed_style(style: MapStyleKind, scheme: ColorSchemeKind) -> StyleRecord {
    let variant = VariantKind::Default;
    let mut sources = Vec::new();
    if matches!(style, MapStyleKind::Hybrid | MapStyleKind::Satellite) {
        sources.push(style_source(SATELLITE_SOURCE, "raster.satellite"));
    }
    if !matches!(style, MapStyleKind::Satellite) {
        sources.push(style_source(BASEMAP_SOURCE, "vector.basemap"));
    }
    let palette = palette(style, scheme);
    let scope = style_scope(style, scheme, variant);
    StyleRecord {
        style,
        color_scheme: scheme,
        name: canonical_style_name(style, scheme, variant),
        id: format!("{}-{STYLE_VERSION}", scope.to_lowercase()),
        version: STYLE_VERSION.to_string(),
        created: STYLE_CREATED.to_string(),
        modified: STYLE_MODIFIED.to_string(),
        light: LightRecord {
            anchor: LIGHT_ANCHOR.to_string(),
            // The light block is the published style's own and does not vary
            // with the colour scheme: the same light, cast on a different
            // palette, is what a dark scheme is.
            intensity: LIGHT_INTENSITY,
            position: LIGHT_POSITION,
        },
        sky: sky(scheme),
        projection: PROJECTION.to_string(),
        background: background(style, &palette),
        layers: style_layers::BASEMAP_LAYERS,
        palette,
        sources,
        sprite_url_template: assets::sprite_url_template(style, scheme, variant),
        glyph_url_template: assets::GLYPH_URL_TEMPLATE.to_string(),
        font_stack: match style {
            MapStyleKind::Monochrome => vec!["Amazon Ember Condensed RC Regular".to_string()],
            _ => vec!["Amazon Ember Regular".to_string()],
        },
        content_type: "application/json".to_string(),
        cache_control: "max-age=3600".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The published style catalog's own identity
// ---------------------------------------------------------------------------

/// The prefix every published style's canonical name carries.
pub const STYLE_NAME_PREFIX: &str = "AmazonLocation";

/// The geographic coverage of the styles this catalog publishes: one worldwide
/// map rather than a per-region cut, which is the middle term of a style's
/// canonical name.
pub const STYLE_COVERAGE: &str = "Global";

/// The version of the style documents this catalog publishes, as ONE release
/// version string. It is reported in a document's metadata and is the suffix of
/// its `id`, so a client can tell two published versions apart.
///
/// A style release is versioned semantically, so its version is a three-part
/// core and, while the release is still a pre-release of that core, a
/// qualifier after it. The qualifier is PART OF the version — `4.1.0-beta.1`
/// and `4.1.0` are two different releases — so the whole string is carried
/// verbatim into both `metadata.version` and the document's `id`, and nothing
/// reassembles a version from its core alone: two releases that differ only in
/// the qualifier would otherwise report one identity and collide in a client's
/// cache.
pub const STYLE_VERSION: &str = "4.1.0-beta.1";

/// When [`STYLE_VERSION`] was published, and when it last changed. Catalog data
/// rather than a clock reading: a style document is published content, so every
/// read of it reports the same two instants.
pub const STYLE_CREATED: &str = "2025-02-03T00:00:00Z";
pub const STYLE_MODIFIED: &str = "2025-02-03T00:00:00Z";

/// What a style's `light` position is measured against, the
/// `[radial, azimuthal, polar]` position itself, and how strongly it is cast.
const LIGHT_ANCHOR: &str = "viewport";
const LIGHT_POSITION: [f64; 3] = [1.5, 235.0, 30.0];
const LIGHT_INTENSITY: f64 = 0.3;

/// The projection every published style is rendered in.
const PROJECTION: &str = "globe";

/// The sky and atmosphere the published styles paint above the horizon.
///
/// It belongs to the COLOUR SCHEME rather than to the palette: the sky is the
/// real one above the map, so a light scheme shows daylight sky over white haze
/// and a dark scheme shows night sky, and neither is the ground's colour. The
/// fog is translucent, which is why a colour notation that carries alpha is the
/// only one that can state it.
fn sky(scheme: ColorSchemeKind) -> SkyRecord {
    let (sky_color, horizon_color, fog_color) = match scheme {
        ColorSchemeKind::Light => (
            Hsla::new(208, 90, 72),
            Hsla::new(0, 0, 100),
            Hsla::new(0, 0, 100).with_alpha(FOG_ALPHA),
        ),
        ColorSchemeKind::Dark => (
            Hsla::new(208, 30, 18),
            Hsla::new(0, 0, 24),
            Hsla::new(0, 0, 8).with_alpha(FOG_ALPHA),
        ),
    };
    SkyRecord {
        sky_color,
        horizon_color,
        fog_color,
        sky_horizon_blend: 0.2,
        horizon_fog_blend: 0.9,
        fog_ground_blend: 0.8,
        atmosphere_blend: 0.2,
    }
}

/// How opaque the fog band is, in [`Hsla`]'s thousandths.
const FOG_ALPHA: u16 = 200;

/// One supplier of the map data this catalog serves, and where that supplier's
/// own notice lives.
#[derive(Debug, Clone, Copy)]
pub struct DataSupplier {
    pub name: &'static str,
    pub notice_url: &'static str,
}

/// Every supplier of this catalog's map data, in the order the credit line
/// names them: the service that serves the tiles, then the map-content supplier
/// whose data they carry.
///
/// Source: the data-provider attribution the Amazon Location Service Developer
/// Guide requires a client to display for map resources, which names the
/// service and each content supplier behind the tiles with a link to that
/// supplier's terms.
pub const DATA_SUPPLIERS: &[DataSupplier] = &[
    DataSupplier {
        name: "AWS",
        notice_url: "https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html",
    },
    DataSupplier {
        name: "HERE",
        notice_url: "https://legal.here.com/en-gb/terms/general-content-supplier-terms-and-notices",
    },
];

/// The credit line this catalog's map data is served under.
///
/// Every supplier in [`DATA_SUPPLIERS`] is named, each linked to its own
/// notice: a client renders this line verbatim, so a line that names fewer
/// suppliers than the tiles carry data from leaves a required notice
/// undisplayed.
pub fn attribution() -> String {
    let mut line = String::from("\u{a9} ");
    for (index, supplier) in DATA_SUPPLIERS.iter().enumerate() {
        if index > 0 {
            line.push_str(", ");
        }
        let _ = write!(
            line,
            "<a href='{}'>{}</a>",
            supplier.notice_url, supplier.name
        );
    }
    line
}

/// The highest zoom the vector tilesets have tiles for.
pub const VECTOR_MAX_ZOOM: u32 = 17;

/// The style-document source name of the vector basemap, and of the satellite
/// imagery. The basemap is the source a client reads the map itself from.
pub const BASEMAP_SOURCE: &str = "aws";
pub const SATELLITE_SOURCE: &str = "satellite";

/// The basemap layer that carries the ground. A style paints it as the
/// document's `background` layer, which is what lies under everything else.
pub const GROUND_LAYER: &str = "land";

/// The basemap layer that carries inland and coastal water.
pub const WATER_LAYER: &str = "water";

/// The basemap layer that carries land use: the surfaces a style fills before
/// anything is drawn on them.
pub const LANDUSE_LAYER: &str = "landuse";

/// The basemap layer that carries the road, rail and ferry network.
pub const ROAD_LAYER: &str = "roads";

/// The basemap layer that carries administrative boundaries.
pub const ADMIN_LAYER: &str = "admin";

/// The basemap layer that carries points of interest, which `PoiDensity` and
/// `PoiCategories` project onto.
pub const POI_LAYER: &str = "places";

/// The basemap layer that carries building footprints, which `Buildings`
/// projects onto.
pub const BUILDING_LAYER: &str = "buildings";

/// The traffic layer that carries the continuous flow of traffic.
pub const TRAFFIC_FLOW_LAYER: &str = "traffic_flow";

/// The traffic layer that carries incident events, as opposed to the flow of
/// traffic the rest of that tileset's layers carry.
pub const TRAFFIC_INCIDENT_LAYER: &str = "traffic_incidents";

/// The scope part of a published style's identity: the style, its coverage, its
/// colour scheme and its variant, in the order the canonical name writes them.
pub fn style_scope(style: MapStyleKind, scheme: ColorSchemeKind, variant: VariantKind) -> String {
    format!(
        "{}-{STYLE_COVERAGE}-{}-{}",
        style.as_wire(),
        scheme.as_wire(),
        variant.as_wire()
    )
}

/// The canonical name a published style document reports: the prefix every
/// published style carries, followed by its scope.
pub fn canonical_style_name(
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    variant: VariantKind,
) -> String {
    format!("{STYLE_NAME_PREFIX}-{}", style_scope(style, scheme, variant))
}

fn style_source(name: &str, tileset: &str) -> StyleSource {
    StyleSource {
        name: name.to_string(),
        tileset: tileset.to_string(),
        tile_url_template: assets::tile_url_template(tileset),
    }
}

/// The sprite sheet for one (style, colour scheme, variant) triple: the
/// iconography that sheet publishes, each icon at the rectangle the published
/// index reports for it, tinted from that pair's palette — so the `Dark` sheet
/// really is a different sheet rather than the same bytes under another key.
///
/// The sheet's geometry is catalog data: it is fixed here, once, from each
/// icon's own published footprint and the published listing order, so a request
/// never invents an atlas. The icon NAMES are read per sheet too, because two
/// published sheets of this catalog do not carry the same shield name set.
fn seed_sprite_sheet(
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    variant: VariantKind,
) -> SpriteSheetRecord {
    let p = palette(style, scheme);
    // The whole iconography of the sheet: places, traffic incidents, city dots,
    // map pins, route shields and the heavy-goods-vehicle restriction signs.
    // One sequence, because the published sheet packs them as one — not a row
    // per family.
    let shields = route_shield_icons(style, scheme, variant);
    let mut icons = Vec::with_capacity(
        PLACE_ICONS.len()
            + INCIDENT_ICONS.len()
            + CITY_DOT_ICONS.len()
            + MARKER_ICONS.len()
            + shields.len()
            + RESTRICTION_SHIELD_ICONS.len(),
    );
    for name in PLACE_ICONS {
        let (width, height) = place_icon_box(name);
        icons.push(IconSpec::new(
            name,
            width,
            height,
            IconShape::Disc,
            p.label.rgba(),
        ));
    }
    for name in INCIDENT_ICONS {
        icons.push(IconSpec::new(
            name,
            INCIDENT_ICON_BOX,
            INCIDENT_ICON_BOX,
            IconShape::Square,
            p.marker.rgba(),
        ));
    }
    for name in CITY_DOT_ICONS {
        let box_size = city_dot_box(name);
        icons.push(IconSpec::new(
            name,
            box_size,
            box_size,
            IconShape::Disc,
            p.label.rgba(),
        ));
    }
    for name in MARKER_ICONS {
        icons.push(IconSpec::new(
            name,
            MARKER_BOX,
            MARKER_BOX,
            IconShape::Pin,
            p.marker.rgba(),
        ));
    }
    for name in shields {
        let (width, height) = route_shield_box(name);
        icons.push(IconSpec::new(
            name,
            width,
            height,
            IconShape::Square,
            p.building.rgba(),
        ));
    }
    for name in RESTRICTION_SHIELD_ICONS {
        icons.push(IconSpec::new(
            name,
            RESTRICTION_SHIELD_BOX,
            RESTRICTION_SHIELD_BOX,
            IconShape::Square,
            p.road.rgba(),
        ));
    }
    SpriteSheetRecord {
        style,
        color_scheme: scheme,
        variant,
        icons: place_icons_on_sheet(in_published_order(icons)),
        cache_control: "max-age=86400".to_string(),
    }
}

/// Give every icon its offset on the sheet: one sequence of icons, in the
/// published listing order, packed left to right and wrapped at the sheet's
/// width.
///
/// Each icon abuts the one listed before it, so an entry's `x` is the previous
/// entry's `x` plus that entry's own width; an icon that no longer fits starts a
/// new row directly below, as tall as the tallest icon of the row it closes.
/// The families are NOT rows of their own: the published index runs the shields
/// on from the symbol iconography in the same row and wraps in the middle of
/// them, which is why the offsets follow the listing order and the sheet's
/// width rather than the family an icon belongs to.
fn place_icons_on_sheet(icons: Vec<IconSpec>) -> Vec<IconSpec> {
    let mut placed = Vec::with_capacity(icons.len());
    let mut x = 0;
    let mut y = 0;
    let mut row_height = 0;
    for mut icon in icons {
        if x > 0 && x + icon.width > SPRITE_SHEET_WIDTH {
            x = 0;
            y += row_height;
            row_height = 0;
        }
        icon.x = x;
        icon.y = y;
        x += icon.width;
        row_height = row_height.max(icon.height);
        placed.push(icon);
    }
    placed
}

/// The sheet's icons in the order the published index lists them.
///
/// The index lists an icon FAMILY together — every name the family's own root
/// name is a prefix of, as `citydot` roots the four city dots and
/// `shield-CN:expressway` roots the plate that widens it — and lists a family's
/// members widest plate first, which is why the open city ring is listed before
/// the filled dot it is drawn around and a `-long` number plate before the
/// plate it widens. A name no other name is a prefix of is a family of its own,
/// so for those the name alone decides the place and the width tiebreak never
/// comes into it.
fn in_published_order(icons: Vec<IconSpec>) -> Vec<IconSpec> {
    let names: Vec<String> = icons.iter().map(|icon| icon.name.clone()).collect();
    let mut keyed: Vec<(usize, std::cmp::Reverse<u32>, IconSpec)> = icons
        .into_iter()
        .enumerate()
        .map(|(own, icon)| {
            let family = published_icon_family(&icon.name, &names, own);
            (family, std::cmp::Reverse(icon.width), icon)
        })
        .collect();
    // Every family key is a place in `names`, which those keys were read from.
    keyed.sort_by(|left, right| {
        (names[left.0].as_str(), left.1, left.2.name.as_str())
            .cmp(&(names[right.0].as_str(), right.1, right.2.name.as_str()))
    });
    keyed.into_iter().map(|(_, _, icon)| icon).collect()
}

/// Which name of the sheet is the family the published index lists `name`
/// under: the shortest name that is a prefix of it. Answered as that name's
/// place in `names`, so no family key is copied per icon.
///
/// `own` is `name`'s own place, which is the answer when no other name is a
/// prefix of it — a name is a prefix of itself, so the scan finds it anyway, and
/// passing it in keeps the answer a place of THIS icon rather than of whichever
/// icon the sheet happens to list first.
fn published_icon_family(name: &str, names: &[String], own: usize) -> usize {
    names
        .iter()
        .enumerate()
        .filter(|(_, candidate)| name.starts_with(candidate.as_str()))
        .min_by_key(|(_, candidate)| candidate.len())
        .map_or(own, |(index, _)| index)
}

// ---------------------------------------------------------------------------
// The published sprite-sheet iconography
// ---------------------------------------------------------------------------

/// How wide the published sheet is, in style pixels: the width the published
/// index's rows wrap at.
///
/// Source for this, as for every footprint and name in this section: the
/// `sprites.json` index the Maps asset endpoint serves for a sheet — the icon
/// catalog `GetSprites`' own model documentation points at ("Style iconography
/// with sprites" in the Amazon Location Service Developer Guide). The index
/// reports the symbol iconography and the route shields through the Korean
/// expressway shields on the first row, and the rest of the shields from the
/// Korean national shield on, so the sheet is as wide as that first row and not
/// as wide as its whole iconography.
///
/// The NUMBER therefore follows from that row break and from the footprints
/// this section carries, which is why the two are one piece of catalog data: it
/// is the first row's own width, so a footprint corrected in this section moves
/// it, and the index's row break is what says where it has to land.
pub const SPRITE_SHEET_WIDTH: u32 = 1958;

/// The box most place (point-of-interest) icons occupy at `pixelRatio` 1: the
/// published index reports this square for every place icon that does not
/// appear in [`PLACE_ICON_BOXES`] (`accommodation` and `airport` are 21x21).
pub const PLACE_ICON_BOX: u32 = 21;

/// The place icons the published index reports at a footprint of their own
/// rather than at the family's square [`PLACE_ICON_BOX`]: a published sheet's
/// icons are drawn art, so an icon is as wide and as tall as its own artwork.
///
/// `oneway` is a wide, short arrow drawn along the road it marks, and `other`
/// is the generic place glyph, drawn at the size of a traffic-incident icon
/// rather than of a place icon.
const PLACE_ICON_BOXES: &[(&str, u32, u32)] = &[
    ("bicycle", 22, 20),
    ("bus", 20, 20),
    ("entrance", 20, 20),
    ("oneway", 50, 29),
    ("other", 32, 32),
];

/// The box the place icon `name` occupies at `pixelRatio` 1: its own published
/// footprint where [`PLACE_ICON_BOXES`] gives it one, otherwise the family's
/// square [`PLACE_ICON_BOX`].
fn place_icon_box(name: &str) -> (u32, u32) {
    PLACE_ICON_BOXES
        .iter()
        .find(|(icon, _, _)| *icon == name)
        .map_or((PLACE_ICON_BOX, PLACE_ICON_BOX), |(_, width, height)| {
            (*width, *height)
        })
}

/// The box a traffic-incident icon occupies: bigger than a place icon, because
/// an incident is drawn over the road it blocks (`accident` is 32x32).
pub const INCIDENT_ICON_BOX: u32 = 32;

/// The name every city-dot icon starts with — the family the published index
/// lists the four dots under.
pub const CITY_DOT_FAMILY: &str = "citydot";

/// The box a filled city dot occupies, and the box the open ring occupies: the
/// ring is drawn a pixel wider and taller than the filled dot. A dot's RANK
/// (`_lowrank`) does not change its box — the published index reports
/// `citydot` and `citydot_lowrank` at the same size, and likewise both open
/// dots.
pub const CITY_DOT_BOX: u32 = 5;
pub const CITY_DOT_OPEN_BOX: u32 = 6;

/// The box a map pin (`marker`, `marker_*`) occupies: the published index
/// reports every pin of the family at this square, the same box as a place
/// icon.
pub const MARKER_BOX: u32 = 21;

/// The height of a route shield the published index reports at no footprint of
/// its own, and the width of the box that holds a two-digit route number. Such
/// a shield grows by [`SHIELD_DIGIT_WIDTH`] for every digit beyond
/// [`SHIELD_BASE_DIGITS`] it must hold; a shield [`ROUTE_SHIELD_BOXES`] gives
/// its own plate keeps that plate's box instead.
pub const SHIELD_HEIGHT: u32 = 20;
pub const SHIELD_BASE_WIDTH: u32 = 20;
pub const SHIELD_BASE_DIGITS: u32 = 2;
pub const SHIELD_DIGIT_WIDTH: u32 = 7;
/// The widest route number a shield in this catalog holds.
pub const SHIELD_MAX_DIGITS: u32 = 4;

/// The box a heavy-goods-vehicle restriction sign (`shield-hgv-*`) occupies:
/// a regulatory sign, square and larger than a route shield.
pub const RESTRICTION_SHIELD_BOX: u32 = 24;

/// The city-dot box for one dot name: the open ring is the wider one. A dot's
/// RANK suffix does not change its box.
fn city_dot_box(name: &str) -> u32 {
    let is_open = name
        .strip_prefix(CITY_DOT_FAMILY)
        .is_some_and(|rest| rest.starts_with("-open"));
    if is_open {
        CITY_DOT_OPEN_BOX
    } else {
        CITY_DOT_BOX
    }
}

/// The route shields the published index reports at a footprint of their own
/// rather than at the box the digit-capacity rule below produces.
///
/// A published sheet's shields are drawn plates, so a shield is as wide and as
/// tall as its own artwork: a plate that carries its number above a route word
/// is taller than the digits it holds, a plate drawn around a long network word
/// is wider, and a plate drawn as a square is both. That footprint is catalog
/// data per shield, which is why it is read here per name instead of being
/// derived; where a network's plate is carried under a second, short name as
/// well (`shield-BAB` beside `shield-DE:BAB`, `shield-AE` beside
/// `shield-AE:E-route`) both names describe the same plate and so report the
/// same box.
///
/// Source for every footprint here, as for the names and the offsets in this
/// section: the `sprites.json` index the Maps asset endpoint serves for a sheet
/// — the icon catalog `GetSprites`' own model documentation points at ("Style
/// iconography with sprites" in the Amazon Location Service Developer Guide).
/// A shield this list does not name is reported at the box its name's digit
/// capacity produces.
const ROUTE_SHIELD_BOXES: &[(&str, u32, u32)] = &[
    ("shield-AE", 21, 30),
    ("shield-AE:E-route", 21, 30),
    ("shield-AU:MR", 19, 20),
    ("shield-AU:NH", 19, 20),
    ("shield-AU:T", 21, 22),
    ("shield-BAB", 36, 20),
    ("shield-BR", 22, 27),
    ("shield-CA-TransCanada", 24, 24),
    ("shield-CA:transcanada", 24, 24),
    ("shield-CN:expressway", 30, 28),
    ("shield-CN:expressway-long", 43, 28),
    ("shield-CR:CR", 23, 26),
    ("shield-CR:national", 23, 26),
    ("shield-DE:BAB", 36, 20),
];

/// The box the route shield `name` occupies at `pixelRatio` 1: its own
/// published footprint where [`ROUTE_SHIELD_BOXES`] gives it one, otherwise the
/// width its name's digit capacity states at the family's [`SHIELD_HEIGHT`].
pub fn route_shield_box(name: &str) -> (u32, u32) {
    ROUTE_SHIELD_BOXES
        .iter()
        .find(|(icon, _, _)| *icon == name)
        .map_or((route_shield_width(name), SHIELD_HEIGHT), |(_, width, height)| {
            (*width, *height)
        })
}

/// How wide the box of a route shield `name` with no published footprint of its
/// own is.
///
/// Such a shield is as wide as the longest route number it has to hold, which
/// its own name states: either as a width word (`-long`, `-xlong`, `-wide`) or
/// as a trailing digit-capacity suffix (`-3`, `-1-2`, `-3-4`). Everything else
/// holds the catalog's base of [`SHIELD_BASE_DIGITS`] digits.
pub fn route_shield_width(name: &str) -> u32 {
    SHIELD_BASE_WIDTH + (route_shield_digits(name) - SHIELD_BASE_DIGITS) * SHIELD_DIGIT_WIDTH
}

/// The widest route number the shield `name` holds, in digits.
fn route_shield_digits(name: &str) -> u32 {
    if name.ends_with("-xlong") {
        return SHIELD_MAX_DIGITS;
    }
    if name.ends_with("-long") || name.ends_with("-wide") {
        return SHIELD_BASE_DIGITS + 1;
    }
    name.rsplit('-')
        .take_while(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|segment| segment.parse::<u32>().ok())
        .max()
        .unwrap_or(SHIELD_BASE_DIGITS)
        .clamp(SHIELD_BASE_DIGITS, SHIELD_MAX_DIGITS)
}

/// The place (point-of-interest) icons a published sheet carries.
///
/// Source for every name in this section: the `sprites.json` manifest the Maps
/// asset endpoint serves for a sheet — the icon catalog `GetSprites`' own model
/// documentation points at ("Style iconography with sprites" in the Amazon
/// Location Service Developer Guide). The names are catalog data; the box each
/// family occupies and the shape it is drawn as are the constants above.
const PLACE_ICONS: &[&str] = &[
    "accommodation",
    "airport",
    "attraction",
    "bicycle",
    "book",
    "bus",
    "cable-car",
    "camping",
    "car",
    "cargo",
    "cemetery",
    "charging-station",
    "cinema",
    "cocktail",
    "coffee",
    "delivery-entrance",
    "doctor",
    "education",
    "entrance",
    "facilities",
    "fast-food",
    "ferry",
    "funicular",
    "game",
    "gas-station",
    "government",
    "grocery",
    "gym",
    "harbour",
    "hospital",
    "information",
    "light-rail",
    "loading-zone",
    "metro",
    "money",
    "monorail",
    "museum",
    "nature",
    "nightclub",
    "oneway",
    "other",
    "outdoor",
    "park",
    "parking",
    "pharmacy",
    "place-of-worship",
    "police",
    "pub",
    "rail-yard",
    "restaurant",
    "salon",
    "shopping",
    "ski",
    "stadium",
    "swimming",
    "taxi",
    "theater",
    "toll-booth",
    "train",
    "truck-parking",
    "truck-services",
    "truck-weigh-station",
];

/// The traffic-incident icons a published sheet carries.
const INCIDENT_ICONS: &[&str] = &["accident", "congestion", "construction", "road_closure"];

/// The city-dot icons a published sheet carries: the dot a place label is
/// anchored to, open or filled, at full and at low rank.
const CITY_DOT_ICONS: &[&str] = &[
    "citydot",
    "citydot-open",
    "citydot-open_lowrank",
    "citydot_lowrank",
];

/// The map-pin icons a published sheet carries.
const MARKER_ICONS: &[&str] = &[
    "marker",
    "marker_arts_entertainment",
    "marker_food_drink",
    "marker_recreation",
    "marker_stores_services",
    "marker_transit",
];

/// The route shields every published sheet of this catalog carries, except the
/// Trans-Canada shield, whose name differs per sheet ([`route_shield_icons`]).
///
/// A shield's published name is the road NETWORK it belongs to, written as that
/// network's own namespaced identifier (`shield-AU:NH`, `shield-JP:prefectural`,
/// `shield-ch:national`) where the network has one, and the shield's shape or
/// route family (`shield-rectangle-blue`, `shield-US-Hwy-1-2`) where it does
/// not. A few networks are carried under a SECOND, short name as well — the
/// route or the local word for it (`shield-AE`, `shield-BAB`,
/// `shield-NL-ring`) — so a style layer that asks for either name resolves.
/// WHICH networks those are is read from the published index rather than derived
/// from a shape of name: the country codes of the other networked shields
/// (`shield-AU`, `shield-CA`, `shield-DE`, `shield-JP`) are no icon's name at
/// all, so those networks resolve under their own namespaced identifier only.
const ROUTE_SHIELD_ICONS: &[&str] = &[
    "shield-AE",
    "shield-AE:E-route",
    "shield-AU:MR",
    "shield-AU:NH",
    "shield-AU:S",
    "shield-AU:T",
    "shield-BAB",
    "shield-BR",
    "shield-CN:expressway",
    "shield-CN:expressway-long",
    "shield-CR:CR",
    "shield-CR:national",
    "shield-DE:BAB",
    "shield-Emirates routes",
    "shield-HU:national",
    "shield-IN-National",
    "shield-IN-National-wide",
    "shield-IN-State",
    "shield-JP:JP",
    "shield-JP:national",
    "shield-JP:prefectural",
    "shield-KR:expressway",
    "shield-KR:expressway-long",
    "shield-KR:national",
    "shield-MX-Federal-1-2",
    "shield-MX-Federal-3-4",
    "shield-MY",
    "shield-NL",
    "shield-NL-ring",
    "shield-NL:binnenstedelijke_ring",
    "shield-PH:E",
    "shield-PK:motorway",
    "shield-PK:national",
    "shield-Qatar motorways",
    "shield-SA:national",
    "shield-TR:motorway",
    "shield-TR:motorway-long",
    "shield-TW:expressway",
    "shield-TW:freeway",
    "shield-TW:provincial",
    "shield-US-Hwy-1-2",
    "shield-US-Hwy-3",
    "shield-US-state",
    "shield-UY",
    "shield-VE:T",
    "shield-ZM-roads",
    "shield-autostrade-2-3",
    "shield-autostrade-4-5",
    "shield-blue-pentagon",
    "shield-blue-pentagon-long",
    "shield-blue-red-kite",
    "shield-blue-red-kite-3",
    "shield-ch:national",
    "shield-cy:national",
    "shield-green-hexagon",
    "shield-green-red-kite",
    "shield-green-red-kite-long",
    "shield-green-square-kite",
    "shield-kite-white",
    "shield-kite-white-long",
    "shield-mashreq",
    "shield-md:national",
    "shield-na",
    "shield-rectangle-blue",
    "shield-rectangle-blue-long",
    "shield-rectangle-green",
    "shield-rectangle-green-long",
    "shield-rectangle-jp",
    "shield-rectangle-red",
    "shield-rectangle-white",
    "shield-rectangle-white-long",
    "shield-rectangle-white-xlong",
    "shield-rectangle-yellow",
    "shield-sadc",
    "shield-white-red-square-kite",
];

/// The Trans-Canada shield, under the two names published sheets of this
/// catalog carry it: after the ROUTE it signs, and after the NETWORK it belongs
/// to, as every other networked shield is named.
const TRANSCANADA_SHIELD_BY_ROUTE: &str = "shield-CA-TransCanada";
const TRANSCANADA_SHIELD_BY_NETWORK: &str = "shield-CA:transcanada";

/// The sheet whose published index names the Trans-Canada shield after its
/// route, addressed the way `GetSprites` addresses a sheet.
const TRANSCANADA_BY_ROUTE_SHEET: (MapStyleKind, ColorSchemeKind, VariantKind) = (
    MapStyleKind::Standard,
    ColorSchemeKind::Light,
    VariantKind::Default,
);

/// The route shields the sheet addressed by `(style, scheme, variant)` carries.
///
/// The iconography is not one shared list: the published sheets of this catalog
/// do not all carry the same shield names, so the name set is read per sheet.
/// [`TRANSCANADA_BY_ROUTE_SHEET`]'s index names the Trans-Canada shield after
/// the route it signs, where the catalog's other sheets name it after its
/// network.
fn route_shield_icons(
    style: MapStyleKind,
    scheme: ColorSchemeKind,
    variant: VariantKind,
) -> Vec<&'static str> {
    let mut names = ROUTE_SHIELD_ICONS.to_vec();
    names.push(if (style, scheme, variant) == TRANSCANADA_BY_ROUTE_SHEET {
        TRANSCANADA_SHIELD_BY_ROUTE
    } else {
        TRANSCANADA_SHIELD_BY_NETWORK
    });
    names
}

/// The heavy-goods-vehicle restriction signs a published sheet carries.
const RESTRICTION_SHIELD_ICONS: &[&str] = &[
    "shield-hgv-axle_number_restriction",
    "shield-hgv-axle_number_restriction_usa",
    "shield-hgv-general_truck_restriction",
    "shield-hgv-general_truck_restriction_usa",
    "shield-hgv-hazmat_any_jpn",
    "shield-hgv-hazmat_any_usa",
    "shield-hgv-hazmat_corrosive_usa",
    "shield-hgv-hazmat_explosives_usa",
    "shield-hgv-hazmat_explosivesandflammable_usa",
    "shield-hgv-hazmat_flammable_usa",
    "shield-hgv-hazmat_flammablesolid_combustible_usa",
    "shield-hgv-hazmat_gas_usa",
    "shield-hgv-hazmat_general",
    "shield-hgv-hazmat_harmful_for_water",
    "shield-hgv-hazmat_harmfulforwater_usa",
    "shield-hgv-hazmat_organic_usa",
    "shield-hgv-hazmat_other_usa",
    "shield-hgv-hazmat_permit_required",
    "shield-hgv-hazmat_poison_inhalation_hazard_usa",
    "shield-hgv-hazmat_poison_usa",
    "shield-hgv-hazmat_radioactive_usa",
    "shield-hgv-hazmat_tunnelcategory_usa",
    "shield-hgv-hazmat_water",
    "shield-hgv-height_restriction",
    "shield-hgv-height_restriction_usa",
    "shield-hgv-length_restriction",
    "shield-hgv-length_restriction_kpra_usa",
    "shield-hgv-length_restriction_usa",
    "shield-hgv-speed_limit",
    "shield-hgv-trailer_restriction",
    "shield-hgv-trailer_restriction_usa",
    "shield-hgv-weight_per_axle_restriction",
    "shield-hgv-weight_per_axle_restriction_usa",
    "shield-hgv-weight_restriction",
    "shield-hgv-weight_restriction_usa",
    "shield-hgv-width_restriction",
    "shield-hgv-width_restriction_usa",
];

/// The font stacks this catalog has metrics for.
fn seed_fonts() -> Vec<FontRecord> {
    vec![
        font_record("Amazon Ember Regular", 24, 18, 20),
        font_record("Amazon Ember Bold", 24, 20, 20),
        font_record("Amazon Ember Condensed RC Regular", 24, 15, 20),
        font_record("Noto Sans Regular", 24, 19, 21),
    ]
}

fn font_record(name: &str, em: u32, advance: u32, ascender: i32) -> FontRecord {
    FontRecord {
        name: name.to_string(),
        em,
        // 3px is the SDF buffer the MapLibre glyph pipeline uses, so a client
        // that assumes it reads our bitmaps correctly.
        buffer: 3,
        advance,
        ascender,
        cache_control: "max-age=86400".to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGeomapsMapsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGeomapsMapsserviceState>)
}

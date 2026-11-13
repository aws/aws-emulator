// SPDX-License-Identifier: CC0-1.0

//! The bitmap a resolved metric widget is drawn into: the graph
//! `GetMetricWidgetImage` answers with.
//!
//! "The image of the graph, in the output format specified."
//! (`GetMetricWidgetImageOutput$MetricWidgetImage`) — so the bytes a caller receives are a
//! real PNG a client can decode, and every pixel of the plot comes from the series the
//! resolution read out of the store ([`super::plotted_series`]): the window, the value axis,
//! the line through each series and the legend swatch beside it are all computed from those
//! points, so a graph of an empty store and a graph of a published datapoint are different
//! images.
//!
//! The drawing itself is the maintained plotting library's — the lines, columns, markers and
//! rectangles are its elements, placed in the bitmap's own pixel coordinates — and the bitmap
//! is encoded by the maintained PNG encoder. What this module owns is the LAYOUT: where the
//! plot sits in the image, how a value and an instant become a pixel, and the theme's
//! colours.
//!
//! Both axes are LABELLED ([`lettering`]): the value axis carries the values its grid lines
//! stand at and the time axis the instants the window's grid lines fall on. That lettering is
//! what puts the series' own numbers in the image, because the plot alone is normalised to the
//! data it drew — a single point sits halfway up whatever value it took, and a series whose
//! values are all scaled by the same factor keeps its shape — so without the axes two stores
//! holding different datapoints would be the same picture. A graph too small to hold the
//! lettering beside a plot is drawn without it rather than drawn over itself.
//!
//! The legend is the colour swatch of each series: a series' label is the definition's or the
//! metric's own text rather than a number read off the graph, so it is not what the image has
//! to carry.

use ::plotters::prelude::*;

use super::{MetricWidget, PlottedSeries, View, MAX_IMAGE_SIDE};

/// The glyphs the axis labels are written in, and the bitmap they are written into.
mod lettering;

/// The margin between the image's edge and the plot, in pixels.
const PAD: i32 = 8;

/// How tall the legend strip under the plot is, in pixels.
const LEGEND_HEIGHT: i32 = 14;

/// The size of one series' legend swatch, in pixels.
const SWATCH: (i32, i32) = (12, 8);

/// The gap between two legend swatches, in pixels.
const SWATCH_GAP: i32 = 6;

/// How many grid lines divide the plot along each axis.
const GRID_DIVISIONS: i32 = 4;

/// How wide a series' line is drawn, in pixels.
const LINE_WIDTH: u32 = 2;

/// The radius of the marker drawn at a series' point, in pixels.
const MARKER_RADIUS: i32 = 2;

/// How many points of one series are still marked individually.
///
/// A marker is what makes a window's value visible where the line through it is short — a
/// series of ONE datapoint is a line of no length at all — so the markers are drawn while they
/// are distinguishable and left off a series dense enough that they would merge into the line.
const MAX_MARKED_POINTS: usize = 200;

/// The gap between the plot and the lettering beside it, in pixels.
const LABEL_GAP: i32 = 3;

/// The smallest plot an axis is lettered beside, in pixels.
///
/// A graph shorter or narrower than this has no room for both the labels and a plot worth
/// reading, so it is drawn as the plot alone.
const MIN_LETTERED_SIDE: i32 = 40;

/// How many seconds a window has to span before its instants are labelled with the day they
/// fall on as well as the time of day.
const DAY_SECS: f64 = 24.0 * 60.0 * 60.0;

/// The colours one theme draws a graph in.
#[derive(Debug, Clone, Copy)]
struct Theme {
    /// The image's background.
    background: [u8; 3],
    /// The frame around the plot.
    frame: [u8; 3],
    /// The grid inside it.
    grid: [u8; 3],
    /// A horizontal annotation's line.
    annotation: [u8; 3],
    /// The axis lettering.
    text: [u8; 3],
}

/// The light theme, which a definition naming no `theme` is drawn on.
const LIGHT: Theme = Theme {
    background: [0xff, 0xff, 0xff],
    frame: [0x87, 0x95, 0x96],
    grid: [0xe6, 0xe6, 0xe6],
    annotation: [0xd1, 0x32, 0x12],
    text: [0x54, 0x5b, 0x64],
};

/// The dark theme (`theme: dark`).
const DARK: Theme = Theme {
    background: [0x16, 0x19, 0x1f],
    frame: [0x68, 0x70, 0x78],
    grid: [0x2a, 0x2e, 0x33],
    annotation: [0xff, 0x7a, 0x59],
    text: [0xd1, 0xd5, 0xdb],
};

/// One label of an axis: the value or the instant it stands at, and the text written for it.
#[derive(Debug, Clone)]
struct AxisLabel {
    /// The value, or the instant in epoch seconds, the label stands at.
    at: f64,
    /// The text written for it.
    text: String,
}

/// Where the plot sits in the image.
#[derive(Debug, Clone, Copy)]
struct Frame {
    /// The leftmost column of the plot.
    left: i32,
    /// Its topmost row.
    top: i32,
    /// Its rightmost column.
    right: i32,
    /// Its bottommost row.
    bottom: i32,
    /// How tall the strip under the plot the window's instants are written in is, which is zero
    /// on a graph too small to letter.
    instants: i32,
    /// Whether the axes carry their labels.
    lettered: bool,
}

impl Frame {
    /// Whether the plot has an inside at all: an image too small to hold one is answered with
    /// the background alone rather than with a plot drawn outside itself.
    fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    /// The row the legend strip's swatches sit on.
    fn legend_top(&self) -> i32 {
        self.bottom + self.instants + PAD
    }
}

/// The widget's graph as PNG bytes, or the sentence saying why the bitmap could not be
/// rendered.
///
/// The bitmap is the only allocation here, and its size is CLAMPED to
/// [`super::MAX_IMAGE_SIDE`] on the line before it is made — the definition's own sizes were
/// already refused past that bound when it was parsed, so the clamp is the backstop that makes
/// the ceiling a property of this function rather than of its caller. The work the drawing
/// does is bounded the same way: the pixels by that ceiling, and the points by the resolution
/// that read them (`crate::state::metric_query`, which refuses a definition asking for more
/// datapoints than one read retrieves).
///
/// The failure channel exists because both the drawing library and the encoder are fallible
/// seams; neither can fail over an owned in-memory bitmap of a size this module already
/// bounded, so a caller reports it as the server fault it would be.
pub fn png(widget: &MetricWidget, series: &[PlottedSeries]) -> Result<Vec<u8>, String> {
    let width = widget.width.clamp(1, MAX_IMAGE_SIDE);
    let height = widget.height.clamp(1, MAX_IMAGE_SIDE);
    let mut bitmap = vec![0u8; width as usize * height as usize * 3];
    draw(widget, series, (width, height), &mut bitmap)?;
    encoded((width, height), &bitmap)
}

/// Draw the graph into `bitmap`, which is `width * height` RGB pixels.
fn draw(
    widget: &MetricWidget,
    series: &[PlottedSeries],
    (width, height): (u32, u32),
    bitmap: &mut [u8],
) -> Result<(), String> {
    let theme = if widget.dark { DARK } else { LIGHT };
    let range = value_range(widget, series);
    let values = value_labels(range);
    let instants = instant_labels(widget);
    let frame = frame_of((width, height), series, &values);

    plot(widget, series, (width, height), bitmap, theme, frame, range)?;

    // The plotting library has handed the buffer back, so the labels go straight into the
    // pixels — over the grid they annotate rather than under it.
    if frame.lettered && !frame.is_empty() {
        letter(
            widget,
            (width, height),
            bitmap,
            theme,
            frame,
            range,
            (&values, &instants),
        );
    }
    Ok(())
}

/// Where the plot sits in a `width` x `height` image: inside the margin, less the legend strip
/// when there is a series to legend, and less the room the axis labels take when the graph is
/// big enough to letter.
fn frame_of((width, height): (u32, u32), series: &[PlottedSeries], values: &[AxisLabel]) -> Frame {
    let legend = if series.is_empty() { 0 } else { LEGEND_HEIGHT };
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let (width, height) = (width as i32, height as i32);
    let right = width - 1 - PAD;
    let gutter = values
        .iter()
        .map(|label| lettering::text_width(&label.text))
        .max()
        .unwrap_or(0)
        + LABEL_GAP;
    let instants = lettering::GLYPH_HEIGHT + LABEL_GAP;
    let plain = Frame {
        left: PAD,
        top: PAD,
        right,
        bottom: height - 1 - PAD - legend,
        instants: 0,
        lettered: false,
    };
    let lettered = Frame {
        left: PAD + gutter,
        top: PAD,
        right,
        bottom: height - 1 - PAD - legend - instants,
        instants,
        lettered: true,
    };
    if lettered.right - lettered.left >= MIN_LETTERED_SIDE
        && lettered.bottom - lettered.top >= MIN_LETTERED_SIDE
    {
        lettered
    } else {
        plain
    }
}

/// Draw the background, the grid, the annotations, every series and the legend into `bitmap`.
#[allow(clippy::too_many_arguments)]
fn plot(
    widget: &MetricWidget,
    series: &[PlottedSeries],
    (width, height): (u32, u32),
    bitmap: &mut [u8],
    theme: Theme,
    frame: Frame,
    (low, high): (f64, f64),
) -> Result<(), String> {
    let root = BitMapBackend::with_buffer(bitmap, (width, height)).into_drawing_area();
    root.fill(&rgb(theme.background)).map_err(failed)?;

    let (left, top, right, bottom) = (frame.left, frame.top, frame.right, frame.bottom);
    if frame.is_empty() {
        root.present().map_err(failed)?;
        return Ok(());
    }

    for division in 1..GRID_DIVISIONS {
        let y = top + (bottom - top) * division / GRID_DIVISIONS;
        let x = left + (right - left) * division / GRID_DIVISIONS;
        root.draw(&PathElement::new(
            vec![(left, y), (right, y)],
            rgb(theme.grid),
        ))
        .map_err(failed)?;
        root.draw(&PathElement::new(
            vec![(x, top), (x, bottom)],
            rgb(theme.grid),
        ))
        .map_err(failed)?;
    }
    root.draw(&Rectangle::new(
        [(left, top), (right, bottom)],
        rgb(theme.frame),
    ))
    .map_err(failed)?;

    let x_of = |instant: f64| horizontal(instant, widget, frame);
    let y_of = |value: f64| vertical(value, (low, high), frame);

    for value in &widget.annotations {
        if *value < low || *value > high {
            continue;
        }
        let y = y_of(*value);
        root.draw(&PathElement::new(
            vec![(left, y), (right, y)],
            rgb(theme.annotation),
        ))
        .map_err(failed)?;
    }

    // How wide one column of a `bar` graph is: the plot divided by the windows the graph's
    // period lays over its range, so the columns of a range fill it whatever its granularity.
    let windows = ((widget.end - widget.start) / f64::from(widget.period.max(1))).max(1.0);
    #[allow(clippy::cast_possible_truncation)]
    let column = (f64::from(right - left) / windows).floor().max(1.0) as i32;

    for drawn in series {
        let color = rgb(drawn.color);
        let plotted: Vec<(i32, i32)> = drawn
            .points
            .iter()
            .filter(|(instant, value)| instant.is_finite() && value.is_finite())
            .map(|(instant, value)| (x_of(*instant), y_of(*value)))
            .collect();
        match widget.view {
            View::Bar => {
                for (x, y) in plotted {
                    root.draw(&Rectangle::new(
                        [(x, y), ((x + column).min(right), bottom)],
                        color.filled(),
                    ))
                    .map_err(failed)?;
                }
            }
            View::TimeSeries => {
                if plotted.len() > 1 {
                    root.draw(&PathElement::new(
                        plotted.clone(),
                        color.stroke_width(LINE_WIDTH),
                    ))
                    .map_err(failed)?;
                }
                if plotted.len() <= MAX_MARKED_POINTS {
                    for point in plotted {
                        root.draw(&Circle::new(point, MARKER_RADIUS, color.filled()))
                            .map_err(failed)?;
                    }
                }
            }
        }
    }

    // One swatch per series, left to right under the plot, in the order the definition listed
    // them; a strip that runs out of width legends as many as fit.
    let mut at = left;
    let legend_top = frame.legend_top();
    for drawn in series {
        if at + SWATCH.0 > right {
            break;
        }
        root.draw(&Rectangle::new(
            [(at, legend_top), (at + SWATCH.0, legend_top + SWATCH.1)],
            rgb(drawn.color).filled(),
        ))
        .map_err(failed)?;
        at += SWATCH.0 + SWATCH_GAP;
    }

    root.present().map_err(failed)?;
    Ok(())
}

/// Write the axis labels into `bitmap`: the values the plot's grid lines stand at, in the
/// gutter left of it, and the instants its columns fall on, in the strip under it.
///
/// A label is written only where the one before it has left room, so a plot narrow or short
/// enough that its labels would collide carries the ones that fit rather than a smear.
#[allow(clippy::too_many_arguments)]
fn letter(
    widget: &MetricWidget,
    (width, height): (u32, u32),
    bitmap: &mut [u8],
    theme: Theme,
    frame: Frame,
    range: (f64, f64),
    (values, instants): (&[AxisLabel], &[AxisLabel]),
) {
    let mut canvas = lettering::Canvas::new(bitmap, (width, height));

    // Top down, so the highest value on the axis is the one that keeps its label.
    let mut next_free_row = i32::MIN;
    for label in values.iter().rev() {
        let row = vertical(label.at, range, frame) - lettering::GLYPH_HEIGHT / 2;
        if row < next_free_row {
            continue;
        }
        let column = frame.left - LABEL_GAP - lettering::text_width(&label.text);
        canvas.write(&label.text, (column, row), theme.text);
        next_free_row = row + lettering::GLYPH_HEIGHT + LABEL_GAP;
    }

    // Left to right, so the window's own start is the instant that keeps its label.
    let mut next_free_column = i32::MIN;
    let row = frame.bottom + LABEL_GAP;
    for label in instants {
        let span = lettering::text_width(&label.text);
        let column = (horizontal(label.at, widget, frame) - span / 2).max(0);
        if column < next_free_column {
            continue;
        }
        canvas.write(&label.text, (column, row), theme.text);
        next_free_column = column + span + LABEL_GAP;
    }
}

/// The values the value axis is labelled with: the one every grid line of the plot stands at,
/// lowest first.
fn value_labels((low, high): (f64, f64)) -> Vec<AxisLabel> {
    (0..=GRID_DIVISIONS)
        .map(|division| {
            let value = low + (high - low) * f64::from(division) / f64::from(GRID_DIVISIONS);
            AxisLabel {
                at: value,
                text: formatted_value(value),
            }
        })
        .collect()
}

/// The instants the time axis is labelled with: the one every column of the plot falls on,
/// earliest first.
fn instant_labels(widget: &MetricWidget) -> Vec<AxisLabel> {
    let spans_days = widget.end - widget.start >= DAY_SECS;
    (0..=GRID_DIVISIONS)
        .map(|division| {
            let instant = widget.start
                + (widget.end - widget.start) * f64::from(division) / f64::from(GRID_DIVISIONS);
            AxisLabel {
                at: instant,
                text: formatted_instant(instant, spans_days),
            }
        })
        .collect()
}

/// `value` as the axis labels it: enough digits to tell two labels of one axis apart, and no
/// trailing zeros to spend the plot's width on.
fn formatted_value(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && (magnitude >= 1e6 || magnitude < 1e-3) {
        return format!("{value:.2e}");
    }
    let written = format!("{value:.3}");
    let trimmed = written.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `instant` as the time axis labels it: the time of day in UTC, and the day as well on a
/// window that spans more than one.
fn formatted_instant(instant: f64, spans_days: bool) -> String {
    if !instant.is_finite() {
        return String::new();
    }
    #[allow(clippy::cast_possible_truncation)]
    let Some(at) = ::chrono::DateTime::from_timestamp(instant.floor() as i64, 0) else {
        return String::new();
    };
    let layout = if spans_days { "%m-%d %H:%M" } else { "%H:%M" };
    at.format(layout).to_string()
}

/// The values the plot's vertical axis spans.
///
/// The axis covers the data the graph drew, so a reader sees the whole of every series:
/// `yAxis.left.min` and `yAxis.left.max` fix either end where the definition names one, and a
/// series that never changes — or a graph with no datapoint at all — is given a span around
/// its value rather than a zero-height axis nothing can be placed on.
fn value_range(widget: &MetricWidget, series: &[PlottedSeries]) -> (f64, f64) {
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for value in series
        .iter()
        .flat_map(|drawn| drawn.points.iter().map(|(_, value)| *value))
        .filter(|value| value.is_finite())
    {
        low = low.min(value);
        high = high.max(value);
    }
    if !low.is_finite() || !high.is_finite() {
        low = 0.0;
        high = 1.0;
    }
    let low = widget.y_min.unwrap_or(low);
    let high = widget.y_max.unwrap_or(high);
    if high - low <= f64::EPSILON * low.abs().max(1.0) {
        return (low - 1.0, high + 1.0);
    }
    (low, high)
}

/// The column of the plot `instant` falls in.
fn horizontal(instant: f64, widget: &MetricWidget, frame: Frame) -> i32 {
    scaled(
        instant,
        (widget.start, widget.end),
        (frame.left, frame.right),
    )
}

/// The row of the plot `value` sits at on an axis spanning `range`.
fn vertical(value: f64, range: (f64, f64), frame: Frame) -> i32 {
    scaled(value, range, (frame.bottom, frame.top))
}

/// `value` placed on the pixel axis `to`, by where it falls in `from`.
///
/// The result is clamped to the axis, so a value outside the range the axis spans — a point
/// under a `yAxis.left.min`, say — is drawn at its edge rather than outside the plot.
fn scaled(value: f64, from: (f64, f64), to: (i32, i32)) -> i32 {
    let span = from.1 - from.0;
    let share = if span.abs() <= f64::MIN_POSITIVE {
        0.0
    } else {
        ((value - from.0) / span).clamp(0.0, 1.0)
    };
    let (low, high) = (f64::from(to.0), f64::from(to.1));
    #[allow(clippy::cast_possible_truncation)]
    let placed = (low + (high - low) * share).round() as i32;
    placed.clamp(to.0.min(to.1), to.0.max(to.1))
}

/// A theme or series colour as the drawing library's own.
fn rgb(color: [u8; 3]) -> RGBColor {
    RGBColor(color[0], color[1], color[2])
}

/// A failure of the drawing library or the encoder, as the sentence the caller reports.
fn failed(error: impl ::std::fmt::Display) -> String {
    format!("the metric widget image could not be rendered: {error}")
}

/// `bitmap` encoded as an 8-bit RGB PNG of `width` x `height`.
fn encoded((width, height): (u32, u32), bitmap: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = ::png::Encoder::new(&mut out, width, height);
        encoder.set_color(::png::ColorType::Rgb);
        encoder.set_depth(::png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(failed)?;
        writer.write_image_data(bitmap).map_err(failed)?;
        writer.finish().map_err(failed)?;
    }
    Ok(out)
}

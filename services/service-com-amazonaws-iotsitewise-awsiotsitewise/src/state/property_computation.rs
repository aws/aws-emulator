// SPDX-License-Identifier: CC0-1.0

//! What the series of a `transform` or a `metric` property IS: the values its
//! formula computes from the properties its variables name.
//!
//! # The rule
//!
//! A property whose definition carries a formula holds no ingested data of its
//! own — it holds what its expression evaluates to over the properties its
//! variables name:
//!
//! * a `transform` is "a one-to-one mapping of a property's data points from one
//!   form to another ... by applying the transformation expression to each data
//!   point" of its input, so it has ONE data point per instant an input STREAM
//!   holds a data point at, each computed with every variable bound to the value
//!   its property carried at that instant;
//! * a `metric` "maps several values to a single value (such as a sum)" over the
//!   `window` "over which IoT SiteWise computes the metric's aggregation
//!   expression. IoT SiteWise computes one data point per `window`", so it has one
//!   data point per COMPLETED window, each computed with every variable bound to
//!   the data points its property holds inside that window.
//!
//! The window grid is the one `TumblingWindow.interval` documents — "IoT SiteWise
//! places the computed data point at the end of the interval" — see
//! [`window_grid`]; the formula language and the engine that evaluates it are in
//! [`formula_language`].
//!
//! So this is a READ: a formula property's series is DERIVED, never stored, and
//! every reader of a data stream reaches it through
//! [`ComAmazonawsIotsitewiseAwsiotsitewiseState::resolved_series`] — the latest
//! value, the history, the aggregates and the interpolation alike, so none of them
//! can disagree about what a computed property holds. Nothing is written: a
//! metric's next window simply becomes due as the clock passes its end.

use std::collections::BTreeMap;

use super::*;

use crate::ops::types::{Metric, Transform, TumblingWindow};

mod formula_language;
mod window_grid;

use formula_language::{
    bind_nothing, bind_number, bind_numbers, compile, evaluate, formula_context,
};
use window_grid::WindowGrid;

/// How deep a chain of formula properties one read follows.
///
/// DOCUMENTED, not chosen: `Metric` states "The maximum number of
/// dependent/cascading variables used in any one metric calculation is 10.
/// Therefore, a root metric can have up to 10 cascading metrics in its
/// computational dependency tree".
const MAX_CASCADE_DEPTH: usize = 10;

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The data points of the stream `key` names, oldest first: the samples
    /// ingested into it, or — for an asset property whose definition carries a
    /// formula — the values that formula computes as of `now`.
    ///
    /// The ONE reader every data-plane read of this service takes, so the latest
    /// value, the history, the aggregates and the interpolation all report the
    /// same series for one property. Borrowed for an ingested series and owned
    /// for a computed one, so reading a measurement still copies nothing.
    #[must_use]
    pub fn resolved_series(
        &self,
        key: &SeriesKey,
        now: SystemTime,
    ) -> Cow<'_, [AssetPropertyValue]> {
        if let SeriesKey::Property(asset_id, property_id) = key {
            if let Some(computed) = computed_property_series(self, asset_id, property_id, now) {
                return Cow::Owned(computed);
            }
        }
        Cow::Borrowed(self.series(key))
    }
}

/// The series the formula of `(asset_id, property_id)` computes as of `now`, or
/// `None` when that property's definition carries no formula — an `attribute` or a
/// `measurement`, whose series is the ingested one.
///
/// # Bounds
///
/// A formula property's series is derived from the stored samples of the
/// properties its variables name, each bounded at [`MAX_SAMPLES_PER_SERIES`], and
/// the derived series is bounded at the same count, so one read computes a bounded
/// number of data points however much is stored and however wide a range the
/// request that led here names. The walk over a chain of formula properties
/// computes each one ONCE (memoised per read) and stops at [`MAX_CASCADE_DEPTH`],
/// and a formula that reaches itself through its variables is cut off rather than
/// followed, so the walk terminates whatever the stored definitions say. The
/// expression each step evaluates is bounded by the model (see
/// [`formula_language::compile`]).
#[must_use]
pub fn computed_property_series(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    asset_id: &str,
    property_id: &str,
    now: SystemTime,
) -> Option<Vec<AssetPropertyValue>> {
    Evaluation::new(now).formula_series(store, asset_id, property_id, 0)
}

/// One read's evaluation of a formula property: the instant it reads the store at,
/// what it has already computed, and which properties it is computing right now.
#[derive(Debug)]
struct Evaluation {
    /// The instant this read happens at, in nanoseconds of Unix epoch time. A
    /// metric window is a data point only once it has ENDED by this instant.
    now_nanos: i128,
    /// The series already computed for a property, so a property two formulas
    /// both read is computed once per read.
    computed: HashMap<(String, String), Vec<AssetPropertyValue>>,
    /// The properties whose formulas are being evaluated right now, so a formula
    /// that reaches itself through its variables is cut off rather than followed.
    active: HashSet<(String, String)>,
}

impl Evaluation {
    /// An evaluation that reads the store as of `now`.
    fn new(now: SystemTime) -> Self {
        Self {
            now_nanos: instant_nanos(now),
            computed: HashMap::new(),
            active: HashSet::new(),
        }
    }

    /// [`computed_property_series`] at cascade depth `depth`.
    fn formula_series(
        &mut self,
        store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
        asset_id: &str,
        property_id: &str,
        depth: usize,
    ) -> Option<Vec<AssetPropertyValue>> {
        let formula = formula_of(store, asset_id, property_id)?;
        let key = (asset_id.to_string(), property_id.to_string());
        if let Some(held) = self.computed.get(&key) {
            return Some(held.clone());
        }
        if depth >= MAX_CASCADE_DEPTH || !self.active.insert(key.clone()) {
            return Some(Vec::new());
        }
        let computed = match &formula {
            Formula::Transform(transform) => {
                self.transform_series(store, asset_id, transform, depth)
            }
            Formula::Metric(metric) => self.metric_series(store, asset_id, metric, depth),
        };
        self.active.remove(&key);
        self.computed.insert(key, computed.clone());
        Some(computed)
    }

    /// One data point per instant an input stream of the transform holds one at,
    /// oldest first.
    fn transform_series(
        &mut self,
        store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
        asset_id: &str,
        transform: &Transform,
        depth: usize,
    ) -> Vec<AssetPropertyValue> {
        let Some(formula) = compile(&transform.expression) else {
            return Vec::new();
        };
        let inputs = self.inputs(store, asset_id, &transform.variables, depth);

        // The instants the transform maps: every instant an input STREAM holds a
        // data point at, each one once, oldest first. A variable naming an
        // attribute is a constant of the formula, not a stream, so it brings no
        // instant of its own.
        let mut instants: Vec<(i64, i32)> = inputs
            .iter()
            .filter(|input| !input.constant)
            .flat_map(|input| {
                input
                    .series
                    .iter()
                    .map(|sample| timestamp_key(&sample.timestamp))
            })
            .collect();
        instants.sort_unstable();
        instants.dedup();
        keep_newest(&mut instants);

        let mut computed = Vec::with_capacity(instants.len());
        for at in instants {
            let mut context = formula_context();
            // Every variable is bound to the value its property carried AT that
            // instant: a transform maps the data point that arrived together with
            // the values its other inputs hold. An input carrying no number there
            // — nothing yet, or a reading that is no number — leaves the formula
            // nothing to map, so that instant has no data point.
            let bound = inputs.iter().all(|input| {
                input
                    .number_at(at)
                    .is_some_and(|number| bind_number(&mut context, &input.name, number))
            });
            if !bound {
                continue;
            }
            if let Some(number) = evaluate(&formula, &context) {
                computed.push(computed_sample(timestamp_at_key(at), number));
            }
        }
        computed
    }

    /// One data point per COMPLETED window of the metric, oldest first.
    fn metric_series(
        &mut self,
        store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
        asset_id: &str,
        metric: &Metric,
        depth: usize,
    ) -> Vec<AssetPropertyValue> {
        let Some(formula) = metric.expression.as_deref().and_then(compile) else {
            return Vec::new();
        };
        let Some(grid) = metric
            .window
            .tumbling
            .as_ref()
            .and_then(WindowGrid::of_window)
        else {
            return Vec::new();
        };
        let variables = metric.variables.as_deref().unwrap_or_default();
        let inputs = self.inputs(store, asset_id, variables, depth);

        // Each input data point into the window it falls in, per variable. The
        // pass is over the DATA POINTS, not over the windows the stored data
        // spans, so a window no input holds a data point in costs nothing and the
        // work is bounded by what is stored. A variable naming an attribute holds
        // no data points to bucket: it is a constant of the formula, bound into
        // every window below.
        let mut windows: BTreeMap<i128, Vec<Vec<f64>>> = BTreeMap::new();
        for (at_variable, input) in inputs.iter().enumerate() {
            if input.constant {
                continue;
            }
            for sample in &input.series {
                let Some(number) = numeric_value(&sample.value) else {
                    continue;
                };
                let window = grid.window_of(timestamp_nanos(&sample.timestamp));
                let bucket = windows
                    .entry(window)
                    .or_insert_with(|| vec![Vec::new(); inputs.len()]);
                if let Some(numbers) = bucket.get_mut(at_variable) {
                    numbers.push(number);
                }
            }
        }

        // `BTreeMap` iterates in window order, so the computed series comes out
        // ascending by timestamp, as every series of this service is kept.
        let mut computed = Vec::new();
        for (window, per_variable) in windows {
            let Some(end) = grid.end_of(window) else {
                continue;
            };
            // "IoT SiteWise computes one data point per window", at the end of it:
            // a window the clock has not reached the end of yet has no data point.
            if end > self.now_nanos {
                continue;
            }
            let mut context = formula_context();
            let bound = inputs
                .iter()
                .zip(per_variable.iter())
                .all(|(input, numbers)| {
                    // A variable naming a stream is bound to the data points of
                    // THIS window, which the aggregation functions of the formula
                    // reduce; one naming an attribute is bound to the one value
                    // the asset carries for it, as the number it is, so a formula
                    // combines a constant with an aggregation of its window.
                    if !input.constant {
                        return bind_numbers(&mut context, &input.name, numbers);
                    }
                    match input.constant_number() {
                        Some(number) => bind_number(&mut context, &input.name, number),
                        None => bind_nothing(&mut context, &input.name),
                    }
                });
            if !bound {
                continue;
            }
            if let Some(number) = evaluate(&formula, &context) {
                computed.push(computed_sample(timestamp_at_nanos(end), number));
            }
        }
        keep_newest(&mut computed);
        computed
    }

    /// What each `variables` entry reads, in the order the formula declares them.
    fn inputs(
        &mut self,
        store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
        asset_id: &str,
        variables: &[ExpressionVariable],
        depth: usize,
    ) -> Vec<Input> {
        let mut inputs = Vec::with_capacity(variables.len());
        for variable in variables {
            let sources = variable_sources(store, asset_id, variable);
            // An `attribute` is a value the ASSET carries — "all assets that you
            // create from the asset model contain this attribute value. You can
            // update an attribute's value after you create an asset" — rather than
            // a stream of readings, so a variable naming one is a constant of the
            // formula at every instant and in every window.
            let constant = !sources.is_empty()
                && sources.iter().all(|(source_asset, source_property)| {
                    is_attribute(store, source_asset, source_property)
                });
            let mut series: Vec<AssetPropertyValue> = Vec::new();
            for (source_asset, source_property) in sources {
                series.extend(self.source_series(store, &source_asset, &source_property, depth));
            }
            // One variable can read several assets' streams (a variable scoped to
            // a hierarchy), so the merged series is sorted rather than assumed
            // ordered; a single source is already ascending and the sort keeps it.
            series.sort_by_key(|sample| timestamp_key(&sample.timestamp));
            inputs.push(Input {
                name: variable.name.clone(),
                series,
                constant,
            });
        }
        inputs
    }

    /// The series one source of a variable reads: the formula it computes, or the
    /// samples ingested into it.
    fn source_series(
        &mut self,
        store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
        asset_id: &str,
        property_id: &str,
        depth: usize,
    ) -> Vec<AssetPropertyValue> {
        match self.formula_series(store, asset_id, property_id, depth + 1) {
            Some(computed) => computed,
            None => store
                .series(&SeriesKey::Property(
                    asset_id.to_string(),
                    property_id.to_string(),
                ))
                .to_vec(),
        }
    }
}

/// What one variable of a formula reads.
#[derive(Debug, Clone)]
struct Input {
    /// The name the formula calls it by — `ExpressionVariable.name`, "the friendly
    /// name of the variable to be used in the expression".
    name: String,
    /// The data points of the stream(s) it reads, oldest first.
    series: Vec<AssetPropertyValue>,
    /// Whether it names an `attribute`: a value the asset carries rather than a
    /// stream of readings, so it is a CONSTANT of the formula — it brings no
    /// instant of its own to a transform and no data point to a metric's window,
    /// and binds the same value into every one of them.
    constant: bool,
}

impl Input {
    /// The number this input is bound to at `at`: the asset's value for a
    /// constant, and the newest reading at or before that instant otherwise.
    fn number_at(&self, at: (i64, i32)) -> Option<f64> {
        if self.constant {
            return self.constant_number();
        }
        // The series is ascending by timestamp, so the reading in force at `at` is
        // the one before the first reading past it.
        let past = self
            .series
            .partition_point(|sample| timestamp_key(&sample.timestamp) <= at);
        numeric_value(&self.series.get(past.checked_sub(1)?)?.value)
    }

    /// The one number a constant input is bound to: the asset's current value for
    /// it, which is the newest of the series an attribute's value lives in.
    fn constant_number(&self) -> Option<f64> {
        numeric_value(&self.series.last()?.value)
    }
}

/// Whether `(asset_id, property_id)` is an `attribute` — the definition carries
/// `PropertyType.attribute` — so a formula reading it reads a constant.
fn is_attribute(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    asset_id: &str,
    property_id: &str,
) -> bool {
    store
        .assets
        .get(asset_id)
        .and_then(|asset| store.asset_models.get(&asset.asset_model_id))
        .and_then(|model| store.asset_model_property_definition(model, property_id))
        .is_some_and(|definition| definition.r#type.attribute.is_some())
}

/// The formula a property definition carries.
#[derive(Debug, Clone)]
enum Formula {
    /// `PropertyType.transform` — one computed data point per input data point.
    Transform(Transform),
    /// `PropertyType.metric` — one computed data point per completed window.
    Metric(Metric),
}

/// The formula the definition of `(asset_id, property_id)` carries, or `None` when
/// it carries none.
///
/// Read off the asset MODEL's definition, through the one resolver every reader of
/// a property's definition takes
/// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::asset_model_property_definition`]),
/// so a property one of the model's composite models contributes carries its
/// formula exactly as a top-level one does.
fn formula_of(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    asset_id: &str,
    property_id: &str,
) -> Option<Formula> {
    let asset = store.assets.get(asset_id)?;
    let model = store.asset_models.get(&asset.asset_model_id)?;
    let property_type = store
        .asset_model_property_definition(model, property_id)?
        .r#type;
    if let Some(transform) = property_type.transform {
        return Some(Formula::Transform(transform));
    }
    property_type.metric.map(Formula::Metric)
}

/// The `(asset, property)` streams one variable of `asset_id`'s formula reads.
///
/// A variable "identifies an asset property from which to use values"
/// (`ExpressionVariable.value`): the property it names on the asset the formula
/// belongs to, or — when it also names a `hierarchyId`, "the ID of the hierarchy to
/// query for the property ID" — that property on every asset associated under that
/// hierarchy of this asset, which is the roll-up a metric over a hierarchy
/// computes.
fn variable_sources(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    asset_id: &str,
    variable: &ExpressionVariable,
) -> Vec<(String, String)> {
    let Some(label) = variable.value.property_id.as_deref() else {
        return Vec::new();
    };
    match variable.value.hierarchy_id.as_deref() {
        Some(hierarchy_label) => {
            let Some(hierarchy) = store
                .assets
                .get(asset_id)
                .and_then(|asset| asset.hierarchy(hierarchy_label))
                .and_then(|hierarchy| hierarchy.id.clone())
            else {
                return Vec::new();
            };
            store
                .hierarchy_children(asset_id, &hierarchy)
                .iter()
                .filter_map(|child| {
                    resolve_property(store, child, label).map(|property| (child.clone(), property))
                })
                .collect()
        }
        None => resolve_property(store, asset_id, label)
            .map(|property| (asset_id.to_string(), property))
            .into_iter()
            .collect(),
    }
}

/// The stored property id the variable label `label` names on asset `asset_id`.
///
/// A stored variable reference carries the identifier of the member it resolved to
/// at write time, so the raw-id form is what normally reaches here; the NAME and
/// `externalId:` spellings a write accepts are resolved too, so a reference stored
/// before the rewrite — or one naming a property by name on the same model — still
/// reaches its stream.
fn resolve_property(
    store: &ComAmazonawsIotsitewiseAwsiotsitewiseState,
    asset_id: &str,
    label: &str,
) -> Option<String> {
    if let Some(property) = store.resolve_asset_property(asset_id, label) {
        return Some(property.id);
    }
    let asset = store.assets.get(asset_id)?;
    let model = store.asset_models.get(&asset.asset_model_id)?;
    let resolved = model.resolve_property_id(label)?;
    store
        .resolve_asset_property(asset_id, &resolved)
        .map(|property| property.id)
}

/// One computed data point: the number `value` at `timestamp`.
///
/// A computed property's data type is `DOUBLE` ("A transform can only have a data
/// type of `DOUBLE`", and "a metric can only have a data type of `DOUBLE`"), so
/// the value is carried in the `Variant`'s `doubleValue`, with the quality an
/// ordinary reading carries ([`DEFAULT_QUALITY`]).
fn computed_sample(timestamp: TimeInNanos, value: f64) -> AssetPropertyValue {
    AssetPropertyValue {
        value: Variant {
            double_value: Some(value),
            ..Variant::default()
        },
        timestamp,
        quality: Some(DEFAULT_QUALITY.to_string()),
    }
}

/// A [`timestamp_key`] back as the modelled `TimeInNanos` it was read from.
fn timestamp_at_key(key: (i64, i32)) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: key.0,
        offset_in_nanos: Some(key.1),
    }
}

/// An instant as whole nanoseconds since the Unix epoch, saturating for one
/// outside the range of an `i128` of nanoseconds.
fn instant_nanos(at: SystemTime) -> i128 {
    match at.duration_since(::std::time::UNIX_EPOCH) {
        Ok(since) => i128::try_from(since.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// Drop everything but the newest [`MAX_SAMPLES_PER_SERIES`] entries of an
/// ascending list, which is the bound every series of this service holds to.
fn keep_newest<T>(series: &mut Vec<T>) {
    if series.len() > MAX_SAMPLES_PER_SERIES {
        let excess = series.len() - MAX_SAMPLES_PER_SERIES;
        series.drain(..excess);
    }
}

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Transform and metric properties never compute a value: a formula property always reports no reading

_Filed: 2026-10-24T13:53:13Z_
_Operation: GetAssetPropertyValue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: IoT SiteWise evaluates a property's formula. A `transform` property produces one computed data point for every data point ingested into the measurement(s) its expression references, and a `metric` property produces one computed data point per completed aggregation `window` over its inputs. So after `BatchPutAssetPropertyValue` writes measurements, `GetAssetPropertyValue` on the asset's transform/metric property returns the computed value, `GetAssetPropertyValueHistory` returns the computed series, and `GetAssetPropertyAggregates`/`BatchGetAssetPropertyAggregates` aggregate that computed series.

Actual: nothing in the crate ever evaluates an expression. `crate::state` stores a property definition's `type.transform` / `type.metric` (and validates their `variables` against the model's other properties) but there is no evaluator: `state.rs` only reads `property_values`, the raw ingested samples, and the only writer of `property_values` is `record_stream_value` on the `BatchPutAssetPropertyValue` path. A transform or metric property therefore has an empty series forever, so `GetAssetPropertyValue` answers 200 with no `propertyValue`, the history read answers an empty list, and both aggregate reads answer with no windows. Only measurement properties (and alias-addressed disassociated streams) ever hold data, which makes the whole computed-property half of the service unobservable even though `CreateAssetModel`/`UpdateAssetModel` accept and report the formulas.

## Evidence

Smithy model: `AssetModelProperty.type` targets `PropertyType` with `transform` (`Transform{expression, variables, processingConfig}`) and `metric` (`Metric{expression, variables, window, processingConfig}`) members; `Metric.window` is documented as the time interval over which the metric is computed, and `ExpressionVariable.value` resolves to another property of the asset — i.e. the service is specified to derive values from other properties. Well-established behaviour: transforms emit a point per input point and metrics emit a point per window, readable through `GetAssetPropertyValue`/`GetAssetPropertyValueHistory`.

Handlers: `src/ops/get_asset_property_value.rs:220-224` returns `store.series(&stream).last().cloned()` — the raw ingested series only; `src/ops/get_asset_property_value_history.rs:265-274` and `src/ops/get_asset_property_aggregates.rs:371` read the same raw series through `crate::state::aggregated_values`. `src/state.rs` has no evaluator (grep for `expression` finds only `formula_variables` at state.rs:2421, used for dependency validation), and the only writer of `property_values` is `record_stream_value`/`record_property_value` (state.rs:1618-1659) called from `batch_put_asset_property_value.rs:339`.

Tests: `tests/all/property_value_history.rs`, `property_aggregates.rs`, `single_property_aggregates.rs` and `batch_get_latest_values.rs` only write to and read back measurement properties; no test ingests a measurement and then reads the transform/metric property that depends on it, so nothing asserts the computed behaviour.

## Steps to Reproduce

1. CreateAssetModel with a measurement property `Temperature` (DOUBLE) and a metric property `AvgTemperature` whose expression averages it over a 1-minute tumbling window (plus optionally a transform property `TemperatureF` = `Temperature * 9/5 + 32`).
2. CreateAsset from that model and wait for it to report ACTIVE.
3. BatchPutAssetPropertyValue several `Temperature` TQVs inside the last minute -> 200, no errorEntries.
4. GetAssetPropertyValue for the asset's `TemperatureF` property -> 200 with NO `propertyValue`, where AWS reports the transformed value for the newest input point.
5. GetAssetPropertyValueHistory / GetAssetPropertyAggregates for `AvgTemperature` over the ingestion window -> empty, where AWS reports one computed point per completed window.

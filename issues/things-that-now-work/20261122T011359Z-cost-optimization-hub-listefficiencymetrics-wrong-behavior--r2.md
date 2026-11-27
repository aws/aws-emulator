Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a86eaa9019e1ad6a755963a94b21597ff73fa47e

# Exclusive timePeriod.end is bounded like an inclusive bound, making the present interval unreportable

_Filed: 2026-11-22T01:13:59Z_
_Operation: ListEfficiencyMetrics | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a86eaa9019e1ad6a755963a94b21597ff73fa47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`period()` judges the EXCLUSIVE `end` bound with exactly the same `> present` test it applies to the INCLUSIVE `start` bound. Because `Granularity::intervals` stops at the first interval that is not `< period.end`, the only way to ask for a series that INCLUDES the present interval is an `end` naming the NEXT interval — and that request is refused with `future_period(END, ...)` ("Time period cannot be in the future").

Consequence: a `Daily` series can never carry a data point for today, and a `Monthly` series can never carry a data point for the current month, even though no data point of such a series would lie in the future. `end` = today is accepted but its last point is yesterday; `end` = today+1 is refused; `start` = current month with `end` = current month is refused by `empty_period` ("End month must be after start month"). So no request at all can obtain the present interval.

The handler's own doc and its own interval loop treat `end` as exclusive; only the bound check in `period()` treats it as if it were inclusive, which is the inconsistency.

## Evidence

Smithy model, `TimePeriod$end` (smithy_query query="doc" shape="TimePeriod" member="end"): "The end of the time period (exclusive). Specify the date in ISO 8601 format, such as 2024-12-31." and `TimePeriod` shape doc: "Specifies a date range for retrieving efficiency metrics. The start date is inclusive and the end date is exclusive."

Handler agrees with that reading in its own doc — services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_efficiency_metrics.rs:24-26:
"//! Each group's series carries one data point per interval of the requested
//! `granularity` that the requested `timePeriod` reaches (start inclusive, end
//! exclusive)"

and in the loop that builds the series — list_efficiency_metrics.rs:483:
`        while let Some(day) = cursor.filter(|day| *day < period.end) {`
(so an `end` one interval past the present produces a last point AT the present interval, never past it).

But the bound check applies the inclusive-start test to the exclusive end — list_efficiency_metrics.rs:724-732:
```
    // Nothing has been spent yet over time that has not elapsed, so a bound
    // naming an interval later than the one the clock is on is refused rather
    // than reported as a series of nothing.
    if start > present {
        return Err(future_period(START, granularity));
    }
    if end > present {
        return Err(future_period(END, granularity));
    }
```
where `present` is `Granularity::present_bound` = "the day the clock is on for a daily series, and the month it is in for a monthly one" (list_efficiency_metrics.rs:567-570).

No test pins the boundary case: tests/all/efficiency_metrics_period_refusal.rs:244-248 deliberately avoids it — "A future bound is two intervals ahead, not one, because the clock is read here and again in the handler: a bound one interval ahead would be the present one if the UTC day rolled over between the two." — so `end` == present + 1 interval is unpinned behaviour.

## Steps to Reproduce

1. Boot the emulator with the Cost Optimization Hub service and opt the account into AWS Compute Optimizer and into Cost Optimization Hub (what tests/all/cost_optimization_hub.rs `enrolled_router()` does), so the read reaches the period check.
2. Call ListEfficiencyMetrics with {"granularity":"Daily","timePeriod":{"start":"<today-1>","end":"<today+1>"}} — a period whose exclusive end names tomorrow, i.e. a series whose last point is TODAY.
3. Observe HTTP 400 ValidationException, message "Invalid TimePeriod specified. Time period cannot be in the future.", fields entry on `timePeriod.end` (list_efficiency_metrics.rs:730-732).
4. Call the same request with {"end":"<today>"} — served (HTTP 200), but `metricsByTime` stops at <today-1>, so no request yields a point for today.
5. Repeat at Monthly: {"granularity":"Monthly","timePeriod":{"start":"<this month>","end":"<next month>"}} is refused as future, and {"start":"<this month>","end":"<this month>"} is refused by `empty_period` — so the current month has no reportable series at all.

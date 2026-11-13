Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.dashboard-widget-image-render-freshness-window
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.dashboard-widget-image-render-freshness-window|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# GetMetricWidgetImage freshness window: the widget-image render check never completes within its 60s budget

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricWidgetImage

## Description
Cause 1 of 1: GetMetricWidgetImage does not serve a stable PNG render over a fixed absolute window.
AWS: GetMetricWidgetImage returns image/png bytes (PNG magic \x89PNG\r\n\x1a\n) for the supplied MetricWidget JSON. Over a fixed absolute start/end window the render is cacheable and the same bytes come back for a repeat call, while a window that includes 'now' re-renders as new datapoints land. Recorded behavior only for the freshness rule; the status is 200 with a PNG body in both arms.
Emulator: the check publishes datapoints, asks for the image over a fixed absolute window, then repeats and compares — and never gets through its own 60s budget, so no PNG/freshness assertion is ever satisfied. The run reports only the expired budget.
Why: a client that embeds dashboard widget images (snapshot graphs in tickets, chat, status pages) gets no image, and nothing it caches can be validated.
Fix: the GetMetricWidgetImage handler and its renderer in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801 (image response path) must answer within a normal request latency and return PNG bytes that are stable for an identical absolute window.
Why this is not covered by an existing issue: 20261105T062419Z-cloudwatch-4-net-new-root-causes-in-getmetricwidgetimage.md owns five other widget-image checks but not this replay key.
Traces: cloudwatch.dashboard-widget-image-render-freshness-window.

## Evidence
Expected: GetMetricWidgetImage over a fixed absolute window returns 200 with a body starting \x89PNG\r\n\x1a\n, and an identical second call over the same absolute window returns the same bytes; actual: 'Error: Check timed out after 60.0s' — the render never completed inside the check's budget.

## Steps to Reproduce
1. PutMetricData for a custom metric over a few minutes.
2. Call GetMetricWidgetImage with a MetricWidget JSON naming that metric and an absolute start/end window covering the published points.
3. Repeat the identical call -> the check does not complete within 60s; no PNG and no byte-stability comparison is obtained.

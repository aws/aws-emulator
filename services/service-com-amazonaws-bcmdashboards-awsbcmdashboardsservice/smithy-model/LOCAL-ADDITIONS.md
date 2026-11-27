# Local additions to the Smithy model snapshot

`com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json` in this directory is
a snapshot of the published BCM Dashboards model and is kept EXACTLY as
shipped: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers `GetDashboard` for the AWS-managed dashboard catalog with widget
queries that snapshot does not declare. They are declared in this crate's own
source instead, from a small overlay model beside the snapshot
(`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no
hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-arczonalshift-percdataplane`
(`smithy-model/local-additions.json` + `src/ops/types/zonal_shift_types.rs`)
and by `services/service-com-amazonaws-workspaces-workspacesservice`.

## `budget` and `costEfficiency` on `QueryParameters`

- **What AWS does**: the published `Cost Overview and Trends` dashboard serves
  two panels whose `configs[].queryParameters` union member is neither of the
  cost-and-usage, Savings Plans or reservation queries: the budget-health panel
  carries `budget`, a query over the account's own budgets that models no time
  period, no granularity and no metrics — only an optional `filter` selecting
  which budgets to report — and the cost-efficiency panel carries
  `costEfficiency`, a query over a time range at a granularity, optionally
  grouped and filtered, whose series is a score rather than a `MetricName`.
  Both are recorded in the wire fixtures of the managed catalog's detail read.
- **What the snapshot declares**: `QueryParameters` with the five members
  `costAndUsage`, `savingsPlansCoverage`, `savingsPlansUtilization`,
  `reservationCoverage` and `reservationUtilization`, and no shape named
  `BudgetQuery` or `CostEfficiencyQuery` at all.
- **Where it is declared**: `src/ops/types/query_parameters_types.rs`, which
  holds the whole union and the two query shapes only it reaches. It is
  generated from `local-additions.json`, which restates the union's five
  modelled members — with their own targets, so each still delegates to the
  modelled query shape in `shared_types` — and adds the two AWS carries.
- **What else is in the overlay**: only the shapes those members target, and
  only as much of each as codegen reads. A shape whose Rust type is generated
  from the snapshot is restated by TYPE alone, because a structure member
  delegates to that type's own schema and nothing of its member list is read
  here; `Granularity` carries its value set and `GroupDefinitions` its element
  target, because those ARE read — the first to hold `costEfficiency`'s
  granularity to the enumerated values, the second to classify its grouping.

`tests/all/local_query_parameters.rs` pins both halves: the two members on the
wire, through a caller's own `CreateDashboard`/`GetDashboard`/`UpdateDashboard`
round trip and through the published catalog; and the overlay against the
snapshot, shape by shape (type, restated members and their targets, enum value
sets). If the snapshot is ever refreshed with either member, with a member the
overlay does not restate, or with a changed target or value set, that suite
fails — and the local declaration should then give way to the modelled shape.

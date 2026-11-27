# Local additions to the Smithy model snapshot

`com-amazonaws-budgets-awsbudgetservicegateway.json` in this directory is a
snapshot of the published AWS Budgets model and is kept EXACTLY as shipped: it is
a third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so
an edit there would be invisible to a reader of the crate's source and lost on
the next refresh — which is exactly what happened to the three members below when
the snapshot was last refreshed.

AWS answers some operations with members that snapshot does not declare. Each is
declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint traits,
no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-workspaces-workspacesservice`
(`smithy-model/local-additions.json` +
`src/ops/types/workspace_properties_types.rs`) and by
`services/service-com-amazonaws-arczonalshift-percdataplane`.

All three members are declared in `src/ops/types/budget_record_types.rs`, which
holds the three shapes every operation of this crate reads and reports a budget
through: the `Budget` of `CreateBudget`, `UpdateBudget`, `DescribeBudget` and
`DescribeBudgets`, the `Notification` of the notification operations and of
`DescribeBudgetNotificationsForAccount`, and the `Subscriber` of both the
notification and the budget-action operations.

## `Budget$CreatedAt`

- **What AWS does**: every `Budget` a read reports carries the instant the budget
  was created, on the single read and in the account-wide listing alike.
- **What the snapshot declares**: `com.amazonaws.budgets#Budget` with fifteen
  members, none of them `CreatedAt`.
- **How it is declared**: a member targeting the snapshot's own
  `GenericTimestamp`, the shape `LastUpdatedTime` beside it targets, so no
  timestamp shape is invented for it.
- **Where the value comes from**: the write that establishes a budget stamps it
  and an update leaves it alone (`crate::state::stamp_service_owned`), so a
  request cannot set it.

## `Notification$ConsecutiveBreachCount`

- **What AWS does**: every notification a read reports carries how many budget
  periods in a row the spend it watches has passed its threshold.
- **What the snapshot declares**: `com.amazonaws.budgets#Notification` with five
  members, none of them `ConsecutiveBreachCount`, and no shape of that name.
- **How it is declared**: an open `integer`, because nothing publishes a range
  for it.
- **Where the value comes from**: counted off the budget's own figures on every
  read (`crate::state::consecutive_breach_count`), like the `NotificationState`
  beside it; the write path drops whatever a request carried
  (`crate::state::stamp_notification_owned`).

## `Subscriber$VerificationStatus`

- **What AWS does**: every subscriber of a budget ACTION is reported with the
  verification state of its address; the reads of a NOTIFICATION's subscribers
  carry no such member.
- **What the snapshot declares**: `com.amazonaws.budgets#Subscriber` with its two
  caller-owned members only, and no shape of that name.
- **How it is declared**: an open `string`, because nothing publishes the value
  set.
- **Where the value comes from**: the action write paths stamp it
  (`crate::state::action_subscribers`); an address is confirmed out of band, by
  whoever holds it, so a request cannot assert it
  (`crate::state::drop_subscriber_derived`).

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen
reads — the enums carry their values (that is what validation enforces), the
strings their length and pattern, the double its range and default, the maps and
lists their key, value and member targets, the timestamp its type. A STRUCTURE
member is resolved through the Rust field's type rather than through the model, so
the overlay declares those shapes (`Spend`, `CostTypes`, `TimePeriod`,
`CalculatedSpend`, `AutoAdjustData`, `Expression`, `HealthStatus`) without
restating their members; the modelled types in `shared_types.rs` are what those
members read and write.

`tests/all/recorded_response_members.rs` pins every half of this: each member on
the wire, over tables whose rows span what the rule generalises over; that a
request cannot set any of the three; and the overlay against the snapshot, shape
by shape (type, members, enum values and constraint traits, documentation aside).
If the snapshot is ever refreshed with one of these members, with a member the
overlay does not restate, or with a changed target, value set or constraint, that
suite fails — and the local declaration should then give way to the modelled
shape.

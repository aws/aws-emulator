// SPDX-License-Identifier: CC0-1.0

//! Where this service's `@timestampFormat("date-time")` members get their wire form.
//!
//! Every timestamp member of this model declares the `date-time` form — a subscription's
//! `createdAt`/`updatedAt` on both `Subscription` and `SubscriptionSummary`, and a pending
//! change's `effectiveDate` — so each one travels as an RFC 3339 string. None of the
//! structures below this module carry that format on their own member schemas: the
//! vendored codegen emits no `with_timestamp_format`, and the correction lives ONCE for
//! the whole workspace in `service_common::date_time_json`, which `src/service.rs` returns
//! from `Service::override_protocol`.
//!
//! This module holds no code on purpose. It exists so the question "why does a `date-time`
//! member of this service not need a hand-written member schema?" is answered next to the
//! types that raise it, instead of being answered a second time by another copy of the
//! rule.

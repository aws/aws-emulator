// SPDX-License-Identifier: CC0-1.0

//! The clock this service's clock-owed transitions are decided against.
//!
//! Not a clock of its own: the emulator holds ONE, shared by every service it serves (see
//! [`::service_common::clock`]), so the due instants this store records and the comparisons
//! that materialise them are measured against the same time as every other service's. The
//! type is re-exported here so a reader of this store finds it beside the records it dates.

pub use ::service_common::clock::Clock;

// SPDX-License-Identifier: CC0-1.0

//! The SYNTAX of a mapping template, which is the Velocity Template Language's and not this
//! service's: it is read by the tree's one reader of that language
//! ([`::service_common::velocity_template`]), so the template this service evaluates and the
//! mapping template API Gateway renders are taken apart by the same grammar.
//!
//! What stays here is everything ABOUT THIS SERVICE: the `$ctx` surface a reference resolves
//! against, the `$util` helpers it may call, and the budgets one evaluation runs under (the
//! parent module), because those are AppSync's own and not the language's.

pub use ::service_common::velocity_template::{
    parse, parse_with_diagnosis, parse_with_position, Access, Expected, Expr, Node, Operator,
    Parse, ParseFailure, Reference, MAX_NESTING,
};

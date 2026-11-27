// SPDX-License-Identifier: CC0-1.0

//! The structures a deployment configuration is built from, with the members Amazon ECS
//! DEFAULTS when a request leaves them out.
//!
//! These are the ONE `DeploymentCircuitBreaker`, `ThresholdConfiguration`,
//! `DeploymentAlarms` and `DeploymentEarlySuccessCriteria` types of this crate — the shapes
//! `CreateService` and `UpdateService` parse a `DeploymentConfiguration` through and every
//! service read reports it through.
//!
//! They are declared here rather than beside the other shared shapes because the snapshot
//! marks members that carry a `@default` `@required` as well, and the derive refuses a
//! request that omits a `@required` member whatever default it declares. Amazon ECS fills
//! the default in instead: a request that names `enable` and nothing else is served, and the
//! structure reads back whole. The snapshot is kept as shipped, so the relaxed declaration
//! lives in the small overlay `smithy-model/local-additions.json` (see
//! `smithy-model/LOCAL-ADDITIONS.md`), which restates each member with its own target and
//! its `@default`, and leaves `@required` on the members the snapshot gives no default —
//! `ThresholdConfiguration$type` and `DeploymentAlarms$alarmNames` — so a request that omits
//! one of those is still refused.
//!
//! Nothing about the wire is hand-written: the member names, the shape ids and the traits
//! all come from that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#DeploymentCircuitBreaker")]
#[derive(Debug, Clone, Default)]
pub struct DeploymentCircuitBreaker {
    pub enable: bool,
    pub rollback: bool,
    pub reset_on_healthy_task: Option<bool>,
    pub threshold_configuration: Option<ThresholdConfiguration>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#ThresholdConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct ThresholdConfiguration {
    pub r#type: String,
    pub value: i32,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#DeploymentAlarms")]
#[derive(Debug, Clone, Default)]
pub struct DeploymentAlarms {
    pub alarm_names: Vec<String>,
    pub rollback: bool,
    pub enable: bool,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#DeploymentEarlySuccessCriteria")]
#[derive(Debug, Clone, Default)]
pub struct DeploymentEarlySuccessCriteria {
    pub enable: bool,
    pub healthy_percent: Option<i32>,
    pub source_service_revision_cleanup: Option<String>,
}

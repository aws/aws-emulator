// SPDX-License-Identifier: CC0-1.0

//! The gateway inventory this emulator publishes.
//!
//! # Why an inventory is published rather than created
//!
//! The Global Inference Gateway Runtime API binds exactly two operations
//! (`smithy_query(query="digest")`), both invocations: there is no `CreateGateway` here, so
//! a gateway is not a resource a caller of THIS service can bring into being. The
//! inventory is therefore published, the way a catalogue of base models a caller cannot
//! create is published by the inference data plane beside it, and it is what makes
//! `ResourceNotFoundException`, `ServiceUnavailableException` and a served completion a
//! decision taken against state rather than a guess.
//!
//! Each id below satisfies the model's own `GatewayId` constraints — `@length(1, 64)` and
//! `@pattern("^[a-z0-9]+$")`, which admit no hyphen or underscore
//! (`smithy_query(query="shape", shape="GatewayId")`). The model names a gateway's targets
//! serve are the names a request's own top-level `model` member carries, which the model
//! constrains in no way: they are the OpenAI-shaped request's model names.
//!
//! Every published target has capacity. Capacity is a condition of the fleet rather than
//! of the API, so the state that answers 503 is published available and changed through
//! [`super::ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState::set_target_capacity`].

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::Gateway;

/// The gateways this emulator publishes, each with the models its targets serve.
///
/// Two of them, with DIFFERENT target sets, so routing a model is decided per gateway: a
/// model one gateway serves is not thereby served by the other.
pub const PUBLISHED_GATEWAYS: &[(&str, &[&str])] = &[
    (
        "default",
        &["gpt-oss-120b", "gpt-oss-20b", "llama-3.1-8b-instruct"],
    ),
    ("secondary", &["qwen3-32b"]),
];

/// The published inventory as the registry holds it, in `GatewayId` order.
#[must_use]
pub fn published(region: &str, now: SystemTime) -> BTreeMap<String, Gateway> {
    PUBLISHED_GATEWAYS
        .iter()
        .map(|(gateway_id, models)| {
            let targets: Vec<(&str, bool)> = models.iter().map(|model| (*model, true)).collect();
            (
                (*gateway_id).to_string(),
                Gateway::new(gateway_id, region, &targets, now),
            )
        })
        .collect()
}

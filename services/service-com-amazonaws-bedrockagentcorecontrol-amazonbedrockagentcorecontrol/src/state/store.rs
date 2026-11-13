// SPDX-License-Identifier: CC0-1.0

//! The account's STORES: every lookup and mutation the handlers drive
//! [`ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState`]
//! through, in one place beside the records it holds.
//!
//! Moved here from the module root verbatim, so the behaviour is the root's own;
//! the root re-exports this module, and a handler still names every member
//! through the state struct it is an inherent method of.

use super::*;

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint an `agentRuntimeId` for `name` that no stored runtime holds:
    /// `name` + `-` + 10 alphanumerics, per `AgentRuntimeId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_runtime_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.agent_runtimes.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The (runtime, version) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the runtime still
    /// exists.
    pub fn replayed_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, u32)> {
        let (id, version) = self.idempotency.get(&token_key(operation, client_token?))?;
        let record = self.agent_runtimes.get(id)?;
        record
            .versions
            .contains_key(version)
            .then(|| (id.clone(), *version))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        agent_runtime_id: &str,
        version: u32,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.idempotency,
            &mut self.idempotency_order,
            token_key(operation, &token),
            (agent_runtime_id.to_string(), version),
        );
    }

    /// Drop the idempotency records naming a runtime that no longer exists.
    ///
    /// Called from the delete path: such a record can never replay anything
    /// again, and nothing else would ever reclaim it. The endpoint records
    /// naming it go with them: the endpoints were held on the runtime, so they
    /// are gone too.
    pub fn forget_client_tokens_for(&mut self, agent_runtime_id: &str) {
        self.idempotency.retain(|_, (id, _)| id != agent_runtime_id);
        let live = &self.idempotency;
        self.idempotency_order.retain(|token| live.contains_key(token));
        self.runtime_endpoint_idempotency
            .retain(|_, (id, _)| id != agent_runtime_id);
        let live = &self.runtime_endpoint_idempotency;
        self.runtime_endpoint_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint an `agentRuntimeEndpointId` for `endpoint_name` that no endpoint of
    /// `agent_runtime_id` holds: the endpoint's name + `-` + 10 alphanumerics,
    /// per `AgentRuntimeEndpointId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_runtime_endpoint_id(
        &mut self,
        agent_runtime_id: &str,
        endpoint_name: &str,
        at: SystemTime,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!(
                "{endpoint_name}-{}",
                suffix_for(
                    &format!("{agent_runtime_id}/{endpoint_name}"),
                    self.mint_counter,
                    at
                )
            );
            let taken = self.agent_runtimes.get(agent_runtime_id).is_some_and(|runtime| {
                runtime
                    .endpoints
                    .values()
                    .any(|endpoint| endpoint.endpoint_id == candidate)
            });
            if !taken {
                return candidate;
            }
        }
    }

    /// The (runtime, endpoint name) a previous request to `operation` carrying
    /// this `clientToken` produced, if the record is still held and the endpoint
    /// still exists.
    pub fn replayed_runtime_endpoint_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, String)> {
        let (agent_runtime_id, endpoint_name) = self
            .runtime_endpoint_idempotency
            .get(&token_key(operation, client_token?))?;
        self.agent_runtimes
            .get(agent_runtime_id)?
            .endpoints
            .contains_key(endpoint_name)
            .then(|| (agent_runtime_id.clone(), endpoint_name.clone()))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_runtime_endpoint_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        agent_runtime_id: &str,
        endpoint_name: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.runtime_endpoint_idempotency,
            &mut self.runtime_endpoint_idempotency_order,
            token_key(operation, &token),
            (agent_runtime_id.to_string(), endpoint_name.to_string()),
        );
    }

    /// Mint a `gatewayId` for `name` that no stored gateway holds: the name,
    /// lower-cased, + `-` + 10 lower-case alphanumerics, per `GatewayId`'s
    /// pattern `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`.
    ///
    /// Two things the pattern requires and `GatewayName`
    /// (`^([0-9a-zA-Z][-]?){1,48}$`) does not: the id is lower case, and no two
    /// dashes are adjacent — so a name ENDING in a dash has it trimmed before the
    /// suffix's own dash is appended, rather than minting an id outside the
    /// pattern a client validates the response against.
    pub fn mint_gateway_id(&mut self, name: &str, at: SystemTime) -> String {
        let lowercased = name.to_ascii_lowercase();
        let stem = lowercased.trim_end_matches('-');
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{stem}-{}", suffix_for(stem, self.mint_counter, at));
            if !self.gateways.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `targetId` no target of `gateway_id` holds: 10 alphanumerics, per
    /// `TargetId`'s pattern `^[0-9a-zA-Z]{10}$`, which \u2014 unlike a gateway id \u2014
    /// carries no name.
    pub fn mint_target_id(&mut self, gateway_id: &str, name: &str, at: SystemTime) -> String {
        self.mint_gateway_child_id(gateway_id, name, at, suffix_for, |gateway, candidate| {
            gateway.targets.contains_key(candidate)
        })
    }

    /// Mint an id for a CHILD of the gateway `gateway_id` — a target, a rate
    /// limit, a rule — that the gateway does not already hold one under.
    ///
    /// The shape every gateway-child mint shares, in one place so they cannot
    /// drift: the counter is bumped on every attempt, so two children minted for
    /// one gateway in the same instant are digested from different inputs and
    /// cannot share an id; `derive` is the id form that child's own pattern takes,
    /// applied over `<gatewayId>/<seed>`; and `held` says whether the gateway
    /// already holds that candidate.
    fn mint_gateway_child_id(
        &mut self,
        gateway_id: &str,
        seed: &str,
        at: SystemTime,
        derive: impl Fn(&str, u64, SystemTime) -> String,
        held: impl Fn(&GatewayRecord, &str) -> bool,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = derive(&format!("{gateway_id}/{seed}"), self.mint_counter, at);
            let taken = self
                .gateways
                .get(gateway_id)
                .is_some_and(|gateway| held(gateway, &candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// The (gateway, target) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and what it names
    /// still exists.
    pub fn replayed_gateway_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, Option<String>)> {
        let (gateway_id, target_id) = self
            .gateway_idempotency
            .get(&token_key(operation, client_token?))?;
        let gateway = self.gateways.get(gateway_id)?;
        match target_id {
            None => Some((gateway_id.clone(), None)),
            Some(target_id) => gateway
                .targets
                .contains_key(target_id)
                .then(|| (gateway_id.clone(), Some(target_id.clone()))),
        }
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_gateway_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        gateway_id: &str,
        target_id: Option<&str>,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.gateway_idempotency,
            &mut self.gateway_idempotency_order,
            token_key(operation, &token),
            (gateway_id.to_string(), target_id.map(str::to_string)),
        );
    }

    /// Drop the idempotency records naming a gateway that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does.
    pub fn forget_gateway_client_tokens_for(&mut self, gateway_id: &str) {
        self.gateway_idempotency.retain(|_, (id, _)| id != gateway_id);
        let live = &self.gateway_idempotency;
        self.gateway_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Every stored agent-runtime VERSION associated with the capacity provider
    /// `capacity_provider_id`, each with the opaque page cursor that identifies
    /// it, in one deterministic order.
    ///
    /// The association is the one this service persists: a version carries a
    /// `capacityProviderConfiguration` naming the provider's ARN
    /// ([`AgentRuntimeVersionRecord::capacity_provider_id`]), which is what makes
    /// that version one "you must disassociate before you can delete the
    /// capacity provider" (`smithy_query doc
    /// shape=ListAgentRuntimeVersionsByCapacityProvider`).
    ///
    /// The cursor is `<agentRuntimeId>/<version zero-padded to five digits>` —
    /// five because `AgentRuntimeVersion` is `^([1-9][0-9]{0,4})$` — so the
    /// cursors sort exactly as the rows do and a page can resume strictly after
    /// the one it ended on.
    pub fn runtime_versions_for_capacity_provider(
        &self,
        capacity_provider_id: &str,
        now: SystemTime,
    ) -> Vec<(String, AgentRuntimeVersionSummary)> {
        let mut rows = Vec::new();
        for record in self.agent_runtimes.values() {
            for version in record.versions.values() {
                if version.capacity_provider_id() != Some(capacity_provider_id) {
                    continue;
                }
                rows.push((
                    format!("{}/{:05}", record.agent_runtime_id, version.version),
                    version.summary(&record.agent_runtime_arn, now),
                ));
            }
        }
        // The maps are ordered, but a runtime id and a version number are two
        // keys: sorting on the composed cursor is what makes the one order the
        // pager walks and the cursors it hands out agree.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));
        rows
    }

    /// Whether this account holds a capacity provider that
    /// `capacity_provider_id` names.
    ///
    /// The only record of a capacity provider this service persists today is the
    /// association an agent-runtime version carries: `CreateCapacityProvider` is
    /// still scaffolded (see its own `src/ops/create_capacity_provider.rs`), so
    /// an id no stored version is configured with names nothing in this account.
    ///
    /// KNOWN DEVIATION, which that stub owns and this read resolves the day it
    /// lands: a capacity provider that exists with NO associated runtime version
    /// is answered `AccessDeniedException` (403) here, where the real service
    /// answers 200 with an empty `agentRuntimes` list — the listing is documented
    /// as naming "the runtimes you must disassociate before you can delete the
    /// capacity provider", so an empty page is a legitimate answer for a provider
    /// nothing uses. Nothing in this emulator can mint or hold such a provider
    /// yet, so there is no state from which to tell it apart from an id that
    /// names nothing; when `CreateCapacityProvider` is implemented, its store
    /// becomes the authority this reads and the empty-provider case answers 200.
    pub fn holds_capacity_provider(&self, capacity_provider_id: &str) -> bool {
        self.agent_runtimes.values().any(|record| {
            record
                .versions
                .values()
                .any(|version| version.capacity_provider_id() == Some(capacity_provider_id))
        })
    }

    /// The gateway whose PUBLISHED endpoint host is the one a data-plane request
    /// addressed: the gateway `gateway_id` names, and only while the URL it
    /// published is the one on `region`'s gateway host.
    ///
    /// The Region is part of the address, so a gateway created in one Region is
    /// not reachable through another Region's host — the gateway store is keyed
    /// by id alone, and the published URL is what ties a record to a host
    /// ([`gateway_url`]).
    pub fn gateway_on_endpoint_host(
        &self,
        region: &str,
        gateway_id: &str,
    ) -> Option<&GatewayRecord> {
        let record = self.gateways.get(gateway_id)?;
        let published = gateway_url(region, gateway_id, record.protocol_type.as_deref());
        (record.gateway_url == published).then_some(record)
    }

    /// Remove every gateway whose deletion the clock has already made due, with
    /// the name it reserved, the targets on it and the idempotency records naming
    /// it — and, on the gateways that remain, every TARGET whose own deletion the
    /// clock has made due.
    ///
    /// `DeleteGateway` and `DeleteGatewayTarget` ACCEPT the deletion (202) and
    /// report the resource `DELETING`; the record itself goes when the span it
    /// owes passes, so a caller polling it sees `DELETING` and then the modelled
    /// 404. The removal is owed by the CLOCK — it was due at this instant whether
    /// or not anybody called — so every operation that reads the gateway store
    /// materialises it first, and nothing ever sleeps for it.
    pub fn expire_deleted_gateways_and_targets(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .gateways
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.gateways.remove(&id) {
                self.gateway_names.remove(&record.name);
                self.forget_gateway_client_tokens_for(&record.gateway_id);
            }
        }
        for gateway in self.gateways.values_mut() {
            gateway
                .targets
                .retain(|_, target| target.removal_due_at.is_none_or(|at| now < at));
        }
    }

    /// Mint a `harnessId` for `name` that no stored harness holds: the name + `-`
    /// + 10 alphanumerics, per `HarnessId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,39}-[a-zA-Z0-9]{10}$`.
    ///
    /// `HarnessName` is `^[a-zA-Z][a-zA-Z0-9_]{0,39}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping — unlike a
    /// gateway id, whose pattern is narrower than its name's.
    pub fn mint_harness_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.harnesses.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `memoryId` for `name` that no stored memory holds: `name` + `-` +
    /// 10 alphanumerics, per `MemoryId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$` (min length 12, which
    /// the 11 characters the suffix contributes already satisfy for any `Name`).
    pub fn mint_memory_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.memories.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `strategyId` none of `minted` holds, for a strategy named
    /// `strategy_name`. `MemoryStrategyId` carries the same pattern as
    /// `MemoryId`, so the id has the same shape; `minted` is the ids already
    /// minted for the memory being created, which the store does not hold yet.
    pub fn mint_memory_strategy_id(
        &mut self,
        strategy_name: &str,
        at: SystemTime,
        minted: &BTreeSet<String>,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!(
                "{strategy_name}-{}",
                suffix_for(strategy_name, self.mint_counter, at)
            );
            if !minted.contains(&candidate) {
                return candidate;
            }
        }
    }

    /// The harness a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and that harness still exists.
    pub fn replayed_harness_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let harness_id = self
            .harness_idempotency
            .get(&token_key(operation, client_token?))?;
        self.harnesses
            .contains_key(harness_id)
            .then(|| harness_id.clone())
    }

    /// The memory a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the memory still exists.
    pub fn replayed_memory_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let memory_id = self.memory_idempotency.get(&token_key(operation, client_token?))?;
        self.memories
            .contains_key(memory_id)
            .then(|| memory_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_harness_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        harness_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.harness_idempotency,
            &mut self.harness_idempotency_order,
            token_key(operation, &token),
            harness_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a harness that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does. The endpoint
    /// records naming it go with them: the endpoints were removed with the
    /// harness, so neither record can ever replay anything again.
    pub fn forget_harness_client_tokens_for(&mut self, harness_id: &str) {
        self.harness_idempotency.retain(|_, id| id != harness_id);
        let live = &self.harness_idempotency;
        self.harness_idempotency_order
            .retain(|token| live.contains_key(token));
        self.harness_endpoint_idempotency
            .retain(|_, (id, _)| id != harness_id);
        let live = &self.harness_endpoint_idempotency;
        self.harness_endpoint_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// The (harness, endpointName) a previous request to `operation` carrying
    /// this `clientToken` produced, if the record is still held and the endpoint
    /// it names still exists.
    pub fn replayed_harness_endpoint_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, String)> {
        let (harness_id, endpoint_name) = self
            .harness_endpoint_idempotency
            .get(&token_key(operation, client_token?))?;
        self.harnesses
            .get(harness_id)?
            .endpoints
            .contains_key(endpoint_name)
            .then(|| (harness_id.clone(), endpoint_name.clone()))
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_harness_endpoint_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        harness_id: &str,
        endpoint_name: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.harness_endpoint_idempotency,
            &mut self.harness_endpoint_idempotency_order,
            token_key(operation, &token),
            (harness_id.to_string(), endpoint_name.to_string()),
        );
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_memory_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        memory_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.memory_idempotency,
            &mut self.memory_idempotency_order,
            token_key(operation, &token),
            memory_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a memory that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does: the memory the
    /// record names is gone, so it can never replay anything again.
    pub fn forget_memory_client_tokens_for(&mut self, memory_id: &str) {
        self.memory_idempotency.retain(|_, id| id != memory_id);
        let live = &self.memory_idempotency;
        self.memory_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove every memory whose accepted deletion the clock has made due, and
    /// the name index entry and idempotency records naming it.
    ///
    /// `DeleteMemory` ACCEPTS the deletion (202) and reports the memory
    /// `DELETING`; the record itself goes when the span it owes passes, so a
    /// caller polling the memory sees `DELETING` and then the modelled 404. The
    /// removal is owed by the CLOCK — it was due at this instant whether or not
    /// anybody called — so every operation that reads the memory store
    /// materialises it first, and nothing ever sleeps for it.
    pub fn expire_deleted_memories(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .memories
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.memories.remove(&id) {
                self.memory_names.remove(&record.name);
                self.forget_memory_client_tokens_for(&record.id);
            }
        }
    }

    /// Mint a `registryId` no stored registry holds: [`REGISTRY_ID_LEN`]
    /// alphanumerics, per `RegistryId`'s pattern `^[a-zA-Z0-9]{12,16}$`, which
    /// — unlike a runtime, gateway or memory id — carries no name. The name is
    /// still part of what the id is derived from, so two registries created in
    /// the same instant under different names cannot collide.
    pub fn mint_registry_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = suffix_of_length(name, self.mint_counter, at, REGISTRY_ID_LEN);
            if !self.registries.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The registry a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the registry still exists.
    pub fn replayed_registry_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let registry_id = self
            .registry_idempotency
            .get(&token_key(operation, client_token?))?;
        self.registries
            .contains_key(registry_id)
            .then(|| registry_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_registry_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        registry_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.registry_idempotency,
            &mut self.registry_idempotency_order,
            token_key(operation, &token),
            registry_id.to_string(),
        );
    }

    /// Mint a `browserId` for `name` that no stored browser holds: the name +
    /// `-` + 10 alphanumerics, per `BrowserId`'s pattern
    /// `^(aws\.browser\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`.
    ///
    /// `SandboxName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping. The other
    /// alternative, `aws.browser.v1`, is the service-provided browser tool,
    /// which no create mints.
    pub fn mint_browser_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.browsers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The browser a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the browser still exists.
    pub fn replayed_browser_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let browser_id = self
            .browser_idempotency
            .get(&token_key(operation, client_token?))?;
        self.browsers
            .contains_key(browser_id)
            .then(|| browser_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_browser_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        browser_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.browser_idempotency,
            &mut self.browser_idempotency_order,
            token_key(operation, &token),
            browser_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a browser that no longer exists, for
    /// the same reason [`Self::forget_client_tokens_for`] does.
    pub(super) fn forget_browser_client_tokens_for(&mut self, browser_id: &str) {
        self.browser_idempotency.retain(|_, id| id != browser_id);
        let live = &self.browser_idempotency;
        self.browser_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Mint a `profileId` for `name` that no stored browser profile holds: the
    /// name + `-` + 10 alphanumerics, per `BrowserProfileId`'s pattern
    /// `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`.
    ///
    /// `BrowserProfileName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly
    /// the stem that pattern admits, so the name needs no reshaping.
    pub fn mint_browser_profile_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.browser_profiles.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The browser profile a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the profile still
    /// exists.
    pub fn replayed_browser_profile_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let profile_id = self
            .browser_profile_idempotency
            .get(&token_key(operation, client_token?))?;
        self.browser_profiles
            .contains_key(profile_id)
            .then(|| profile_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, for the same
    /// reason [`Self::remember_browser_client_token`] does.
    pub fn remember_browser_profile_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        profile_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.browser_profile_idempotency,
            &mut self.browser_profile_idempotency_order,
            token_key(operation, &token),
            profile_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a browser profile that no longer
    /// exists, for the same reason [`Self::forget_client_tokens_for`] does.
    pub(super) fn forget_browser_profile_client_tokens_for(&mut self, profile_id: &str) {
        self.browser_profile_idempotency
            .retain(|_, id| id != profile_id);
        let live = &self.browser_profile_idempotency;
        self.browser_profile_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove every browser profile whose accepted deletion the clock has made
    /// due, and the name index entry and idempotency records naming it:
    /// `DeleteBrowserProfile` reports the profile `DELETING`, and the record
    /// itself goes when the span it owes passes, so a caller polling the profile
    /// sees `DELETING` and then the modelled 404. The removal is owed by the
    /// CLOCK — it was due at this instant whether or not anybody called — so
    /// every operation that reads the profile store materialises it first, and
    /// nothing ever sleeps for it.
    pub fn expire_deleted_browser_profiles(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .browser_profiles
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.browser_profiles.remove(&id) {
                self.browser_profile_names.remove(&record.name);
                self.forget_browser_profile_client_tokens_for(&record.profile_id);
            }
        }
    }

    /// Mint a `codeInterpreterId` for `name` that no stored code interpreter
    /// holds: the name + `-` + 10 alphanumerics, per `CodeInterpreterId`'s
    /// pattern
    /// `^(aws\.codeinterpreter\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`.
    ///
    /// `SandboxName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which is exactly the
    /// stem that pattern admits, so the name needs no reshaping. The other
    /// alternative, `aws.codeinterpreter.v1`, is the service-provided code
    /// interpreter tool, which no create mints.
    pub fn mint_code_interpreter_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.code_interpreters.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The code interpreter a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the interpreter
    /// still exists.
    pub fn replayed_code_interpreter_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let code_interpreter_id = self
            .code_interpreter_idempotency
            .get(&token_key(operation, client_token?))?;
        self.code_interpreters
            .contains_key(code_interpreter_id)
            .then(|| code_interpreter_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, for the same
    /// reason [`Self::remember_browser_client_token`] does.
    pub fn remember_code_interpreter_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        code_interpreter_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.code_interpreter_idempotency,
            &mut self.code_interpreter_idempotency_order,
            token_key(operation, &token),
            code_interpreter_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a code interpreter that no longer
    /// exists, for the same reason [`Self::forget_client_tokens_for`] does.
    pub(super) fn forget_code_interpreter_client_tokens_for(&mut self, code_interpreter_id: &str) {
        self.code_interpreter_idempotency
            .retain(|_, id| id != code_interpreter_id);
        let live = &self.code_interpreter_idempotency;
        self.code_interpreter_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove every code interpreter whose accepted deletion the clock has made
    /// due, and the name index entry and idempotency records naming it:
    /// `DeleteCodeInterpreter` ACCEPTS the deletion (202) and reports the
    /// interpreter `DELETING`, and the record itself goes when the span it owes
    /// passes, so a caller polling it sees `DELETING` and then the modelled 404.
    pub fn expire_deleted_code_interpreters(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .code_interpreters
            .iter()
            .filter(|(_, record)| record.removal_due_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(record) = self.code_interpreters.remove(&id) {
                self.code_interpreter_names.remove(&record.name);
                self.forget_code_interpreter_client_tokens_for(&record.code_interpreter_id);
            }
        }
    }

    /// Remove the browser `browser_id` names, with the name index entry and the
    /// idempotency records naming it, and hand the record back.
    ///
    /// A browser's teardown completes inside the `DeleteBrowser` call, so the
    /// removal is this call's own and not a span the clock owes: the delete
    /// answers `DELETED` and every later read of the id is the modelled 404. The
    /// name becomes this account's to use again, and an idempotency record naming
    /// a browser that is gone can never replay anything, so nothing else would
    /// reclaim it.
    pub fn remove_browser(&mut self, browser_id: &str) -> Option<BrowserRecord> {
        let record = self.browsers.remove(browser_id)?;
        self.browser_names.remove(&record.name);
        self.forget_browser_client_tokens_for(&record.browser_id);
        Some(record)
    }

    /// Mint a `bundleId` for `bundle_name` that no stored bundle holds:
    /// `bundle_name` + `-` + 10 alphanumerics, per `ConfigurationBundleId`'s
    /// pattern `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
    pub fn mint_configuration_bundle_id(&mut self, bundle_name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{bundle_name}-{}", suffix_for(bundle_name, self.mint_counter, at));
            if !self.configuration_bundles.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `versionId` for a commit of `bundle_name`: the UUID form
    /// `ConfigurationBundleVersion` takes
    /// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
    ///
    /// The counter is bumped first, so two commits of one bundle in the same
    /// instant are digested from different inputs and cannot share an id.
    pub fn mint_configuration_bundle_version_id(
        &mut self,
        bundle_name: &str,
        at: SystemTime,
    ) -> String {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        uuid_shaped(bundle_name, self.mint_counter, at)
    }

    /// The configuration bundle a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the bundle still
    /// exists.
    pub fn replayed_configuration_bundle_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let bundle_id = self
            .configuration_bundle_idempotency
            .get(&token_key(operation, client_token?))?;
        self.configuration_bundles
            .contains_key(bundle_id)
            .then(|| bundle_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_configuration_bundle_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        bundle_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.configuration_bundle_idempotency,
            &mut self.configuration_bundle_idempotency_order,
            token_key(operation, &token),
            bundle_id.to_string(),
        );
    }

    /// Drop the idempotency records naming a configuration bundle that no longer
    /// exists, for the same reason [`Self::forget_client_tokens_for`] does.
    pub(super) fn forget_configuration_bundle_client_tokens_for(&mut self, bundle_id: &str) {
        self.configuration_bundle_idempotency
            .retain(|_, id| id != bundle_id);
        let live = &self.configuration_bundle_idempotency;
        self.configuration_bundle_idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// Remove the configuration bundle `bundle_id` names, with everything held
    /// for it: the version lineage the record carries, the name index entry that
    /// reserved its name for this account, and the idempotency records naming it.
    /// Answers the record that was removed, or `None` when no bundle of that id
    /// is stored.
    ///
    /// `DeleteConfigurationBundle` "deletes a configuration bundle and all of its
    /// versions", so the removal is one step rather than three a caller could do
    /// half of: it lives here, at the one place a bundle is dropped, so no path
    /// can leave a name reserved by a bundle that is gone or an idempotency
    /// record pointing at one.
    pub fn remove_configuration_bundle(
        &mut self,
        bundle_id: &str,
    ) -> Option<ConfigurationBundleRecord> {
        let record = self.configuration_bundles.remove(bundle_id)?;
        self.configuration_bundle_names.remove(&record.bundle_name);
        self.forget_configuration_bundle_client_tokens_for(&record.bundle_id);
        Some(record)
    }

    /// Mint a `policyEngineId` for `name` that no stored engine holds: the name
    /// + `-` + 10 lower-case alphanumerics, per `ResourceId`'s pattern
    /// `^[A-Za-z][A-Za-z0-9_]*-[a-z0-9_]{10}$`.
    ///
    /// `PolicyEngineName` is `^[A-Za-z][A-Za-z0-9_]*$`, which is exactly the
    /// stem that pattern asks for, so the name is carried into the id as it
    /// stands rather than folded.
    pub fn mint_policy_engine_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", suffix_for(name, self.mint_counter, at));
            if !self.policy_engines.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint a `policyId` no policy of the engine `policy_engine_id` holds, on the
    /// same `ResourceId` pattern [`Self::mint_policy_engine_id`] mints an engine
    /// id under — a policy carries its own name the same way.
    pub fn mint_policy_id(
        &mut self,
        policy_engine_id: &str,
        name: &str,
        at: SystemTime,
    ) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!(
                "{name}-{}",
                suffix_for(
                    &format!("{policy_engine_id}/{name}"),
                    self.mint_counter,
                    at
                )
            );
            let taken = self
                .policy_engines
                .get(policy_engine_id)
                .is_some_and(|engine| engine.policies.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint a `rateLimitId` no rate limit of the gateway `gateway_id` holds: 10
    /// alphanumerics, per `GatewayRateLimitId`'s pattern
    /// `^[a-zA-Z0-9][a-zA-Z0-9\-_\.]{0,62}[a-zA-Z0-9]$`, which — unlike a
    /// gateway id — carries no name, because a rate limit has none.
    ///
    /// `taken` says which ids the set being written already holds, so a batch
    /// that mints several ids at once cannot mint one twice: the entries of that
    /// batch are not in the store yet.
    pub fn mint_gateway_rate_limit_id(
        &mut self,
        gateway_id: &str,
        seed: &str,
        taken: &BTreeSet<String>,
        at: SystemTime,
    ) -> String {
        self.mint_gateway_child_id(gateway_id, seed, at, suffix_for, |gateway, candidate| {
            taken.contains(candidate) || gateway.rate_limits.contains_key(candidate)
        })
    }

    /// The (gateway, rate limit) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and what it names
    /// still exists.
    ///
    /// The same idempotency record [`Self::remember_gateway_client_token`]
    /// writes, read for a RATE LIMIT rather than a target: the key carries the
    /// operation name, so a token remembered by a rate-limit write is never
    /// resolved against the gateway's targets, and a record whose rate limit has
    /// since been deleted is no replay at all \u2014 the next request carrying that
    /// token is served as a fresh one.
    pub fn replayed_gateway_rate_limit_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
        gateway_identifier: &str,
    ) -> Option<(String, String)> {
        self.replayed_gateway_child_result(
            operation,
            client_token,
            gateway_identifier,
            |gateway, rate_limit_id| gateway.rate_limits.contains_key(rate_limit_id),
        )
    }

    /// The (gateway, child) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the child it names
    /// still exists — `held` says whether the gateway still holds that child.
    ///
    /// A replay is a replay only on the gateway the first request addressed:
    /// `gateway_identifier` is the gateway THIS request addresses, and a token
    /// remembered for another gateway is no replay here — answering with that
    /// gateway's child would report a resource the addressed gateway does not
    /// hold, and would hide the refusal an unknown gateway is owed.
    ///
    /// The shape every gateway-child replay shares, in one place so they cannot
    /// drift.
    fn replayed_gateway_child_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
        gateway_identifier: &str,
        held: impl Fn(&GatewayRecord, &str) -> bool,
    ) -> Option<(String, String)> {
        let (gateway_id, child_id) = self
            .gateway_idempotency
            .get(&token_key(operation, client_token?))?;
        if gateway_id != gateway_identifier {
            return None;
        }
        let child_id = child_id.as_ref()?;
        let gateway = self.gateways.get(gateway_id)?;
        held(gateway, child_id).then(|| (gateway_id.clone(), child_id.clone()))
    }

    /// Mint a `ruleId` no rule of the gateway `gateway_id` holds: the UUID form
    /// `GatewayRuleId` takes
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), which
    /// — unlike a gateway id — carries no name, because a rule has none.
    pub fn mint_gateway_rule_id(&mut self, gateway_id: &str, seed: &str, at: SystemTime) -> String {
        self.mint_gateway_child_id(gateway_id, seed, at, uuid_shaped, |gateway, candidate| {
            gateway.rules.contains_key(candidate)
        })
    }

    /// The (gateway, rule) a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and what it names
    /// still exists.
    ///
    /// The same idempotency record [`Self::remember_gateway_client_token`]
    /// writes, read for a RULE rather than a target: the key carries the
    /// operation name, so a token remembered by a rule create is never resolved
    /// against the gateway's targets, and a record whose rule has since been
    /// deleted is no replay at all — the next request carrying that token is
    /// served as a fresh one.
    pub fn replayed_gateway_rule_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
        gateway_identifier: &str,
    ) -> Option<(String, String)> {
        self.replayed_gateway_child_result(
            operation,
            client_token,
            gateway_identifier,
            |gateway, rule_id| gateway.rules.contains_key(rule_id),
        )
    }

    /// The (policy engine, policy) a previous request to `operation` carrying
    /// this `clientToken` produced, if the record is still held and what it names
    /// still exists.
    pub fn replayed_policy_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<(String, Option<String>)> {
        let (policy_engine_id, policy_id) = self
            .policy_idempotency
            .get(&token_key(operation, client_token?))?;
        let engine = self.policy_engines.get(policy_engine_id)?;
        match policy_id {
            None => Some((policy_engine_id.clone(), None)),
            Some(policy_id) => engine
                .policies
                .contains_key(policy_id)
                .then(|| (policy_engine_id.clone(), Some(policy_id.clone()))),
        }
    }

    /// Record what a `clientToken` produced for `operation` in the policy plane,
    /// evicting the oldest record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held.
    /// An evicted token is indistinguishable from one whose idempotency window
    /// has passed: the next request carrying it is served as a fresh one.
    pub fn remember_policy_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        policy_engine_id: &str,
        policy_id: Option<&str>,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.policy_idempotency,
            &mut self.policy_idempotency_order,
            token_key(operation, &token),
            (policy_engine_id.to_string(), policy_id.map(str::to_string)),
        );
    }

    /// The (policy engine, policy) a policy named `name` is held under, if any
    /// engine of this account holds one.
    ///
    /// A policy name is "the customer-assigned immutable name for the policy.
    /// Must be unique within the account" (`smithy_query doc shape=CreatePolicy
    /// member=name`), so the check spans every engine rather than the one the
    /// request named. The engines and their policies are both bounded, so the
    /// walk is too.
    pub fn policy_named(&self, name: &str) -> Option<(&str, &str)> {
        self.policy_engines.values().find_map(|engine| {
            engine
                .policies
                .values()
                .find(|policy| policy.name == name)
                .map(|policy| (engine.policy_engine_id.as_str(), policy.policy_id.as_str()))
        })
    }

    /// Remove every policy whose deletion the clock has already made due, from
    /// whichever engine held it.
    ///
    /// `DeletePolicy` ACCEPTS the deletion (202) and reports the policy
    /// `DELETING`; the record itself goes when the span it owes passes, so a
    /// caller polling it sees `DELETING` and then the modelled 404, and the name
    /// it held is free again. The removal is owed by the CLOCK — it was due at
    /// this instant whether or not anybody called — so every operation that
    /// reads the policy plane materialises it first, and nothing ever sleeps for
    /// it.
    pub fn expire_deleted_policies(&mut self, now: SystemTime) {
        for engine in self.policy_engines.values_mut() {
            engine
                .policies
                .retain(|_, policy| policy.removal_due_at.is_none_or(|at| now < at));
        }
    }
}

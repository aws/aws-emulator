// SPDX-License-Identifier: CC0-1.0

//! The LOOKUPS of this service's store: the inherent methods
//! `ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState` is reached through — minting
//! an identifier, resolving one to the record it names, and materialising the status
//! transitions the clock has already made due — in one file beside the struct they belong
//! to, which stays in [`super`].

use super::*;

impl ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState {
    /// The guardrail a `guardrailIdentifier` (bare id or full ARN) names.
    pub fn guardrail(&self, identifier: &str) -> Option<&Guardrail> {
        self.guardrails.get(guardrail_id_of(identifier))
    }

    /// The guardrail a `guardrailIdentifier` names, for mutation.
    pub fn guardrail_mut(&mut self, identifier: &str) -> Option<&mut Guardrail> {
        self.guardrails.get_mut(guardrail_id_of(identifier))
    }

    /// Mint a fresh `guardrailId`: `^[a-z0-9]+$`, unique within this store.
    ///
    /// The leading half is this store's mint count, so two ids can never collide;
    /// the trailing half is drawn from the creation instant, so an id is not a
    /// guessable sequence number.
    pub fn mint_guardrail_id(&mut self, now: SystemTime) -> String {
        self.minted_guardrails += 1;
        minted_id(self.minted_guardrails, now)
    }

    /// Mint a fresh `configId` for an enforced guardrail configuration:
    /// `AccountEnforcedGuardrailConfigurationId` is `^[a-z0-9]+$`, which is what
    /// [`minted_id`] spells, and the id is unique within this store for the two reasons a
    /// guardrail id is.
    pub fn mint_enforced_guardrail_config_id(&mut self, now: SystemTime) -> String {
        self.minted_enforced_guardrail_configs += 1;
        minted_id(self.minted_enforced_guardrail_configs, now)
    }

    /// The prompt router a `PromptRouterArn` names.
    pub fn prompt_router(&self, arn: &str) -> Option<&PromptRouter> {
        self.prompt_routers.get(arn)
    }

    /// Forget the prompt router `arn` names, together with its name index entry and the
    /// idempotency token that minted it — so a delete leaves nothing behind for a later
    /// create of the same name to collide with or for a replayed token to hand back, and
    /// both maps stay bounded by the LIVE routers exactly as [`Self::remove_custom_model`]
    /// keeps that family's maps bounded.
    ///
    /// Hands back the record that was removed, or `None` when this store held no router
    /// under that ARN — which is what tells a delete there was nothing to remove.
    pub fn remove_prompt_router(&mut self, arn: &str) -> Option<PromptRouter> {
        let router = self.prompt_routers.remove(arn)?;
        self.prompt_router_names.retain(|_, minted| minted != arn);
        self.prompt_router_create_tokens
            .retain(|_, minted| minted != arn);
        Some(router)
    }

    /// Mint a fresh prompt router id: the resource half of a
    /// `default-prompt-router/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons: the leading half is this
    /// store's mint count so two ids can never collide, and the trailing half is drawn
    /// from the creation instant so an id is not a guessable sequence number. `[a-z0-9]+`
    /// is inside the `[a-zA-Z0-9-:.]+` the `PromptRouterArn` pattern admits there.
    pub fn mint_prompt_router_id(&mut self, now: SystemTime) -> String {
        self.minted_prompt_routers += 1;
        minted_id(self.minted_prompt_routers, now)
    }

    /// Mint a fresh Automated Reasoning policy id: the resource half of an
    /// `automated-reasoning-policy/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `AutomatedReasoningPolicyArn` admits exactly `[a-z0-9]{12}` there, which is what
    /// two [`ID_HALF_WIDTH`] base-36 halves spell, and `AutomatedReasoningPolicyId` is
    /// that same `^[a-z0-9]{12}$`.
    pub fn mint_automated_reasoning_policy_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policies += 1;
        minted_id(self.minted_automated_reasoning_policies, now)
    }

    /// The Automated Reasoning policy an `AutomatedReasoningPolicyArn` names.
    ///
    /// The lookup is EXACT, so the ARN's region and its optional `:<version>` suffix both
    /// select: only the base ARN names the policy whose draft this store holds. Reach a
    /// version through [`Self::automated_reasoning_policy_revision`], which resolves either
    /// spelling.
    #[must_use]
    pub fn automated_reasoning_policy(
        &self,
        policy_arn: &str,
    ) -> Option<&AutomatedReasoningPolicyRecord> {
        self.automated_reasoning_policies.get(policy_arn)
    }

    /// The Automated Reasoning policy an `AutomatedReasoningPolicyArn` names, mutably.
    pub fn automated_reasoning_policy_mut(
        &mut self,
        policy_arn: &str,
    ) -> Option<&mut AutomatedReasoningPolicyRecord> {
        self.automated_reasoning_policies.get_mut(policy_arn)
    }

    /// The REVISION an `AutomatedReasoningPolicyArn` names: the policy, and the version its
    /// optional `:<version>` suffix selects.
    ///
    /// The suffix is what the shape's own pattern admits beside the base ARN
    /// (`^arn:…:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$`), so a
    /// versioned ARN names a version this policy has cut and an unversioned one names its
    /// working draft. A version the policy has not cut names no revision at all, exactly as
    /// an id nothing minted names no policy.
    #[must_use]
    pub fn automated_reasoning_policy_revision(
        &self,
        policy_arn: &str,
    ) -> Option<(
        &AutomatedReasoningPolicyRecord,
        Option<&AutomatedReasoningPolicyVersionRecord>,
    )> {
        let (base_arn, version) = automated_reasoning_policy_arn_parts(policy_arn);
        let policy = self.automated_reasoning_policies.get(base_arn)?;
        match version {
            None => Some((policy, None)),
            Some(number) => Some((policy, Some(policy.version(number)?))),
        }
    }

    /// The `ResourceNotFoundException` message an `AutomatedReasoningPolicyArn` that
    /// [`Self::automated_reasoning_policy_revision`] resolves to NOTHING is refused with.
    ///
    /// WHICH half of the ARN resolved to nothing decides the sentence: an ARN naming a
    /// version of a policy this account DOES hold addressed a revision that policy has not
    /// cut, so the refusal reports the missing version
    /// ([`AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`]) rather than the policy, which is
    /// there. Here rather than in one handler because the operations that READ one policy by
    /// this ARN, and those that address a CHILD of the policy it names, all report one
    /// unresolvable ARN the same way, and that wording is one rule; the policy's own writes
    /// report the absent policy through
    /// [`Self::automated_reasoning_policy_write_not_found_message`] and the listing of the
    /// policies themselves through
    /// [`Self::automated_reasoning_policy_listing_not_found_message`], because Amazon
    /// Bedrock words the absent policy per operation.
    #[must_use]
    pub fn automated_reasoning_policy_revision_not_found_message(&self, policy_arn: &str) -> String {
        self.revision_not_found_message(policy_arn, AUTOMATED_REASONING_POLICY_NOT_FOUND)
    }

    /// The same message for a WRITE of the policy itself, which words the absent policy as
    /// [`AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_WRITE`] — the missing VERSION of a policy
    /// that IS there is reported in the one sentence every operation reports it in, which is
    /// the only wording recorded for that half of the ARN.
    #[must_use]
    pub fn automated_reasoning_policy_write_not_found_message(&self, policy_arn: &str) -> String {
        self.revision_not_found_message(policy_arn, AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_WRITE)
    }

    /// The same message for the LISTING of the policies themselves, whose absent policy is
    /// worded [`AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_LISTING`] — one more operation that
    /// words this error class its own way, served from here so the branch deciding WHICH
    /// half of the ARN resolved to nothing is still written once.
    #[must_use]
    pub fn automated_reasoning_policy_listing_not_found_message(&self, policy_arn: &str) -> String {
        self.revision_not_found_message(policy_arn, AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_LISTING)
    }

    /// Which half of an unresolvable `AutomatedReasoningPolicyArn` resolved to nothing,
    /// reported with `absent_policy` as the sentence for a policy this account does not
    /// hold. The branch lives here once, so the sentences a caller can be refused with
    /// differ only in that wording.
    pub(super) fn revision_not_found_message(&self, policy_arn: &str, absent_policy: &str) -> String {
        let (base_arn, version) = automated_reasoning_policy_arn_parts(policy_arn);
        if version.is_some() && self.automated_reasoning_policy(base_arn).is_some() {
            AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND.to_string()
        } else {
            absent_policy.to_string()
        }
    }

    /// Mint a fresh `AutomatedReasoningPolicyBuildWorkflowId`: the lowercase hyphenated
    /// UUIDv4 that member's pattern spells
    /// (`^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`),
    /// unique within this store because [`minted_uuid`] hashes this store's mint count in.
    pub fn mint_build_workflow_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policy_build_workflows += 1;
        minted_uuid(
            self.minted_automated_reasoning_policy_build_workflows,
            now,
        )
    }

    /// The build workflow `(policy_arn, build_workflow_id)` names, with any status
    /// transition the clock has made due at `now` materialised first — so every caller
    /// reads the status the workflow holds at `now`, whichever operation asked.
    pub fn automated_reasoning_policy_build_workflow(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        now: SystemTime,
    ) -> Option<&mut AutomatedReasoningPolicyBuildWorkflowRecord> {
        let record = self
            .automated_reasoning_policy_build_workflows
            .get_mut(&(policy_arn.to_string(), build_workflow_id.to_string()))?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every build workflow transition of the policy `policy_arn` names that
    /// the clock has made due at `now`, so a caller asking about the POLICY sees the same
    /// workflows a read of each one would.
    pub fn settle_automated_reasoning_policy_build_workflows(
        &mut self,
        policy_arn: &str,
        now: SystemTime,
    ) {
        for ((arn, _), record) in &mut self.automated_reasoning_policy_build_workflows {
            if arn == policy_arn {
                record.settle(now);
            }
        }
    }

    /// Every build workflow started for the policy `policy_arn` names, as this store holds
    /// them now (settle first, with
    /// [`Self::settle_automated_reasoning_policy_build_workflows`], to read them as of an
    /// instant).
    pub fn automated_reasoning_policy_build_workflows_of<'a>(
        &'a self,
        policy_arn: &'a str,
    ) -> impl Iterator<Item = &'a AutomatedReasoningPolicyBuildWorkflowRecord> + 'a {
        self.automated_reasoning_policy_build_workflows
            .iter()
            .filter(move |((arn, _), _)| arn == policy_arn)
            .map(|(_, record)| record)
    }

    /// Mint a fresh Automated Reasoning policy test id, unique within this store.
    ///
    /// Built like a policy id, and for the same two reasons: the leading half is this
    /// store's mint count so two ids can never collide, and the trailing half is drawn
    /// from the creation instant so an id is not a guessable sequence number. Rendered in
    /// UPPER case because `AutomatedReasoningPolicyTestCaseId` is `^[0-9A-Z]{12}$`, which
    /// is the upper-case spelling of the twelve base-36 digits [`minted_id`] produces.
    pub fn mint_automated_reasoning_policy_test_case_id(&mut self, now: SystemTime) -> String {
        self.minted_automated_reasoning_policy_test_cases += 1;
        minted_id(self.minted_automated_reasoning_policy_test_cases, now).to_ascii_uppercase()
    }

    /// The test `test_case_id` names under the policy `policy_arn` names.
    ///
    /// Keyed by the PAIR, so an id of another policy's test names no test of this one, and
    /// the policy ARN selects exactly as [`Self::automated_reasoning_policy`] does.
    #[must_use]
    pub fn automated_reasoning_policy_test_case(
        &self,
        policy_arn: &str,
        test_case_id: &str,
    ) -> Option<&AutomatedReasoningPolicyTestCaseRecord> {
        self.automated_reasoning_policy_test_cases
            .get(&(policy_arn.to_string(), test_case_id.to_string()))
    }

    /// Every test stored under the policy `policy_arn` names, in `testCaseId` order.
    ///
    /// Keyed by the PAIR like [`Self::automated_reasoning_policy_test_case`], so a listing
    /// reports exactly the tests a read of each one reports and none of another policy's.
    /// The order is the id's, which is a TOTAL order over
    /// `AutomatedReasoningPolicyTestCaseId` (`^[0-9A-Z]{12}$`): a listing needs one so its
    /// pages partition the tests whatever is created or deleted between two calls.
    #[must_use]
    pub fn automated_reasoning_policy_test_cases_of(
        &self,
        policy_arn: &str,
    ) -> Vec<&AutomatedReasoningPolicyTestCaseRecord> {
        let mut records: Vec<&AutomatedReasoningPolicyTestCaseRecord> = self
            .automated_reasoning_policy_test_cases
            .iter()
            .filter(|((arn, _), _)| arn == policy_arn)
            .map(|(_, record)| record)
            .collect();
        records.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        records
    }

    /// How many tests the policy `policy_arn` names holds — what
    /// [`MAX_AUTOMATED_REASONING_POLICY_TEST_CASES_PER_POLICY`] bounds.
    #[must_use]
    pub fn automated_reasoning_policy_test_case_count(&self, policy_arn: &str) -> usize {
        self.automated_reasoning_policy_test_cases
            .keys()
            .filter(|(arn, _)| arn == policy_arn)
            .count()
    }

    /// The run of the test `test_case_id` against the build workflow `build_workflow_id` of
    /// the policy `policy_arn` names, with any status transition the clock has made due at
    /// `now` materialised first — so every caller reads the status the run holds at `now`,
    /// whichever operation asked.
    pub fn automated_reasoning_policy_test_run(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        test_case_id: &str,
        now: SystemTime,
    ) -> Option<&mut AutomatedReasoningPolicyTestRunRecord> {
        let record = self.automated_reasoning_policy_test_runs.get_mut(&(
            policy_arn.to_string(),
            build_workflow_id.to_string(),
            test_case_id.to_string(),
        ))?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every test run transition of the build workflow `build_workflow_id` of
    /// the policy `policy_arn` names that the clock has made due at `now`, so a caller
    /// asking about the BUILD WORKFLOW sees the same runs a read of each one would.
    pub fn settle_automated_reasoning_policy_test_runs(
        &mut self,
        policy_arn: &str,
        build_workflow_id: &str,
        now: SystemTime,
    ) {
        for ((arn, workflow, _), record) in &mut self.automated_reasoning_policy_test_runs {
            if arn == policy_arn && workflow == build_workflow_id {
                record.settle(now);
            }
        }
    }

    /// Every run scheduled against the build workflow `build_workflow_id` of the policy
    /// `policy_arn` names, in `testCaseId` order (settle first, with
    /// [`Self::settle_automated_reasoning_policy_test_runs`], to read them as of an
    /// instant).
    ///
    /// The order is the test id's, which is a TOTAL order over
    /// `AutomatedReasoningPolicyTestCaseId` (`^[0-9A-Z]{12}$`), exactly as the tests
    /// themselves are ordered.
    #[must_use]
    pub fn automated_reasoning_policy_test_runs_of(
        &self,
        policy_arn: &str,
        build_workflow_id: &str,
    ) -> Vec<&AutomatedReasoningPolicyTestRunRecord> {
        let mut records: Vec<&AutomatedReasoningPolicyTestRunRecord> = self
            .automated_reasoning_policy_test_runs
            .iter()
            .filter(|((arn, workflow, _), _)| arn == policy_arn && workflow == build_workflow_id)
            .map(|(_, record)| record)
            .collect();
        records.sort_unstable_by(|left, right| left.test_case_id.cmp(&right.test_case_id));
        records
    }

    /// Mint a fresh inference profile id: the resource half of an
    /// `application-inference-profile/<id>` ARN, unique within this store.
    ///
    /// Built like a prompt router id, and for the same two reasons: the leading half is
    /// this store's mint count so two ids can never collide, and the trailing half is
    /// drawn from the creation instant so an id is not a guessable sequence number.
    /// `[a-z0-9]+` is inside the `[a-zA-Z0-9-:.]+` that `InferenceProfileArn` admits
    /// there, and inside the `^[a-zA-Z0-9-:.]+$` of `InferenceProfileId`.
    pub fn mint_inference_profile_id(&mut self, now: SystemTime) -> String {
        self.minted_inference_profiles += 1;
        minted_id(self.minted_inference_profiles, now)
    }

    /// The `InferenceProfileArn` an `InferenceProfileIdentifier` names: the identifier
    /// itself when it is the ARN of a stored profile, otherwise the ARN the profile of
    /// that `inferenceProfileId` carries in `region`.
    ///
    /// The identifier's pattern admits that ARN or a bare profile id
    /// (`smithy_query(query="operation",operation="GetInferenceProfile")`), so both
    /// spellings address the same profile. Either spelling resolves only to a profile of
    /// `region`: the ARN carries a region, so one of another region names another
    /// account-region's profile.
    #[must_use]
    pub fn inference_profile_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if let Some(record) = self.inference_profiles.get(identifier) {
            return (record.region == region).then(|| identifier.to_string());
        }
        let spelled = application_inference_profile_arn(region, identifier);
        self.inference_profiles
            .contains_key(&spelled)
            .then_some(spelled)
    }

    /// The application inference profile an `InferenceProfileIdentifier` names.
    #[must_use]
    pub fn inference_profile(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<&InferenceProfileRecord> {
        let arn = self.inference_profile_arn_of(identifier, region)?;
        self.inference_profiles.get(&arn)
    }

    /// Forget the application inference profile `arn` names, together with the idempotency
    /// tokens that minted it — so the token map stays bounded by the LIVE profiles rather
    /// than by every create this store has ever admitted.
    pub fn remove_inference_profile(&mut self, arn: &str) -> Option<InferenceProfileRecord> {
        let record = self.inference_profiles.remove(arn)?;
        self.inference_profile_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
    }

    /// The marketplace model endpoint an `endpointArn` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status the endpoint holds at `now`, whichever operation asked.
    pub fn marketplace_model_endpoint(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut MarketplaceModelEndpointRecord> {
        let endpoint = self.marketplace_model_endpoints.get_mut(arn)?;
        endpoint.settle(now);
        Some(endpoint)
    }

    /// Drop the marketplace model endpoint an `endpointArn` names, handing back the record
    /// that was removed (`None` when this store held none under that ARN).
    ///
    /// The create tokens that minted it go with it, the way
    /// [`Self::remove_inference_profile`] drops the tokens that minted a profile: the token
    /// map stays bounded by the LIVE endpoints rather than by every create this store has
    /// ever admitted, and a token whose endpoint is gone cannot replay an endpoint back
    /// into being.
    pub fn remove_marketplace_model_endpoint(
        &mut self,
        arn: &str,
    ) -> Option<MarketplaceModelEndpointRecord> {
        let endpoint = self.marketplace_model_endpoints.remove(arn)?;
        self.marketplace_endpoint_create_tokens
            .retain(|_, minted| minted != arn);
        Some(endpoint)
    }

    /// The `ModelImportJobArn` a `ModelImportJobIdentifier` names: the identifier itself
    /// when it is the ARN of a stored job, otherwise the ARN the job of that NAME in
    /// `region` carries.
    ///
    /// The identifier's pattern is the job ARN OR a bare `JobName`
    /// (`smithy_query(query="operation",operation="GetModelImportJob")`), so both
    /// spellings address the same job.
    #[must_use]
    pub fn model_import_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_import_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.model_import_job_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The model import job a `ModelImportJobIdentifier` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status the job holds at `now`, whichever operation asked.
    pub fn model_import_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelImportJobRecord> {
        let arn = self.model_import_job_arn_of(identifier, region)?;
        let job = self.model_import_jobs.get_mut(&arn)?;
        job.complete(now);
        Some(job)
    }

    /// The ARN of the import job that imports the model an `ImportedModelIdentifier`
    /// names, or `None` when no stored job imports one.
    ///
    /// An `ImportedModelIdentifier` is the imported model's ARN or its bare NAME
    /// (`smithy_query(query="operation",operation="GetImportedModel")`), so both spellings
    /// address the same model, and a name names at most one imported model in an account
    /// and region. The imported model is not a record of its own: the job that imports it
    /// holds its identity, its data source and its key from the call that accepted the
    /// import onwards, so there is ONE copy of the model for every operation to read.
    /// The ARN spelling takes PRECEDENCE over the name, as
    /// [`Self::model_import_job_arn_of`] gives the job ARN precedence over the job name:
    /// the store is a `HashMap`, so a scan that accepted either match without an order
    /// would answer whichever record it reached first.
    #[must_use]
    pub fn importing_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        let mut named = None;
        for job in self.model_import_jobs.values() {
            if job.region != region {
                continue;
            }
            if job.imported_model_arn == identifier {
                return Some(job.arn.clone());
            }
            if job.imported_model_name == identifier {
                named = Some(job.arn.clone());
            }
        }
        named
    }

    /// Whether a stored job in `region` already imports a model under
    /// `imported_model_name`.
    #[must_use]
    pub fn imports_model_named(&self, region: &str, imported_model_name: &str) -> bool {
        self.importing_job_arn_of(imported_model_name, region)
            .is_some()
    }

    /// The imported model an `ImportedModelIdentifier` names, reported as the import job
    /// that holds it, together with the instant the import ENDED — the instant that model
    /// came into being — with any status transition the clock has made due at `now`
    /// materialised first, exactly as [`Self::model_import_job`] does it.
    ///
    /// `None` while that job has not reached [`IMPORT_JOB_STATUS_COMPLETED`]: importing is
    /// what CREATES the customized model ("A successful job transitions from in-progress to
    /// completed when the imported model is ready to use",
    /// `smithy_query(query="operation",operation="GetModelImportJob")`), so until the
    /// import has finished there is no imported model to report. That is also why the end
    /// instant is handed back BESIDE the record rather than left to a caller to unwrap:
    /// [`ModelImportJobRecord::complete`] sets it together with the terminal status, so a
    /// model this reports always has one.
    pub fn imported_model(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<(&ModelImportJobRecord, SystemTime)> {
        let arn = self.importing_job_arn_of(identifier, region)?;
        let job = self.model_import_jobs.get_mut(&arn)?;
        job.complete(now);
        let ended_at = job.ended_at?;
        (job.status == IMPORT_JOB_STATUS_COMPLETED).then_some((&*job, ended_at))
    }

    /// Mint a fresh model import job id: the resource half of a `model-import-job/<id>`
    /// ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ModelImportJobArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell.
    pub fn mint_model_import_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_import_jobs += 1;
        minted_id(self.minted_model_import_jobs, now)
    }

    /// Mint a fresh imported model id: the resource half of an `imported-model/<id>` ARN,
    /// unique within this store. `ImportedModelArn` admits `[a-z0-9]{12}` there, as the
    /// job ARN does.
    pub fn mint_imported_model_id(&mut self, now: SystemTime) -> String {
        self.minted_imported_models += 1;
        minted_id(self.minted_imported_models, now)
    }

    /// The evaluation job an `EvaluationJobIdentifier` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status it holds at `now`, whichever operation asked.
    ///
    /// A job a batch delete accepted and whose removal the clock has since made due is gone
    /// by the time it is asked for, so an identifier that named it names nothing — the one
    /// other transition this resource is owed, and the reason the removal is swept here
    /// rather than inside [`EvaluationJobRecord::settle`], which cannot reach the store the
    /// record is held in.
    pub fn evaluation_job(
        &mut self,
        identifier: &str,
        now: SystemTime,
    ) -> Option<&mut EvaluationJobRecord> {
        self.expire_deleted_evaluation_jobs(now);
        let job = self.evaluation_jobs.get_mut(identifier)?;
        job.settle(now);
        Some(job)
    }

    /// Materialise EVERY evaluation job's status transition the clock has already made due
    /// at `now`, and forget every job whose removal it has made due.
    ///
    /// What a listing reports — and what its `statusEquals` filters on — is the status each
    /// job holds at `now`, which is the status a read of that one job reports at the same
    /// instant ([`EvaluationJobRecord::settle`]).
    pub fn settle_evaluation_jobs(&mut self, now: SystemTime) {
        self.expire_deleted_evaluation_jobs(now);
        for record in self.evaluation_jobs.values_mut() {
            record.settle(now);
        }
    }

    /// The runs the evaluation jobs of `region` still owe, taken ONCE
    /// ([`EvaluationJobRecord::due_run`]) and at most
    /// [`evaluation_run::MAX_RUNS_PER_REQUEST`] of them per call.
    ///
    /// A listing reports the status every job it carries holds, and a terminal status is the
    /// status the job's RUN ended in — so a listing takes the runs its jobs are owed exactly
    /// as a read of one job takes that job's, and the two cannot report different statuses
    /// for one job. Jobs of another region are another account-region's resources and are
    /// not listed, so their runs are not taken here either.
    ///
    /// The number taken per call is BOUNDED, for the reason
    /// [`Self::due_model_invocation_runs`] bounds its own: a job whose run this call does not
    /// take still owes it, so a later call takes it — and the iteration is lazy, so a run
    /// that is not taken is not marked as taken either.
    pub fn due_evaluation_runs(&mut self, region: &str) -> Vec<evaluation_run::DueRun> {
        self.evaluation_jobs
            .values_mut()
            .filter(|record| record.region == region)
            .filter_map(EvaluationJobRecord::due_run)
            .take(evaluation_run::MAX_RUNS_PER_REQUEST)
            .collect()
    }

    /// Forget every evaluation job whose removal the clock has already made due at `now`.
    ///
    /// Swept over the whole family rather than over the one identifier asked for, because a
    /// removal frees the job's NAME and its place under the account's bound: both are read
    /// without an identifier in hand.
    pub fn expire_deleted_evaluation_jobs(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .evaluation_jobs
            .iter()
            .filter(|(_, job)| job.is_removal_due(now))
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in due {
            self.remove_evaluation_job(&arn);
        }
    }

    /// Forget the evaluation job `arn` names, together with its name index entry and the
    /// idempotency tokens that minted it — so a deletion leaves nothing behind for a later
    /// create of the same name to collide with, exactly as
    /// [`Self::remove_provisioned_model_throughput`] does for that family.
    pub fn remove_evaluation_job(&mut self, arn: &str) -> Option<EvaluationJobRecord> {
        let job = self.evaluation_jobs.remove(arn)?;
        self.evaluation_job_names
            .remove(&(job.region.clone(), job.name.clone()));
        self.evaluation_job_create_tokens
            .retain(|_, minted| minted != arn);
        Some(job)
    }

    /// Mint a fresh evaluation job id: the resource half of an `evaluation-job/<id>` ARN,
    /// unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `EvaluationJobArn` admits
    /// exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36 halves
    /// spell.
    pub fn mint_evaluation_job_id(&mut self, now: SystemTime) -> String {
        self.minted_evaluation_jobs += 1;
        minted_id(self.minted_evaluation_jobs, now)
    }

    /// Mint a fresh advanced prompt optimization job id: the resource half of an
    /// `advanced-prompt-optimization-job/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `AdvancedPromptOptimizationJobArn` admits exactly `[a-z0-9]{12}` there, which is
    /// what two [`ID_HALF_WIDTH`] base-36 halves spell, and
    /// `AdvancedPromptOptimizationJobIdentifier`'s bare alternative is that same
    /// `[a-z0-9]{12}`.
    pub fn mint_advanced_prompt_optimization_job_id(&mut self, now: SystemTime) -> String {
        self.minted_advanced_prompt_optimization_jobs += 1;
        minted_id(self.minted_advanced_prompt_optimization_jobs, now)
    }

    /// The `AdvancedPromptOptimizationJobArn` an `AdvancedPromptOptimizationJobIdentifier`
    /// names: the identifier itself when it is the ARN of a stored job, otherwise the ARN
    /// the job of that ID carries in `region`.
    ///
    /// The identifier's pattern is that ARN with its `advanced-prompt-optimization-job/<id>`
    /// tail, or the bare `<id>` alone
    /// (`smithy_query(query="operation",operation="GetAdvancedPromptOptimizationJob")`), so
    /// both spellings address the same job — resolved exactly as
    /// [`Self::model_invocation_job_arn_of`] resolves a batch inference job's two
    /// spellings.
    #[must_use]
    pub fn advanced_prompt_optimization_job_arn_of(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<String> {
        if self
            .advanced_prompt_optimization_jobs
            .contains_key(identifier)
        {
            return Some(identifier.to_string());
        }
        let arn = advanced_prompt_optimization_job_arn(region, identifier);
        self.advanced_prompt_optimization_jobs
            .contains_key(&arn)
            .then_some(arn)
    }

    /// The advanced prompt optimization job an `AdvancedPromptOptimizationJobIdentifier`
    /// names, with any status transition the clock has made due at `now` materialised first
    /// — so every caller reads the status the job holds at `now`, whichever operation asked.
    pub fn advanced_prompt_optimization_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut AdvancedPromptOptimizationJobRecord> {
        let arn = self.advanced_prompt_optimization_job_arn_of(identifier, region)?;
        let record = self.advanced_prompt_optimization_jobs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Mint a fresh Provisioned Throughput id: the resource half of a
    /// `provisioned-model/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ProvisionedModelArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell.
    pub fn mint_provisioned_model_id(&mut self, now: SystemTime) -> String {
        self.minted_provisioned_models += 1;
        minted_id(self.minted_provisioned_models, now)
    }

    /// The `ProvisionedModelArn` a `ProvisionedModelId` names: the identifier itself when
    /// it is the ARN of a stored Provisioned Throughput, otherwise the ARN the
    /// Provisioned Throughput of that NAME in `region` carries.
    ///
    /// The identifier's pattern is that ARN OR a bare `ProvisionedModelName`
    /// (`smithy_query(query="operation",operation="GetProvisionedModelThroughput")`), so
    /// both spellings address the same Provisioned Throughput.
    #[must_use]
    pub fn provisioned_model_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.provisioned_model_throughputs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.provisioned_model_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The Provisioned Throughput a `ProvisionedModelId` names, with any status transition
    /// the clock has made due at `now` materialised first — so every caller reads the
    /// status it holds at `now`, whichever operation asked.
    pub fn provisioned_model_throughput(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ProvisionedModelThroughputRecord> {
        let arn = self.provisioned_model_arn_of(identifier, region)?;
        let record = self.provisioned_model_throughputs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every Provisioned Throughput status transition the clock has made due
    /// at `now`, so a LISTING reports — and filters on — the status each record holds at
    /// `now` exactly as a read of that one record would.
    pub fn settle_provisioned_model_throughputs(&mut self, now: SystemTime) {
        for record in self.provisioned_model_throughputs.values_mut() {
            record.settle(now);
        }
    }

    /// Mint a fresh custom model id: the resource half of a `custom-model/<base>/<id>`
    /// ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `CustomModelArn` admits
    /// exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36 halves
    /// spell.
    pub fn mint_custom_model_id(&mut self, now: SystemTime) -> String {
        self.minted_custom_models += 1;
        minted_id(self.minted_custom_models, now)
    }

    /// The `CustomModelArn` a `ModelIdentifier` names: the identifier itself when it is
    /// the ARN of a stored custom model, otherwise the ARN the custom model of that NAME
    /// in `region` carries.
    ///
    /// The identifier's pattern admits that ARN or a bare model name
    /// (`smithy_query(query="operation",operation="GetCustomModel")`), so both spellings
    /// address the same model.
    /// Either spelling resolves only to a model of `region`: the ARN carries a region, so
    /// one of another region names another account-region's model, exactly as the listing
    /// reports only this region's.
    #[must_use]
    pub fn custom_model_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if let Some(record) = self.custom_models.get(identifier) {
            return (record.region == region).then(|| identifier.to_string());
        }
        self.custom_model_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The custom model a `ModelIdentifier` names, with any status transition the clock
    /// has made due at `now` materialised first — so every caller reads the status the
    /// model holds at `now`, whichever operation asked.
    pub fn custom_model(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut CustomModelRecord> {
        // A customization job the clock has already completed is what brings its output
        // model into being, and so is a copy job the clock has already completed, so those
        // completions are materialised before a model is looked
        // up: such a model was owed at its job's due instant even if nobody had read the
        // job.
        self.settle_model_customization_jobs(now);
        self.settle_model_copy_jobs(now);
        let arn = self.custom_model_arn_of(identifier, region)?;
        let record = self.custom_models.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise every custom model status transition the clock has made due at `now`,
    /// so a LISTING reports — and filters on — the status each record holds at `now`
    /// exactly as a read of that one model would.
    pub fn settle_custom_models(&mut self, now: SystemTime) {
        // Including the models whose customization or copy job the clock has completed,
        // for the reason [`Self::custom_model`] materialises those first.
        self.settle_model_customization_jobs(now);
        self.settle_model_copy_jobs(now);
        for record in self.custom_models.values_mut() {
            record.settle(now);
        }
    }

    /// Forget the custom model `arn` names, together with its name index entry and the
    /// idempotency tokens that minted it — so a delete leaves nothing behind for a later
    /// create of the same name to collide with, and the token map stays bounded by the LIVE
    /// custom models, exactly as [`Self::remove_provisioned_model_throughput`] keeps that
    /// family's maps bounded.
    pub fn remove_custom_model(&mut self, arn: &str) -> Option<CustomModelRecord> {
        let record = self.custom_models.remove(arn)?;
        self.custom_model_names
            .remove(&(record.region.clone(), record.name.clone()));
        self.custom_model_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
    }

    /// The ARN of the custom model deployment serving the custom model `arn` names, or
    /// `None` when no stored deployment serves it.
    ///
    /// A deployment carries "the custom model this deployment serves"
    /// ([`CustomModelDeploymentRecord::model_arn`]), so it holds a live association to that
    /// one model — the association a delete of the model would leave pointing at a resource
    /// that no longer exists. The SMALLEST such ARN is reported, so a refusal names the same
    /// deployment however the map happens to be ordered.
    ///
    /// Only this family is asked: a Provisioned Throughput of this store is associated with
    /// a FOUNDATION model (`ops/create_provisioned_model_throughput.rs` resolves its
    /// `modelId` through the published catalog and stores that model's ARN), so none of them
    /// can hold an association to a custom model.
    #[must_use]
    pub fn custom_model_deployment_serving(&self, arn: &str) -> Option<String> {
        self.custom_model_deployments
            .values()
            .filter(|record| record.model_arn == arn)
            .map(|record| record.arn.clone())
            .min()
    }

    /// Mint a fresh model customization job id: the resource half of a
    /// `model-customization-job/<base model>/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons;
    /// `ModelCustomizationJobArn` admits exactly `[a-z0-9]{12}` there, which is what two
    /// [`ID_HALF_WIDTH`] base-36 halves spell.
    pub fn mint_model_customization_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_customization_jobs += 1;
        minted_id(self.minted_model_customization_jobs, now)
    }

    /// The `ModelCustomizationJobArn` a `ModelCustomizationJobIdentifier` names: the
    /// identifier itself when it is the ARN of a stored job, otherwise the ARN the job of
    /// that NAME in `region` carries.
    ///
    /// The identifier's pattern is that ARN OR a bare `JobName`
    /// (`smithy_query(query="operation",operation="GetModelCustomizationJob")`), so both
    /// spellings address the same job — resolved exactly as
    /// [`Self::model_import_job_arn_of`] resolves an import job's two spellings.
    #[must_use]
    pub fn model_customization_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_customization_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.model_customization_job_names
            .get(&(region.to_string(), identifier.to_string()))
            .cloned()
    }

    /// The model customization job a `ModelCustomizationJobIdentifier` names, with any
    /// status transition the clock has made due at `now` materialised first — so every
    /// caller reads the status the job holds at `now`, whichever operation asked.
    pub fn model_customization_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelCustomizationJobRecord> {
        let arn = self.model_customization_job_arn_of(identifier, region)?;
        self.settle_model_customization_job(&arn, now);
        self.model_customization_jobs.get_mut(&arn)
    }

    /// Materialise the transition the clock has already made due at `now` for the job
    /// `arn` names: the stop a caller asked for, or the completion that brings the custom
    /// model the job was customizing into being.
    ///
    /// The only write a reporting operation on this resource makes, and it was owed at the
    /// due instant even if nobody had called — which is why that instant, not `now`, is the
    /// one the job reports as its end and the one the model it produced reports as its
    /// creation. The stop takes precedence because [`ModelCustomizationJobRecord::stop`]
    /// drops the completion the job was owed: a job that was stopped produces no model.
    pub(super) fn settle_model_customization_job(&mut self, arn: &str, now: SystemTime) {
        let completed = {
            let Some(job) = self.model_customization_jobs.get_mut(arn) else {
                return;
            };
            if let Some(due) = job.stopped_due_at.filter(|due| now >= *due) {
                job.status = MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED;
                job.stopped_due_at = None;
                job.ended_at = Some(due);
                job.last_modified_at = due;
                return;
            }
            let Some(due) = job.completed_due_at.filter(|due| now >= *due) else {
                return;
            };
            job.status = MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED;
            job.completed_due_at = None;
            job.ended_at = Some(due);
            job.last_modified_at = due;
            CustomModelRecord {
                arn: job.output_model_arn.clone(),
                name: job.output_model_name.clone(),
                region: job.region.clone(),
                base_model_arn: job.base_model_arn.clone(),
                base_model_name: base_model_id_of(&job.base_model_arn).to_string(),
                // This model descends from the foundation model the job customized, which
                // is what `ListCustomModels`' `foundationModelArnEquals` filters on.
                foundation_model_arn: Some(job.base_model_arn.clone()),
                job_arn: Some(job.arn.clone()),
                customization_type: job.customization_type,
                // The artifacts were produced by the customization rather than named by a
                // request, so the model carries neither source.
                model_source_config: None,
                custom_model_data_source: None,
                model_kms_key_arn: job.output_model_kms_key_arn.clone(),
                role_arn: Some(job.role_arn.clone()),
                tags: job.custom_model_tags.clone(),
                // "After the model-customization job completes successfully, your custom
                // model resource will be ready to use", so the model this registers is
                // ready and the clock owes it no further transition.
                status: CUSTOM_MODEL_STATUS_ACTIVE,
                active_due_at: None,
                created_at: due,
            }
        };
        self.custom_model_names.insert(
            (completed.region.clone(), completed.name.clone()),
            completed.arn.clone(),
        );
        self.custom_models.insert(completed.arn.clone(), completed);
    }

    /// Materialise every model customization job transition the clock has made due at
    /// `now`, so a reader of the resources those jobs PRODUCE sees the same jobs a read of
    /// each one would.
    ///
    /// Lives beside the per-job materialisation because a completion brings a custom model
    /// into being: a caller that reads only the model never touches the job, and the model
    /// was owed at the job's due instant even so.
    pub fn settle_model_customization_jobs(&mut self, now: SystemTime) {
        let arns: Vec<String> = self.model_customization_jobs.keys().cloned().collect();
        for arn in arns {
            self.settle_model_customization_job(&arn, now);
        }
    }

    /// Mint a fresh model copy job id: the resource half of a `model-copy-job/<id>` ARN,
    /// unique within this store.
    ///
    /// Built like a model customization job id, and for the same two reasons;
    /// `ModelCopyJobArn` admits exactly `[a-z0-9]{12}` there.
    pub fn mint_model_copy_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_copy_jobs += 1;
        minted_id(self.minted_model_copy_jobs, now)
    }

    /// The model copy job a `ModelCopyJobArn` names, with any status transition the clock
    /// has made due at `now` materialised first — so every caller reads the status the job
    /// holds at `now`, whichever operation asked.
    pub fn model_copy_job(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&ModelCopyJobRecord> {
        self.settle_model_copy_job(arn, now);
        self.model_copy_jobs.get(arn)
    }

    /// Materialise the completion the clock has already made due at `now` for the job
    /// `arn` names: the copy finishing is what brings the copied custom model into being,
    /// so the completion registers that model beside the account's other custom models
    /// rather than holding a second copy of it.
    ///
    /// The only write a reporting operation on this resource makes, and it was owed at the
    /// due instant even if nobody had called — which is why that instant, not `now`, is the
    /// one the model the copy produced reports as its creation.
    pub(super) fn settle_model_copy_job(&mut self, arn: &str, now: SystemTime) {
        let copied = {
            let Some(job) = self.model_copy_jobs.get_mut(arn) else {
                return;
            };
            let Some(due) = job.completed_due_at.filter(|due| now >= *due) else {
                return;
            };
            job.status = MODEL_COPY_JOB_STATUS_COMPLETED;
            job.completed_due_at = None;
            CustomModelRecord {
                arn: job.target_model_arn.clone(),
                name: job.target_model_name.clone(),
                region: job.region.clone(),
                // The copy is of the SOURCE model, so the copied model descends from
                // whatever that model descended from: its base is the source model's own
                // base, which is the segment both ARNs carry.
                base_model_arn: job.source_model_arn.clone(),
                base_model_name: custom_model_base_segment(&job.source_model_arn)
                    .unwrap_or(CUSTOM_MODEL_IMPORTED_BASE)
                    .to_string(),
                foundation_model_arn: None,
                job_arn: Some(job.arn.clone()),
                customization_type: CUSTOMIZATION_TYPE_IMPORTED,
                // The artifacts were copied from another model rather than named by a
                // request, so the copied model carries neither source.
                model_source_config: None,
                custom_model_data_source: None,
                model_kms_key_arn: job.target_model_kms_key_arn.clone(),
                role_arn: None,
                tags: job.target_model_tags.clone(),
                // The copy has finished, so the model it produced is ready and the clock
                // owes it no further transition.
                status: CUSTOM_MODEL_STATUS_ACTIVE,
                active_due_at: None,
                created_at: due,
            }
        };
        self.custom_model_names.insert(
            (copied.region.clone(), copied.name.clone()),
            copied.arn.clone(),
        );
        self.custom_models.insert(copied.arn.clone(), copied);
    }

    /// Materialise every model copy job completion the clock has made due at `now`, so a
    /// reader of the custom models those copies PRODUCE sees the same jobs a read of each
    /// one would.
    ///
    /// Lives beside the per-job materialisation for the reason
    /// [`Self::settle_model_customization_jobs`] does: a completion brings a custom model
    /// into being, and a caller that reads only the model never touches the job.
    pub fn settle_model_copy_jobs(&mut self, now: SystemTime) {
        let arns: Vec<String> = self.model_copy_jobs.keys().cloned().collect();
        for arn in arns {
            self.settle_model_copy_job(&arn, now);
        }
    }

    /// Mint a fresh batch inference job id: the resource half of a
    /// `model-invocation-job/<id>` ARN, unique within this store.
    ///
    /// Built like a guardrail id, and for the same two reasons; `ModelInvocationJobArn`
    /// admits exactly `[a-z0-9]{12}` there, which is what two [`ID_HALF_WIDTH`] base-36
    /// halves spell, and `ModelInvocationJobIdentifier`'s bare alternative is that same
    /// `[a-z0-9]{12}`.
    pub fn mint_model_invocation_job_id(&mut self, now: SystemTime) -> String {
        self.minted_model_invocation_jobs += 1;
        minted_id(self.minted_model_invocation_jobs, now)
    }

    /// The `ModelInvocationJobArn` a `ModelInvocationJobIdentifier` names: the identifier
    /// itself when it is the ARN of a stored job, otherwise the ARN the job of that ID
    /// carries in `region`.
    ///
    /// The identifier's pattern is that ARN with its `model-invocation-job/<id>` tail, or
    /// the bare `<id>` alone
    /// (`smithy_query(query="operation",operation="GetModelInvocationJob")`), so both
    /// spellings address the same job — resolved exactly as
    /// [`Self::model_customization_job_arn_of`] resolves a customization job's two
    /// spellings, except that the bare spelling here is the minted id rather than a name.
    #[must_use]
    pub fn model_invocation_job_arn_of(&self, identifier: &str, region: &str) -> Option<String> {
        if self.model_invocation_jobs.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        let arn = model_invocation_job_arn(region, identifier);
        self.model_invocation_jobs.contains_key(&arn).then_some(arn)
    }

    /// The batch inference job a `ModelInvocationJobIdentifier` names, with every status
    /// transition the clock has made due at `now` materialised first — so every caller reads
    /// the status the job holds at `now`, whichever operation asked and however many times
    /// it has asked before ([`ModelInvocationJobRecord::settle`]).
    pub fn model_invocation_job(
        &mut self,
        identifier: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut ModelInvocationJobRecord> {
        let arn = self.model_invocation_job_arn_of(identifier, region)?;
        let record = self.model_invocation_jobs.get_mut(&arn)?;
        record.settle(now);
        Some(record)
    }

    /// Materialise the batch inference job transitions the clock has made due at `now`, so
    /// a listing reports — and its `statusEquals` filters on — the status each job holds at
    /// `now`, which is the status a read of that one job reports at the same instant: a
    /// listing observes every job it carries, exactly as a read observes one
    /// ([`ModelInvocationJobRecord::settle`]).
    pub fn settle_model_invocation_jobs(&mut self, now: SystemTime) {
        for record in self.model_invocation_jobs.values_mut() {
            record.settle(now);
        }
    }

    /// The runs the batch inference jobs of `region` still owe, taken ONCE
    /// ([`ModelInvocationJobRecord::due_run`]) and at most
    /// [`model_invocation_run::MAX_RUNS_PER_REQUEST`] of them per call.
    ///
    /// A listing reports the four record counts of every job it carries, and those counts
    /// are the run's own tally — so a listing takes the runs its jobs are owed exactly as a
    /// read of one job takes that job's, and the two cannot report different counts for one
    /// job. Jobs of another region are another account-region's resources and are not
    /// listed, so their runs are not taken here either.
    ///
    /// The number taken per call is BOUNDED because a listing settles every job of the
    /// Region while the work of one run is bounded by its own input: a job whose run this
    /// call does not take still owes it, so a later call takes it — and the iteration is
    /// lazy, so a run that is not taken is not marked as taken either.
    pub fn due_model_invocation_runs(&mut self, region: &str) -> Vec<model_invocation_run::DueRun> {
        self.model_invocation_jobs
            .values_mut()
            .filter(|record| record.region == region)
            .filter_map(ModelInvocationJobRecord::due_run)
            .take(model_invocation_run::MAX_RUNS_PER_REQUEST)
            .collect()
    }

    /// Whether a stored job in `region` is already customizing a model under
    /// `custom_model_name`.
    ///
    /// A job that was stopped is not: it produces no model, so the name it was going to
    /// give one is free again.
    #[must_use]
    pub fn customizes_model_named(&self, region: &str, custom_model_name: &str) -> bool {
        self.model_customization_jobs.values().any(|job| {
            job.region == region && job.output_model_name == custom_model_name && !job.is_stopped()
        })
    }

    /// Forget the Provisioned Throughput `arn` names, together with its name index entry
    /// and the idempotency tokens that minted it — so a delete leaves nothing behind for
    /// a later create of the same name to collide with, and the token map stays bounded by
    /// the LIVE Provisioned Throughputs.
    pub fn remove_provisioned_model_throughput(
        &mut self,
        arn: &str,
    ) -> Option<ProvisionedModelThroughputRecord> {
        let record = self.provisioned_model_throughputs.remove(arn)?;
        self.provisioned_model_names
            .remove(&(record.region.clone(), record.name.clone()));
        self.provisioned_model_create_tokens
            .retain(|_, minted| minted != arn);
        Some(record)
    }

    /// The resource a tagging call's `resourceARN` names, with every status transition the
    /// clock has made due at `now` materialised first — so a resource is tagged, and its
    /// tags reported, as it stands at `now`, exactly as a read of that one resource sees
    /// it.
    ///
    /// The ONE resolution of a tagging ARN, so every tagging operation reaches the same
    /// resource for the same value. A `TaggableResourcesArn` is the ARN of a resource of
    /// one of this service's families, or a bare resource NAME
    /// (`smithy_query(query="operation",operation="TagResource")`), and the resource TYPE
    /// inside the ARN picks the family; each family then resolves the value through the
    /// very index its own read operations resolve an identifier through, so tagging
    /// reaches exactly the resources those reads report. A value naming nothing this store
    /// holds resolves to `None`, whichever family it names.
    ///
    /// An application inference profile is addressable by the ARN it was minted with,
    /// resolved through [`Self::inference_profile_arn_of`] — the index
    /// `GetInferenceProfile` resolves an identifier through. (`TaggableResourcesArn`
    /// spells its own bare-name alternative `@length(min: 20)`, which no minted profile id
    /// reaches, so the ARN is the one spelling a tagging call can carry for it.)
    ///
    /// Two families are addressable under two ARN spellings, and both spellings name the
    /// same resource: a prompt router is `default-prompt-router/<id>` to `PromptRouterArn`
    /// and `prompt-router/<id>` here, and an imported model is `imported-model/<id>` to
    /// `ImportedModelArn` and `custom-model/imported/<id>` here. The second spelling is
    /// read back to the first — the one the store is keyed by — in the region the ARN
    /// itself carries, so neither spelling reaches another region's resource.
    ///
    /// An Automated Reasoning policy is addressable under either spelling its own
    /// `AutomatedReasoningPolicyArn` admits, and each spelling names its own REVISION: the
    /// unversioned ARN names the policy's draft and the `:<version>` suffix names one
    /// numbered version it cut (`TaggableResourcesArn` admits both,
    /// `:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?`). A revision carries the
    /// tags its own record holds — the list the create that minted it filled — so tagging
    /// the draft leaves a version's tags alone and the other way round, and a version
    /// nothing cut names no resource.
    ///
    /// The families this resolution does not reach (a custom model, a model invocation job,
    /// an async invoke) resolve to `None`, so a tagging call that names one is answered as
    /// naming no taggable resource of this account; wiring a family in is that family's own
    /// work, not a create's.
    pub fn tagged_resource(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<TaggedResource> {
        if resource_arn.contains(":guardrail/") {
            let id = guardrail_id_of(resource_arn).to_string();
            return self
                .guardrails
                .contains_key(&id)
                .then_some(TaggedResource::Guardrail(id));
        }
        if resource_arn.contains(":automated-reasoning-policy/") {
            // The ARN names a REVISION of a policy — its draft, or the version its
            // `:<version>` suffix selects — resolved through
            // [`Self::automated_reasoning_policy_revision`], the index every read of that
            // ARN resolves it through, so a tagging call reaches exactly the revisions those
            // reads report and each revision's tags are the ones its own record holds. The
            // number comes off the RESOLVED version rather than from a second read of the
            // suffix, so the two cannot disagree.
            let (policy, revision) = self.automated_reasoning_policy_revision(resource_arn)?;
            let policy_arn = policy.arn.clone();
            return Some(match revision {
                Some(version) => {
                    TaggedResource::AutomatedReasoningPolicyVersion(policy_arn, version.version)
                }
                None => TaggedResource::AutomatedReasoningPolicy(policy_arn),
            });
        }
        if resource_arn.contains(":default-prompt-router/") {
            return self
                .prompt_routers
                .contains_key(resource_arn)
                .then(|| TaggedResource::PromptRouter(resource_arn.to_string()));
        }
        if let Some((_, id)) = resource_arn.rsplit_once(":prompt-router/") {
            let arn = prompt_router_arn(arn_region(resource_arn).unwrap_or(region), id);
            return self
                .prompt_routers
                .contains_key(&arn)
                .then_some(TaggedResource::PromptRouter(arn));
        }
        if resource_arn.contains(":model-import-job/") {
            let job = self.model_import_job(resource_arn, region, now)?;
            return Some(TaggedResource::ModelImportJob(job.arn.clone()));
        }
        if resource_arn.contains(":imported-model/") {
            let (job, _) = self.imported_model(resource_arn, region, now)?;
            return Some(TaggedResource::ImportedModel(job.arn.clone()));
        }
        if let Some((_, id)) = resource_arn.rsplit_once(":custom-model/imported/") {
            let spelled = imported_model_arn(arn_region(resource_arn).unwrap_or(region), id);
            let (job, _) = self.imported_model(&spelled, region, now)?;
            return Some(TaggedResource::ImportedModel(job.arn.clone()));
        }
        if resource_arn.contains(":evaluation-job/") {
            let job = self.evaluation_job(resource_arn, now)?;
            return Some(TaggedResource::EvaluationJob(job.arn.clone()));
        }
        if resource_arn.contains(":provisioned-model/") {
            let record = self.provisioned_model_throughput(resource_arn, region, now)?;
            return Some(TaggedResource::ProvisionedModelThroughput(
                record.arn.clone(),
            ));
        }
        if resource_arn.contains(&format!(":{CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE}/")) {
            let record = self.custom_model_deployment(resource_arn, region, now)?;
            return Some(TaggedResource::CustomModelDeployment(record.arn.clone()));
        }
        if resource_arn.contains(":application-inference-profile/") {
            let arn = self.inference_profile_arn_of(resource_arn, region)?;
            return Some(TaggedResource::InferenceProfile(arn));
        }
        if !resource_arn.starts_with("arn:") {
            // A bare resource NAME, which `TaggableResourcesArn` admits beside an ARN: it
            // names at most one resource of a family whose own identifier admits a name,
            // in this account and region, so each of those families is asked in turn.
            if let Some(job) = self.model_import_job(resource_arn, region, now) {
                return Some(TaggedResource::ModelImportJob(job.arn.clone()));
            }
            if let Some((job, _)) = self.imported_model(resource_arn, region, now) {
                return Some(TaggedResource::ImportedModel(job.arn.clone()));
            }
            if let Some(record) = self.provisioned_model_throughput(resource_arn, region, now) {
                return Some(TaggedResource::ProvisionedModelThroughput(
                    record.arn.clone(),
                ));
            }
            if let Some(record) = self.custom_model_deployment(resource_arn, region, now) {
                return Some(TaggedResource::CustomModelDeployment(record.arn.clone()));
            }
        }
        None
    }

    /// The tags a resolved resource carries, read off the record that holds the resource —
    /// the same list the create that minted it filled and its own read reports, never a
    /// second copy.
    #[must_use]
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<&[Tag]> {
        match resource {
            TaggedResource::Guardrail(id) => {
                self.guardrails.get(id).map(|guardrail| &guardrail.tags)
            }
            TaggedResource::AutomatedReasoningPolicy(arn) => {
                self.automated_reasoning_policies.get(arn).map(|policy| &policy.tags)
            }
            TaggedResource::AutomatedReasoningPolicyVersion(arn, number) => self
                .automated_reasoning_policies
                .get(arn)
                .and_then(|policy| policy.versions.get(number))
                .map(|version| &version.tags),
            TaggedResource::PromptRouter(arn) => {
                self.prompt_routers.get(arn).map(|router| &router.tags)
            }
            TaggedResource::ModelImportJob(arn) => {
                self.model_import_jobs.get(arn).map(|job| &job.job_tags)
            }
            TaggedResource::ImportedModel(arn) => self
                .model_import_jobs
                .get(arn)
                .map(|job| &job.imported_model_tags),
            TaggedResource::EvaluationJob(arn) => self.evaluation_jobs.get(arn).map(|job| &job.tags),
            TaggedResource::ProvisionedModelThroughput(arn) => self
                .provisioned_model_throughputs
                .get(arn)
                .map(|record| &record.tags),
            TaggedResource::CustomModelDeployment(arn) => self
                .custom_model_deployments
                .get(arn)
                .map(|record| &record.tags),
            TaggedResource::InferenceProfile(arn) => {
                self.inference_profiles.get(arn).map(|record| &record.tags)
            }
        }
        .map(Vec::as_slice)
    }

    /// The tags a resolved resource carries, for a tagging call to rewrite in place.
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<Tag>> {
        match resource {
            TaggedResource::Guardrail(id) => self
                .guardrails
                .get_mut(id)
                .map(|guardrail| &mut guardrail.tags),
            TaggedResource::AutomatedReasoningPolicy(arn) => self
                .automated_reasoning_policies
                .get_mut(arn)
                .map(|policy| &mut policy.tags),
            TaggedResource::AutomatedReasoningPolicyVersion(arn, number) => self
                .automated_reasoning_policies
                .get_mut(arn)
                .and_then(|policy| policy.versions.get_mut(number))
                .map(|version| &mut version.tags),
            TaggedResource::PromptRouter(arn) => self
                .prompt_routers
                .get_mut(arn)
                .map(|router| &mut router.tags),
            TaggedResource::ModelImportJob(arn) => self
                .model_import_jobs
                .get_mut(arn)
                .map(|job| &mut job.job_tags),
            TaggedResource::ImportedModel(arn) => self
                .model_import_jobs
                .get_mut(arn)
                .map(|job| &mut job.imported_model_tags),
            TaggedResource::EvaluationJob(arn) => {
                self.evaluation_jobs.get_mut(arn).map(|job| &mut job.tags)
            }
            TaggedResource::ProvisionedModelThroughput(arn) => self
                .provisioned_model_throughputs
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::CustomModelDeployment(arn) => self
                .custom_model_deployments
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::InferenceProfile(arn) => self
                .inference_profiles
                .get_mut(arn)
                .map(|record| &mut record.tags),
        }
    }

    /// The tags the resource a tagging call's `resourceARN` names carries, for REPORTING.
    pub fn resource_tags(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&[Tag]> {
        let resource = self.tagged_resource(resource_arn, region, now)?;
        self.tags_of(&resource)
    }

    /// The tags the resource a tagging call's `resourceARN` names carries, for WRITING.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut Vec<Tag>> {
        let resource = self.tagged_resource(resource_arn, region, now)?;
        self.tags_of_mut(&resource)
    }

    /// The resource a resource-policy call's `resourceArn` names, or `None` when this
    /// account holds no such resource.
    ///
    /// A resource-based policy is attached TO a resource, so the value is resolved through
    /// [`Self::tagged_resource`] — the ONE resolution of a caller-supplied resource ARN in
    /// this service, so a policy reaches exactly the resources a tagging call and that
    /// resource's own reads reach, including the second ARN spelling two of the families
    /// are addressable under. `ResourcePolicyResourceArn` is documented as "the ARN of the
    /// Bedrock resource" (`smithy_query(query="operation",operation="PutResourcePolicy")`)
    /// and declares no `@pattern`, so the ARN form is required HERE: a bare resource name,
    /// which a tagging call admits, is no ARN of this service's namespace and names
    /// nothing to attach a policy to.
    pub fn policy_resource(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<TaggedResource> {
        ::service_common::arn::parse_of(resource_arn, ARN_NAMESPACE)?;
        self.tagged_resource(resource_arn, region, now)
    }

    /// The resource policy attached to the resource a resource-policy call's `resourceArn`
    /// names, or `None` when this account holds no such resource or that resource holds no
    /// policy.
    ///
    /// The document is handed back EXACTLY as it was stored — a policy is a document the
    /// caller wrote, not a shape this emulator re-renders.
    pub fn resource_policy(
        &mut self,
        resource_arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&str> {
        let resource = self.policy_resource(resource_arn, region, now)?;
        self.resource_policies.get(&resource).map(String::as_str)
    }

    /// Attach `document` to `resource`, REPLACING the policy that resource already holds —
    /// a resource holds at most one resource policy, which is what makes the put
    /// idempotent.
    ///
    /// Every entry whose resource this store no longer holds is dropped first (a resolved
    /// resource's record is present exactly when the tags read off it are), so the map
    /// stays bounded by the live resources however many attach and delete cycles run.
    pub fn attach_resource_policy(&mut self, resource: TaggedResource, document: String) {
        self.forget_policies_of_gone_resources();
        self.resource_policies.insert(resource, document);
    }

    /// Detach the policy `resource` holds, reporting the document that was in force (or
    /// `None` when that resource held none).
    ///
    /// The counterpart of [`Self::attach_resource_policy`], and bounded the same way: the
    /// entries whose resource this store no longer holds are dropped here too, so a detach
    /// cannot leave a document behind for a resource that is gone.
    pub fn detach_resource_policy(&mut self, resource: &TaggedResource) -> Option<String> {
        self.forget_policies_of_gone_resources();
        self.resource_policies.remove(resource)
    }

    /// Drop every attached policy whose resource this store no longer holds.
    ///
    /// A resolved resource's record is present exactly when the tags read off it are, so a
    /// resource whose record is gone can hold no policy: keeping its document would report
    /// a grant on a resource that no longer exists and would let the map grow without
    /// bound across create/delete cycles.
    pub(super) fn forget_policies_of_gone_resources(&mut self) {
        let gone: Vec<TaggedResource> = self
            .resource_policies
            .keys()
            .filter(|attached| self.tags_of(attached).is_none())
            .cloned()
            .collect();
        for attached in gone {
            self.resource_policies.remove(&attached);
        }
    }
}

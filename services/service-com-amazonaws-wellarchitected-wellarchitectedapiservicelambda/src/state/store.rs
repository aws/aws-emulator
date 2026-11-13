// SPDX-License-Identifier: CC0-1.0

use super::*;

impl ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// A fresh `WorkloadId`: 32 lowercase hex characters, as
    /// `^[0-9a-f]{32}$` requires, and unused by any stored workload.
    ///
    /// Derived from a per-store counter and the wall clock — never from the request —
    /// so two identical requests still get distinct ids.
    pub fn mint_workload_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.workloads.contains_key(&id) {
                return id;
            }
        }
    }

    /// A fresh review-template ARN: `review-template/<32 lowercase hex>` in `region`, the
    /// form the modelled `TemplateArn` pattern requires, and unused by any stored
    /// template. The ARN is what [`Self::review_templates`] is keyed by, so the collision
    /// check is ONE lookup — and exact, where a suffix match over the stored ARNs could
    /// also collide across Regions.
    pub fn mint_review_template_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn =
                ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("review-template/{id}"));
            if !self.review_templates.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh `ProfileArn`: `profile/<32 lowercase hex>` in `region`, which satisfies
    /// the modelled `ProfileArn` pattern's `profile/[a-z0-9]+` resource part, and unused
    /// by any stored profile. The ARN is what [`Self::profiles`] is keyed by, so the
    /// collision check is ONE lookup — and exact, where a suffix match over the stored
    /// ARNs could also collide across Regions.
    ///
    /// The resource part is a minted id rather than the profile's name: the name is a
    /// mutable member (`UpdateProfile`), and an ARN identifies the resource for its whole
    /// life.
    pub fn mint_profile_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("profile/{id}"));
            if !self.profiles.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh `ShareId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$` requires, and
    /// unused by any share this store holds. A `ShareId` identifies the share on its own
    /// (`DeleteWorkloadShare` and `DeleteLensShare` each resolve one by id), so uniqueness
    /// is checked over every share of every SHARED RESOURCE rather than within one of them
    /// — a workload's and a lens's shares live in one id space.
    pub fn mint_share_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.share_id_taken(&id) {
                return id;
            }
        }
    }

    /// Whether any stored share already carries `share_id`.
    fn share_id_taken(&self, share_id: &str) -> bool {
        self.workload_shares
            .values()
            .any(|shares| shares.contains_key(share_id))
            || self
                .lens_shares
                .values()
                .any(|shares| shares.contains_key(share_id))
            || self
                .profile_shares
                .values()
                .any(|shares| shares.contains_key(share_id))
            || self
                .template_shares
                .values()
                .any(|shares| shares.contains_key(share_id))
    }

    /// A fresh `ShareInvitationId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$`
    /// requires, and unused by any invitation this store holds. An invitation is named by
    /// this id alone (`UpdateShareInvitation` resolves one by it), so uniqueness is
    /// checked over every share of every shared resource — and against the `ShareId`s too,
    /// so the two id spaces of one share can never hand out the same value and be mistaken
    /// for each other.
    pub fn mint_share_invitation_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            let taken = self.share_id_taken(&id)
                || self
                    .workload_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id)
                || self
                    .lens_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id)
                || self
                    .profile_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id)
                || self
                    .template_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id);
            if !taken {
                return id;
            }
        }
    }

    /// A fresh lens id: 32 lowercase hex characters, the form the `LensAlias` doc's
    /// custom lens ARN ends in (`…:lens/0123456789abcdef01234567890abcdef`), and unused
    /// by any stored lens. Minted the same way, from the store and the clock.
    pub fn mint_lens_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.lenses.contains_key(&id) {
                return id;
            }
        }
    }

    /// A fresh `AgentProfileArn`: `agent-profile/<32 lowercase hex>` in `region`, which
    /// satisfies the modelled pattern's `agent-profile/([a-zA-Z0-9_-]+)` resource part,
    /// and unused by any stored agent profile.
    ///
    /// The resource part is a minted id rather than the profile's `name`: the name is a
    /// mutable member (`UpdateAgentProfile`), and an ARN identifies the resource for its
    /// whole life.
    pub fn mint_agent_profile_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("agent-profile/{id}"));
            if !self.agent_profiles.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh goal id: 32 lowercase hex characters in the five dashed groups the
    /// modelled `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any goal this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's goals, not only one profile's: a goal
    /// id identifies a goal, so two profiles handing out the same one would make the id
    /// ambiguous in anything that reports goals side by side.
    pub fn mint_agent_goal_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_goals
                .values()
                .any(|goals| goals.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The next candidate id in the dashed form the modelled `UUID` pattern requires,
    /// from [`Self::next_hex_id`]'s 32 hex characters. The callers above are what check a
    /// candidate against the resources they mint for.
    fn next_uuid(&mut self) -> String {
        let hex = self.next_hex_id();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The goal `id` of the agent profile `profile_arn` names, if that profile holds one.
    pub fn agent_goal(&self, profile_arn: &str, id: &str) -> Option<&AgentGoalRecord> {
        self.agent_goals.get(profile_arn)?.get(id)
    }

    /// Drop one goal of an agent profile and everything keyed to it: any create or update
    /// token that resolved to it, and the profile's goal map once its last goal is gone,
    /// so an emptied profile leaves nothing behind.
    pub fn remove_agent_goal(&mut self, profile_arn: &str, id: &str) -> Option<AgentGoalRecord> {
        let goals = self.agent_goals.get_mut(profile_arn)?;
        let record = goals.remove(id)?;
        if goals.is_empty() {
            self.agent_goals.remove(profile_arn);
        }
        self.create_agent_goal_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.goal_id == id));
        self.update_agent_goal_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.goal_id == id));
        Some(record)
    }

    /// [`Self::agent_goal`] for a caller that is about to write to the goal.
    pub fn agent_goal_mut(&mut self, profile_arn: &str, id: &str) -> Option<&mut AgentGoalRecord> {
        self.agent_goals.get_mut(profile_arn)?.get_mut(id)
    }

    /// How many goals the agent profile `profile_arn` names currently holds — what
    /// [`MAX_GOALS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_goal_count(&self, profile_arn: &str) -> usize {
        self.agent_goals
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// A fresh context id, in the five dashed groups the modelled `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any context this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's contexts, for the reason
    /// [`Self::mint_agent_goal_id`] gives: a context id identifies a context, so two
    /// profiles handing out the same one would make the id ambiguous.
    pub fn mint_agent_context_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_contexts
                .values()
                .any(|contexts| contexts.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The context `id` of the agent profile `profile_arn` names, if that profile holds
    /// one.
    pub fn agent_context(&self, profile_arn: &str, id: &str) -> Option<&AgentContextRecord> {
        self.agent_contexts.get(profile_arn)?.get(id)
    }

    /// [`Self::agent_context`] for a caller that is about to write to the context.
    pub fn agent_context_mut(
        &mut self,
        profile_arn: &str,
        id: &str,
    ) -> Option<&mut AgentContextRecord> {
        self.agent_contexts.get_mut(profile_arn)?.get_mut(id)
    }

    /// Drop one context of an agent profile and everything keyed to it, exactly as
    /// [`Self::remove_agent_goal`] does for a goal: any create or update token that
    /// resolved to it, and the profile's context map once its last context is gone, so an
    /// emptied profile leaves nothing behind.
    pub fn remove_agent_context(
        &mut self,
        profile_arn: &str,
        id: &str,
    ) -> Option<AgentContextRecord> {
        let contexts = self.agent_contexts.get_mut(profile_arn)?;
        let record = contexts.remove(id)?;
        if contexts.is_empty() {
            self.agent_contexts.remove(profile_arn);
        }
        self.create_agent_context_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.context_id == id));
        self.update_agent_context_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.context_id == id));
        Some(record)
    }

    /// How many contexts the agent profile `profile_arn` names currently holds — what
    /// [`MAX_CONTEXTS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_context_count(&self, profile_arn: &str) -> usize {
        self.agent_contexts
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// A fresh recommendation generation id, in the five dashed groups the modelled
    /// `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any generation this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's generations, for the reason
    /// [`Self::mint_agent_goal_id`] gives: the id identifies the generation, so two
    /// profiles handing out the same one would make it ambiguous.
    pub fn mint_agent_generation_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_generations
                .values()
                .any(|generations| generations.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The recommendation generation `id` of the agent profile `profile_arn` names, if
    /// that profile holds one. A generation is reached only THROUGH its profile, so the
    /// pair resolves in one lookup.
    pub fn agent_generation(&self, profile_arn: &str, id: &str) -> Option<&AgentGenerationRecord> {
        self.agent_generations.get(profile_arn)?.get(id)
    }

    /// How many recommendation generations the agent profile `profile_arn` names holds —
    /// what [`MAX_GENERATIONS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_generation_count(&self, profile_arn: &str) -> usize {
        self.agent_generations
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// The generation of `profile_arn` that the request fingerprinted `fingerprint`
    /// started and that has not reached a terminal status as of `now`, if there is one.
    ///
    /// `StartAgentRecommendationGeneration` is modelled `@idempotent`
    /// (`smithy_query(query="operation",
    /// operation="StartAgentRecommendationGeneration")`), so repeating a start request
    /// must not leave a second job running: the repeat resolves to the job already in
    /// flight. Once that job is terminal the same request starts a fresh analysis, which
    /// is what asking again then means.
    pub fn in_flight_agent_generation(
        &self,
        profile_arn: &str,
        fingerprint: &str,
        now: SystemTime,
    ) -> Option<&AgentGenerationRecord> {
        self.agent_generations.get(profile_arn)?.values().find(|record| {
            record.fingerprint == fingerprint && !record.is_terminal(now)
        })
    }

    /// A fresh `RecommendationArn`: `agent-recommendation/<UUID>` in `region`, the form the
    /// modelled `AgentRecommendationArn` / `RecommendationArn` pattern requires
    /// (`…:agent-recommendation/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`),
    /// and unused by any recommendation this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's recommendations: the ARN names the
    /// recommendation on its own — `UpdateAgentRecommendationStatus` and
    /// `GetAgentRecommendation` resolve one by ARN alone, with no profile in the path — so
    /// two profiles handing out the same one would make it ambiguous.
    pub fn mint_agent_recommendation_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_uuid();
            let arn = ::engine::emulator_arn(
                ARN_NAMESPACE,
                region,
                &format!("agent-recommendation/{id}"),
            );
            if !self
                .agent_recommendations
                .values()
                .any(|recommendations| recommendations.contains_key(&arn))
            {
                return arn;
            }
        }
    }

    /// The recommendation `arn` names, if this store holds one.
    ///
    /// A `RecommendationArn` identifies the recommendation on its own —
    /// `GetAgentRecommendation` carries no profile in its path — so the lookup spans every
    /// profile's recommendations: one map lookup per profile, bounded by
    /// [`MAX_AGENT_PROFILES`], and exact, because the inner maps are keyed by the ARN
    /// itself. A listing reaches a profile's recommendations through
    /// [`Self::agent_recommendations`] directly, because it is enumerating them rather
    /// than resolving one.
    pub fn agent_recommendation(&self, arn: &str) -> Option<&AgentRecommendationRecord> {
        self.agent_recommendations
            .values()
            .find_map(|recommendations| recommendations.get(arn))
    }

    /// [`Self::agent_recommendation`] for a caller that is about to write to the
    /// recommendation: `UpdateAgentRecommendationStatus`, which moves its status, and
    /// `PutAgentRecommendationFeedback`, which puts its feedback sub-resource.
    pub fn agent_recommendation_mut(
        &mut self,
        arn: &str,
    ) -> Option<&mut AgentRecommendationRecord> {
        self.agent_recommendations
            .values_mut()
            .find_map(|recommendations| recommendations.get_mut(arn))
    }

    /// Record the recommendations of every generation whose analysis the clock has
    /// finished and whose results are not recorded yet, as of `now`.
    ///
    /// A generation "analyzes your Amazon Web Services resources and generates optimization
    /// recommendations based on the configured pillars and scope", and its results are
    /// ready when it reaches `COMPLETED` ("poll this operation until status reaches a
    /// terminal value of COMPLETED (results are ready)" —
    /// `GetAgentRecommendationGeneration` doc). The completion is owed by the CLOCK
    /// ([`AgentGenerationRecord::ends_at`]), and nothing in this emulator runs between
    /// requests, so the recommendations it produced are materialised by the first operation
    /// that goes looking for them — the write a reporting operation is allowed to make,
    /// because it was due at that instant whether or not anybody called.
    ///
    /// One recommendation per pillar the generation's scope named, which is the same
    /// reading of a scope as its progress ([`AgentGenerationRecord::progress`]): one scoped
    /// pillar is one step of the analysis, and the finding that step produced is the
    /// catalog's content for that pillar. A pillar the catalog publishes no finding for
    /// yields no recommendation, so the table is the only place that decides what a
    /// generation can produce.
    ///
    /// IDEMPOTENT: a generation whose recommendations are already recorded is skipped, so
    /// however many times this runs, one generation's analysis produces its results once.
    pub fn materialise_due_agent_recommendations(&mut self, now: SystemTime) {
        // What each due generation produces, resolved while the generations are only read,
        // so the minting below can take the store mutably.
        let due: Vec<(
            String,
            String,
            String,
            Vec<&'static RecommendationContent>,
            SystemTime,
        )> = self
            .agent_generations
            .iter()
            .flat_map(|(profile_arn, generations)| {
                generations
                    .values()
                    .map(move |generation| (profile_arn, generation))
            })
            .filter(|(profile_arn, generation)| {
                generation.is_terminal(now)
                    && !self.generation_has_recommendations(profile_arn, &generation.id)
            })
            .filter_map(|(profile_arn, generation)| {
                Some((
                    profile_arn.clone(),
                    generation.id.clone(),
                    generation.types.first()?.clone(),
                    generation
                        .scope
                        .pillars
                        .iter()
                        .filter_map(|pillar| recommendation_content(pillar))
                        .collect(),
                    generation.ends_at,
                ))
            })
            .collect();

        for (profile_arn, generation_id, recommendation_type, findings, completed_at) in due {
            // A recommendation of a profile is a resource of the same account and Region as
            // that profile, so its ARN is minted in the Region the profile's own ARN names
            // rather than in the Region of whichever request happened to materialise it.
            let region = arn_region(&profile_arn).to_string();
            for content in findings {
                let arn = self.mint_agent_recommendation_arn(&region);
                let record = AgentRecommendationRecord {
                    arn: arn.clone(),
                    profile_arn: profile_arn.clone(),
                    generation_id: generation_id.clone(),
                    content,
                    recommendation_type: recommendation_type.clone(),
                    // "The current status of the recommendation": a recommendation nobody
                    // has acted on yet is the active one of the lifecycle
                    // `UpdateAgentRecommendationStatus` moves it through.
                    status: RECOMMENDATION_ACTIVE.to_string(),
                    update_reason: None,
                    // "The identifier of the user or system that created this
                    // recommendation": the analysis is the SERVICE's work, so it is
                    // attributed to this emulator's one account, exactly as the generation
                    // that produced it is.
                    created_by: ::engine::emulator_account_id().to_string(),
                    // The instant the analysis finished — when the recommendation came
                    // into being — rather than the instant some later read noticed.
                    created_at: completed_at,
                    last_modified_by: None,
                    last_modified_at: None,
                    // Nobody has judged the finding yet: the feedback sub-resource exists
                    // only once a caller submits one.
                    feedback: None,
                };
                self.agent_recommendations
                    .entry(profile_arn.clone())
                    .or_default()
                    .insert(arn, record);
            }
        }
    }

    /// Whether the generation `generation_id` of `profile_arn` has already had its
    /// recommendations recorded.
    ///
    /// Read off the recommendations themselves rather than off a flag beside them, so there
    /// is ONE record of what a generation produced: every generation this emulator accepts
    /// scopes at least one pillar (`Scope$pillars` is modelled `@length(1, 5)`), so a
    /// generation whose results are recorded always has at least one recommendation to find.
    fn generation_has_recommendations(&self, profile_arn: &str, generation_id: &str) -> bool {
        self.agent_recommendations
            .get(profile_arn)
            .is_some_and(|recommendations| {
                recommendations
                    .values()
                    .any(|record| record.generation_id == generation_id)
            })
    }

    /// The tags recorded on the resource an ARN names, or `None` when it names no
    /// resource this store holds.
    ///
    /// A tag operation names its resource by ARN ALONE, across every taggable resource
    /// type of the service, so ONE lookup spans them all:
    /// the workloads, the custom lenses, the Well-Architected Tool profiles, the review
    /// templates and the agent profiles. Resolved in one place so every operation that
    /// reads or writes a resource's tags agrees about what an ARN names.
    ///
    /// A workload is keyed in this store by its `WorkloadId` rather than by its ARN, so
    /// its ARN is matched over the stored workloads — a scan bounded by
    /// [`MAX_WORKLOADS_PER_ACCOUNT_REGION`]; every other type is keyed by the ARN itself,
    /// so it costs ONE lookup. A lens is resolved through
    /// [`Self::lens_by_alias`], which requires the ARN to agree with the lens's own
    /// (account and Region included) and accepts a version-pinned ARN as naming the same
    /// lens, and it is consulted only for a string that IS an ARN: a bare lens id is not
    /// an ARN, so it names no resource here.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.tagged_resource(resource_arn)? {
            TaggedResource::Workload(workload_id) => {
                self.workloads.get(&workload_id).map(|record| &record.tags)
            }
            TaggedResource::ReviewTemplate => self
                .review_templates
                .get(resource_arn)
                .map(|record| &record.tags),
            TaggedResource::Profile => self.profiles.get(resource_arn).map(|record| &record.tags),
            TaggedResource::AgentProfile => self
                .agent_profiles
                .get(resource_arn)
                .map(|record| &record.tags),
            TaggedResource::Lens => self.lens_by_alias(resource_arn).map(|record| &record.tags),
        }
    }

    /// [`Self::resource_tags`] for a caller that is about to CHANGE the resource's tags —
    /// the same resolution, so a tag write lands on exactly the resource a tag read
    /// reports, and an ARN naming no resource this store holds writes nothing.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match self.tagged_resource(resource_arn)? {
            TaggedResource::Workload(workload_id) => self
                .workloads
                .get_mut(&workload_id)
                .map(|record| &mut record.tags),
            TaggedResource::ReviewTemplate => self
                .review_templates
                .get_mut(resource_arn)
                .map(|record| &mut record.tags),
            TaggedResource::Profile => self
                .profiles
                .get_mut(resource_arn)
                .map(|record| &mut record.tags),
            TaggedResource::AgentProfile => self
                .agent_profiles
                .get_mut(resource_arn)
                .map(|record| &mut record.tags),
            TaggedResource::Lens => self
                .lens_by_alias_mut(resource_arn)
                .map(|record| &mut record.tags),
        }
    }

    /// Whether the resource this ARN names also reports its stored tags through the
    /// modelled `Tag` STRUCTURE, not only through the `TagMap` a tag operation carries.
    ///
    /// An agent profile is the one resource of this store whose own reads project its tag
    /// map that way ([`AgentProfileRecord::tag_list`], reported by `CreateAgentProfile`,
    /// `GetAgentProfile`, `UpdateAgentProfile` and `ListAgentProfiles`); every other
    /// taggable resource reports its tags only as a `TagMap`, which asks nothing of a value
    /// beyond what the request boundary already applied. What that difference COSTS a tag
    /// write is [`tag_structure_refusal`]; this is only which resources it applies to, so a
    /// resource type added to [`TaggedResource`] has to answer the question.
    pub fn tags_reported_as_tag_structures(&self, resource_arn: &str) -> bool {
        match self.tagged_resource(resource_arn) {
            Some(TaggedResource::AgentProfile) => true,
            Some(
                TaggedResource::Workload(_)
                | TaggedResource::ReviewTemplate
                | TaggedResource::Profile
                | TaggedResource::Lens,
            )
            | None => false,
        }
    }

    /// Which resource of this store, if any, the ARN a tag operation named holds its tags
    /// on — the resolution [`Self::resource_tags`] and [`Self::resource_tags_mut`] share.
    fn tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some((workload_id, _)) = self
            .workloads
            .iter()
            .find(|(_, record)| record.workload_arn == resource_arn)
        {
            return Some(TaggedResource::Workload(workload_id.clone()));
        }
        if self.review_templates.contains_key(resource_arn) {
            return Some(TaggedResource::ReviewTemplate);
        }
        if self.profiles.contains_key(resource_arn) {
            return Some(TaggedResource::Profile);
        }
        if self.agent_profiles.contains_key(resource_arn) {
            return Some(TaggedResource::AgentProfile);
        }
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        self.lens_by_alias(resource_arn)
            .is_some()
            .then_some(TaggedResource::Lens)
    }

    /// The next 32-lowercase-hex candidate id: a per-store counter and the wall clock,
    /// hashed, so it is never derived from the request. The callers above are what check
    /// a candidate against the resources they mint for.
    fn next_hex_id(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(self.id_counter.to_le_bytes());
        hasher.update(nanos.to_le_bytes());
        hex::encode(&hasher.finalize()[..16])
    }

    /// The stored lens a `LensAlias` names, if one is stored.
    ///
    /// A custom lens is named by its ARN, whose resource part is `lens/<lens id>`, and
    /// [`lens_alias`] is what reduces either spelling to that id — the lens's identity
    /// in this store. An ARN names more than the id, though: a lens belongs to the
    /// account and Region its ARN carries, so an ARN that agrees on the id and disagrees
    /// on either names a DIFFERENT lens and resolves to nothing. A version-pinned ARN
    /// (`…:lens/<id>/<version>`) names the same lens. A name that is not an ARN resolves
    /// to nothing at all ([`alias_names_lens`]): it is an AWS-official lens's alias
    /// ([`is_aws_official_lens`]), and this store holds only custom lenses.
    pub fn lens_by_alias(&self, alias: &str) -> Option<&LensRecord> {
        let record = self.lenses.get(lens_alias(alias))?;
        alias_names_lens(alias, record).then_some(record)
    }

    /// The CONTENT of the lens a request named, AS OF the version a review of it is
    /// against — the pillars it is organized into and the questions it asks — resolved from
    /// the WHOLE lens catalog.
    ///
    /// A lens review reports and counts what the LENS's content defines, and this store
    /// holds that content in two halves that never overlap: an AWS-official lens (named by
    /// its alias or by its account-less ARN, [`is_aws_official_lens`]) is its catalog row
    /// ([`official_lens`]), and a custom lens is the pillars and questions the document of
    /// one of its published versions defines ([`LensRecord::content_at`]). This is the ONE
    /// place that resolution lives, so a listing of a review's answers, a risk or question
    /// roll-up, a pillar summary and the judgement of a `PillarId` a request supplies
    /// cannot disagree about what a lens asks.
    ///
    /// `version` is the `LensVersion` the review is PINNED at, which is what makes a lens
    /// review a review against ONE version of the lens: a custom lens's published versions
    /// each carry their own document, so a later publish does not move the questions a
    /// review already asks until an upgrade moves the review to it. An AWS-official lens
    /// carries exactly one version here — the catalog row IS its content — so the pin
    /// selects nothing further for it, exactly as [`Self::lens_name_at`] reads its name
    /// from that row whatever version is named.
    ///
    /// Empty ([`LensContentView::is_empty`]) only for a lens this emulator holds no content
    /// for at all: an official alias the catalog carries no questions for, a custom lens
    /// this store does not hold, or one no version has been published from — and an
    /// unpublished custom lens cannot be applied to a workload or a review template in the
    /// first place ([`Self::unapplicable_lenses_refusal`]).
    pub fn lens_content_at(&self, lens: &str, version: Option<&str>) -> LensContentView {
        if is_aws_official_lens(lens) {
            return official_lens(lens).map_or_else(LensContentView::default, |official| {
                LensContentView::new(
                    official
                        .pillars()
                        .into_iter()
                        .map(|(pillar_id, pillar_name, _)| {
                            (pillar_id.to_string(), pillar_name.to_string())
                        })
                        .collect(),
                    official
                        .questions
                        .iter()
                        .map(FrameworkQuestion::to_review_question)
                        .collect(),
                )
            });
        }
        self.lens_by_alias(lens)
            .and_then(|record| record.content_at(version))
            .map_or_else(LensContentView::default, |content| {
                LensContentView::new(
                    content
                        .definition
                        .pillars
                        .iter()
                        .map(|pillar| (pillar.id.clone(), pillar.name.clone()))
                        .collect(),
                    content.definition.review_questions(),
                )
            })
    }

    /// The content of every lens ONE reviewed resource is reviewed against — a workload's
    /// lens reviews, a review template's, or those of the workload a milestone recorded —
    /// resolved in one pass through [`Self::lens_content_at`], so every projection of that
    /// resource reads the same answer to "what does this lens ask?".
    ///
    /// `reviewed` is each lens as the reviewed resource records it beside the `LensVersion`
    /// its review of that lens is against, which is what
    /// [`WorkloadRecord::reviewed_lens_versions`] and
    /// [`ReviewTemplateRecord::reviewed_lens_versions`] project: the version is part of the
    /// question, because a lens review is a review against ONE version of the lens.
    pub fn reviewed_lens_content(
        &self,
        reviewed: &[(&str, Option<&str>)],
    ) -> ReviewedLensContent {
        ReviewedLensContent {
            lenses: reviewed
                .iter()
                .map(|(lens, version)| {
                    (
                        lens_alias(lens).to_string(),
                        self.lens_content_at(lens, *version),
                    )
                })
                .collect(),
        }
    }

    /// The questions the lens a request named asks as of the version a review of it is
    /// against, in the lens's own order — the ONE question set every read of a lens
    /// review's answers reports and pages over, from the one place that resolution lives
    /// ([`Self::lens_content_at`]).
    pub fn lens_review_questions(
        &self,
        lens: &str,
        version: Option<&str>,
    ) -> Vec<ReviewQuestion> {
        self.lens_content_at(lens, version).into_questions()
    }

    /// The ONE question the lens a request named asks under `question_id` as of the version
    /// a review of it is against, when it asks it: [`Self::lens_review_questions`] resolved
    /// by id, which is how every operation that reads a single answer names the question.
    pub fn lens_review_question(
        &self,
        lens: &str,
        version: Option<&str>,
        question_id: &str,
    ) -> Option<ReviewQuestion> {
        self.lens_review_questions(lens, version)
            .into_iter()
            .find(|question| question.question_id == question_id)
    }

    /// Resolve the ONE best practice of ONE lens review of ONE workload a read of its
    /// Trusted Advisor checks names, and the checks associated to it — or the refusal that
    /// read answers with ([`BestPracticeRefusal`]).
    ///
    /// This is the ONE place that walk lives, so every operation that reports those checks
    /// — the per-account view and the summarized one — resolves the same request to the
    /// same best practice and refuses it identically, and a change to the refusal order, to
    /// the version the lens content is resolved at or to the activation gate reaches both.
    /// Each sub-rule it composes is single-homed beside it; what lives here is the SEQUENCE:
    ///
    /// * the workload the request names ([`workload_not_found`]), then its review of the
    ///   lens ([`WorkloadRecord::reviews_lens`], [`lens_review_not_found`]) — a check is
    ///   associated to a lens (`CheckDetail$LensArn` / `CheckSummary$LensArn`) and that
    ///   association is read through the workload's review of it, so a lens the workload
    ///   does not carry has no review whose best practices could be checked;
    /// * then `PillarId`, `QuestionId` and `ChoiceId`, which name one best practice OF THAT
    ///   LENS and so are judged against the lens's own content as of the version this
    ///   workload's review of it is pinned at ([`Self::lens_content_at`]), outermost first:
    ///   the pillar the lens defines ([`undefined_pillar_refusal`]), the question that
    ///   pillar asks ([`undefined_pillar_question_refusal`]) and the choice that question
    ///   offers ([`undefined_choice_member_refusal`]). They are values of the request rather
    ///   than resources of their own — the resources this read resolves are the workload and
    ///   its lens review — so a value the lens's content does not define is refused as a
    ///   field validation, the same rule every operation that takes a pillar or a question
    ///   of a lens review applies. The content comes from the whole lens catalog, so a
    ///   pillar and a question of a CUSTOM lens are judged against the document that version
    ///   published exactly as an official lens's are against its catalog row.
    ///
    /// A lens this emulator holds no content for asks nothing, so those refusals take the
    /// request's identifiers as given and there is no question to resolve: such a lens has
    /// no checked best practice either, so no check is reported. The `ChoiceId` is therefore
    /// judged on exactly the lenses whose content can judge it, which is the rule the pillar
    /// and question refusals already apply.
    ///
    /// A workload whose Trusted Advisor integration is not activated
    /// ([`WorkloadRecord::trusted_advisor_integration_enabled`]) has none of the checks
    /// fetched, so it reports no check at all until a caller activates it. Reads only.
    pub fn best_practice_checks(
        &self,
        workload_id: &str,
        lens: &str,
        pillar_id: &str,
        question_id: &str,
        choice_id: &str,
    ) -> Result<BestPracticeChecks<'_>, BestPracticeRefusal> {
        let workload = self
            .workloads
            .get(workload_id)
            .ok_or_else(|| BestPracticeRefusal::NotFound(workload_not_found(workload_id)))?;
        if !workload.reviews_lens(lens) {
            return Err(BestPracticeRefusal::NotFound(lens_review_not_found(
                workload_id,
                lens,
            )));
        }

        let pinned = workload.lens_review_version(lens);
        let content = self.lens_content_at(lens, pinned);
        if let Some(exception) = undefined_pillar_refusal(
            PILLAR_ID_MEMBER,
            lens,
            &content,
            std::iter::once(pillar_id),
        ) {
            return Err(BestPracticeRefusal::Undefined(exception));
        }
        if let Some(exception) = undefined_pillar_question_refusal(
            QUESTION_ID_MEMBER,
            lens,
            pillar_id,
            &content,
            std::iter::once(question_id),
        ) {
            return Err(BestPracticeRefusal::Undefined(exception));
        }

        // The question as the LENS asks it, which is where its choice catalog — the only
        // place a client discovers the `ChoiceId`s such a request names — comes from.
        let question = self.lens_review_question(lens, pinned, question_id);
        if let Some(question) = question.as_ref() {
            if let Some(exception) = undefined_choice_member_refusal(
                CHOICE_ID_MEMBER,
                question,
                std::iter::once(choice_id),
            ) {
                return Err(BestPracticeRefusal::Undefined(exception));
            }
        }

        let checks = question
            .as_ref()
            .filter(|_| workload.trusted_advisor_integration_enabled())
            .map(|question| checks_for_best_practice(lens, question, choice_id))
            .unwrap_or_default();
        Ok(BestPracticeChecks {
            workload,
            lens_arn: self.lens_arn_of(lens),
            checks,
        })
    }

    /// Sync one workload's lens reviews with the integrated service, recording `at`
    /// against every question the integration covers and answering how many questions
    /// that was.
    ///
    /// The sync is of the WORKLOAD, so it covers each lens associated with it, and the
    /// questions of one review are the ones that lens's own content defines
    /// ([`Self::lens_review_questions`]) — narrowed, for a workload whose issue management
    /// is manual, to the questions that review's selection names
    /// ([`WorkloadRecord::jira_issue_management_is_manual`],
    /// [`WorkloadRecord::jira_selected_question_ids`]). What it records is what every read
    /// of an answer reports as the instant its integration configuration was last synced
    /// ([`WorkloadRecord::question_jira_configuration`]), so nothing derives a sync from a
    /// read.
    ///
    /// Answers 0 for a workload this store does not hold, one associated with no lens, one
    /// whose lenses this emulator holds no question content for, and one whose manual
    /// selection names no question — in each case there is no question to carry an issue,
    /// so nothing is written and the workload is left as it stood.
    ///
    /// The issue management STATUS is deliberately not consulted: `IssueManagementStatus`
    /// (`ENABLED` | `DISABLED` | `INHERIT`, inheriting the account's
    /// `AccountJiraIssueManagementStatus`) governs whether the service manages a question's
    /// issue on its own, where a call that asks for a sync is the caller asking for one
    /// outright — and the account-level connection it would inherit from can never report
    /// `CONFIGURED` through this API ([`JIRA_INTEGRATION_NOT_CONFIGURED`]), so gating the
    /// sync on either status would leave this operation with nothing it could ever sync.
    pub fn sync_workload_integration(&mut self, workload_id: &str, at: SystemTime) -> usize {
        // Which questions the sync covers is read off the stored workload and the lens
        // content BEFORE anything is written, because resolving a lens's question set
        // borrows this store while the write borrows it mutably.
        let covered: Vec<(String, Vec<String>)> = match self.workloads.get(workload_id) {
            Some(workload) => {
                let manual = workload.jira_issue_management_is_manual();
                workload
                    .lenses
                    .iter()
                    .map(|lens| {
                        let selected = manual.then(|| workload.jira_selected_question_ids(lens));
                        let questions = self
                            .lens_review_questions(lens, workload.lens_review_version(lens))
                            .into_iter()
                            .map(|question| question.question_id)
                            .filter(|question_id| {
                                selected
                                    .as_ref()
                                    .is_none_or(|selected| selected.contains(question_id))
                            })
                            .collect();
                        (lens.clone(), questions)
                    })
                    .collect()
            }
            None => return 0,
        };

        let Some(workload) = self.workloads.get_mut(workload_id) else {
            return 0;
        };
        let mut synced = 0usize;
        for (lens, questions) in covered {
            if questions.is_empty() {
                continue;
            }
            let review = workload.lens_review_record_mut(&lens);
            for question_id in questions {
                review.jira_question_syncs.insert(question_id, at);
                synced = synced.saturating_add(1);
            }
        }
        if synced > 0 {
            // The sync changes this workload's lens reviews, which is what a read of the
            // workload and of each of its reviews reports as the instant they were last
            // updated (`Workload$UpdatedAt`, `LensReview$UpdatedAt`).
            workload.updated_at = at;
        }
        synced
    }

    /// The `LensArn` the lens a request named is reported by.
    ///
    /// An AWS-official lens's ARN carries no Region and no owner account and is built from
    /// the alias ([`official_lens_arn`]), while a custom lens's is the one its import
    /// minted — Region- and account-scoped — and this store holds. A custom lens this
    /// store does not hold is reported by the ARN the caller named it with: a custom lens
    /// cannot be reviewed until `CreateLensVersion` publishes it — its own stub — so this
    /// emulator has no other ARN for it.
    pub fn lens_arn_of(&self, lens: &str) -> String {
        if is_aws_official_lens(lens) {
            return official_lens_arn(lens);
        }
        self.lens_by_alias(lens)
            .map_or_else(|| lens.to_string(), |record| record.lens_arn.clone())
    }

    /// The `LensVersion` of the LATEST version of the lens a `LensAlias` names — the
    /// version a review of that lens is CURRENT against, and the one an upgrade of such a
    /// review moves it to.
    ///
    /// An AWS-official lens is published by AWS, so the version of it this emulator ships
    /// content for is its latest ([`lens_metadata`]); a custom lens's latest version is
    /// the one `CreateLensVersion` published most recently
    /// ([`LensRecord::latest_version`]), whatever its draft now holds. `None` for a lens
    /// this emulator holds no content for and for a custom lens no version has been
    /// published from.
    pub fn latest_lens_version(&self, lens: &str) -> Option<&str> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.version);
        }
        self.lens_by_alias(lens)?
            .latest_version()
            .map(|(version, _)| version)
    }

    /// The `LensName` the lens a `LensAlias` names carries as of `version`: the name the
    /// content of that version gives the lens.
    ///
    /// A custom lens is named BY ITS CONTENT ("name" of the lens document), so a version
    /// published from other content can name it differently and the name is read off the
    /// version a review is against; a version this store does not hold falls back to the
    /// lens's latest, which is also what a review pinning no version is against. An
    /// AWS-official lens's name is the catalog's, which its one shipped version is the
    /// content of.
    pub fn lens_name_at(&self, lens: &str, version: Option<&str>) -> Option<&str> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.name);
        }
        let record = self.lens_by_alias(lens)?;
        let published = version
            .and_then(|version| record.versions.get(version))
            .or_else(|| record.latest_version().map(|(_, published)| published))?;
        Some(published.content.definition.name.as_str())
    }

    /// The `LensVersion` a review of `lens` is against: the version the lens's content is
    /// published AS.
    ///
    /// A lens review is a review against ONE version of the lens, so applying a lens pins
    /// the version its content is published as right then
    /// ([`LensRecord::published_version`]) — the same way associating a profile with a
    /// workload pins the profile version it is reviewed against
    /// ([`WorkloadProfileRecord::profile_version`]). `None` for a lens this emulator holds
    /// no content for, and for a custom lens whose content is a DRAFT — which no operation
    /// applies, since a custom lens "cannot be applied to workloads … until it's published
    /// with CreateLensVersion" (`ImportLens` doc) is what
    /// [`Self::unapplicable_lenses_refusal`] enforces.
    pub fn applied_lens_version(&self, lens: &str) -> Option<String> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.version.to_string());
        }
        self.lens_by_alias(lens)?
            .published_version()
            .map(|(version, _)| version.to_string())
    }

    /// The lens reviews a review template applying `lenses` carries the moment it is
    /// created: one per applied lens, keyed by lens ALIAS, each against the version that
    /// lens's content is published as ([`Self::applied_lens_version`]) and with nothing
    /// written to it yet.
    ///
    /// A lens review exists as soon as the lens is applied
    /// ([`ReviewTemplateRecord::applies_lens`]), which is why the records are created with
    /// the template rather than on the first write to one.
    pub fn applied_lens_reviews(
        &self,
        lenses: &[String],
        now: SystemTime,
    ) -> BTreeMap<String, ReviewTemplateLensReviewRecord> {
        lenses
            .iter()
            .map(|lens| {
                (
                    lens_alias(lens).to_string(),
                    ReviewTemplateLensReviewRecord {
                        notes: None,
                        pillar_notes: BTreeMap::new(),
                        updated_at: now,
                        lens_version: self.applied_lens_version(lens),
                    },
                )
            })
            .collect()
    }

    /// The lens reviews a workload associating `lenses` carries the moment it is created:
    /// one per associated lens, keyed by lens ALIAS, each against the version that lens's
    /// content is published as ([`Self::applied_lens_version`]) and with nothing
    /// written to it yet.
    ///
    /// The mirror of [`Self::applied_lens_reviews`], which does the same for a review
    /// TEMPLATE's lens reviews: a lens review is a review against ONE version of the
    /// lens, so associating a lens pins the version its content is published as right
    /// then, whichever resource the review belongs to.
    pub fn applied_workload_lens_reviews(
        &self,
        lenses: &[String],
    ) -> BTreeMap<String, LensReviewRecord> {
        lenses
            .iter()
            .map(|lens| {
                (
                    lens_alias(lens).to_string(),
                    LensReviewRecord {
                        notes: None,
                        pillar_notes: BTreeMap::new(),
                        jira_configuration: None,
                        // A question of this review carries no issue in the integrated
                        // service until a sync records one
                        // ([`Self::sync_workload_integration`]).
                        jira_question_syncs: BTreeMap::new(),
                        lens_version: self.applied_lens_version(lens),
                    },
                )
            })
            .collect()
    }

    /// What the lens catalog says about the lens ONE lens review of the stored review
    /// template `template_arn` is a review of, for the version that review is against
    /// ([`ReviewTemplateRecord::lens_review_version`]).
    ///
    /// `lens` is the lens as the request named it, which is the spelling its `LensArn` is
    /// resolved from. A `template_arn` this store holds no template under has no review to
    /// report — the caller answers that lookup miss itself — and what comes back then
    /// describes the lens as a review pinning no version would be against, its latest.
    pub fn reviewed_lens(&self, template_arn: &str, lens: &str) -> ReviewedLens {
        let pinned = self
            .review_templates
            .get(template_arn)
            .and_then(|template| template.lens_review_version(lens));
        self.reviewed_lens_at(lens, pinned)
    }

    /// What the lens catalog says about the lens ONE lens review is a review of, for the
    /// version that review PINS (`pinned`) — the one place that resolution lives, so a
    /// workload's lens review and a review template's cannot disagree about which version
    /// a review is against or whether that version is current.
    ///
    /// `lens` is the lens as the request named it, which is the spelling its `LensArn` is
    /// resolved from. A review that pins no version is against the lens's LATEST version,
    /// and a lens this emulator holds no content for has no version to be at.
    pub fn reviewed_lens_at(&self, lens: &str, pinned: Option<&str>) -> ReviewedLens {
        let latest = self.latest_lens_version(lens);
        // The version the review is AGAINST: the one it pins, or the lens's latest for a
        // review that pins none.
        let version = pinned.or(latest);
        ReviewedLens {
            lens_arn: self.lens_arn_of(lens),
            lens_name: self.lens_name_at(lens, version).map(str::to_string),
            lens_status: version.zip(latest).map(|(version, latest)| {
                if version == latest {
                    LENS_STATUS_CURRENT.to_string()
                } else {
                    LENS_STATUS_NOT_CURRENT.to_string()
                }
            }),
            lens_version: version.map(str::to_string),
        }
    }

    /// The two versions ONE lens review is BEHIND by — `(the version the review is pinned
    /// at, the lens's latest version)` — or `None` when it is not behind at all.
    ///
    /// A lens review is a review against ONE version of the lens, and a lens's versions only
    /// ever accumulate, so a pinned version that is not the lens's latest is one the lens has
    /// moved PAST: that is what makes the review `NOT_CURRENT` ([`Self::reviewed_lens_at`])
    /// and what an upgrade of the review clears. A review pinning no version is against the
    /// lens's latest by definition, and a lens this emulator holds no content for has no
    /// latest version to be behind, so neither is behind anything.
    ///
    /// The one place that comparison lives, so a review template's update status, the
    /// notifications a stale review raises and an upgrade cannot disagree about which reviews
    /// are behind.
    pub fn superseded_lens_review(
        &self,
        lens: &str,
        pinned: Option<&str>,
    ) -> Option<(String, String)> {
        let latest = self.latest_lens_version(lens)?;
        let pinned = pinned.filter(|pinned| *pinned != latest)?;
        Some((pinned.to_string(), latest.to_string()))
    }

    /// `ReviewTemplate$UpdateStatus` / `ReviewTemplateSummary$UpdateStatus` of one stored
    /// template: [`REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT`] while ANY lens review
    /// of it is against a version its lens has superseded, else
    /// [`REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT`].
    ///
    /// Derived on every read from the template's pinned versions and the lens catalog, so
    /// it moves when either does — the template's own view of the `LensStatus` each of its
    /// lens reviews reports.
    pub fn review_template_update_status(&self, template: &ReviewTemplateRecord) -> &'static str {
        let superseded = template.lenses.iter().any(|lens| {
            self.superseded_lens_review(lens, template.lens_review_version(lens))
                .is_some()
        });
        if superseded {
            REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT
        } else {
            REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT
        }
    }

    /// [`Self::lens_by_alias`] for a caller that is about to write to the lens.
    pub fn lens_by_alias_mut(&mut self, alias: &str) -> Option<&mut LensRecord> {
        let id = lens_alias(alias).to_string();
        let record = self.lenses.get_mut(&id)?;
        alias_names_lens(alias, record).then_some(record)
    }

    /// Drop an agent profile and everything keyed to it: every create or update token
    /// that resolved to it, the goals and contexts recorded against it with the create
    /// tokens that resolved to those, and the recommendation generations started against
    /// it, so nothing outlives the profile it belongs to and the next request carrying
    /// one of those tokens acts afresh.
    ///
    /// `DeleteAgentProfile` "deletes an optimization profile and its associated
    /// configuration": the profile's own configuration — its pillars, aggregation
    /// configuration and tags — lives on the record, and a goal, a context or a
    /// generation is reached only through the profile it belongs to, so none of them can
    /// be left behind. Dropping the generations is also what keeps
    /// [`MAX_GENERATIONS_PER_AGENT_PROFILE`] a real bound on
    /// [`Self::agent_generations`]: that ceiling counts the generations of a LIVE
    /// profile, and an ARN is never minted twice, so a generation left under a deleted
    /// profile's ARN could never be reclaimed.
    pub fn remove_agent_profile(&mut self, profile_arn: &str) -> Option<AgentProfileRecord> {
        let record = self.agent_profiles.remove(profile_arn)?;
        self.create_agent_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.update_agent_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_goals.remove(profile_arn);
        self.create_agent_goal_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.update_agent_goal_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_contexts.remove(profile_arn);
        self.create_agent_context_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.update_agent_context_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_generations.remove(profile_arn);
        // The recommendations those generations produced go with them: a recommendation is
        // a result OF a generation of this profile, so none can outlive the profile, and
        // dropping them is what keeps the bound on
        // [`Self::agent_recommendations`] real.
        self.agent_recommendations.remove(profile_arn);
        Some(record)
    }

    /// The lens a `LensAlias` names: the whole catalog a lens name resolves
    /// against — the lenses Amazon Web Services publishes
    /// ([`AWS_OFFICIAL_LENS_ALIASES`]) and the custom lenses this account holds
    /// ([`Self::lens_by_alias`]), the latter applicable only once a version has been
    /// published from their content.
    pub fn resolve_lens(&self, lens: &str) -> LensResolution {
        // "For custom lenses, this is the lens ARN" (`LensAlias` doc), so a name that
        // carries no owner account can only be an official lens, and one that carries one
        // can only be a custom lens: the two halves of the catalog never overlap.
        if is_aws_official_lens(lens) {
            return if is_published_official_lens(lens) {
                LensResolution::Official
            } else {
                LensResolution::Unknown
            };
        }
        match self.lens_by_alias(lens) {
            None => LensResolution::Unknown,
            Some(record) if record.published_version().is_some() => {
                LensResolution::PublishedCustom
            }
            Some(_) => LensResolution::DraftCustom,
        }
    }

    /// How a list of lenses a request asks to APPLY is refused, or `None` when every entry
    /// names a lens that can be applied.
    ///
    /// A lens is applied by name, and the name has to resolve to a lens the caller can
    /// reach: "Each lens is identified by its `LensSummary$LensAlias`" (`WorkloadLenses`
    /// doc), and a custom lens "cannot be applied to workloads … until it's published with
    /// CreateLensVersion" (`ImportLens` doc). Every operation that attaches lenses to a
    /// resource resolves them through here, so none of them can record a lens another
    /// operation would then report a review of and find nothing for.
    ///
    /// An entry naming NO lens and one naming a lens no version has been published from
    /// are refused IDENTICALLY ([`LensesRefusal`]): both name something the caller cannot
    /// get a lens through, and the service does not disclose which of the two it was — the
    /// same refusal a caller gets for a lens another account holds and has not shared.
    ///
    /// The walk stops at the FIRST entry it refuses and allocates nothing per entry it
    /// accepts, so the work this costs is one map lookup and one scan of the fixed
    /// [`AWS_OFFICIAL_LENS_ALIASES`] catalog per entry and NOTHING that grows with the
    /// caller's list beyond that — the list length itself is bounded by the calling
    /// operation's own lens quota before it gets here.
    pub fn unapplicable_lenses_refusal(&self, lenses: &[String]) -> Option<LensesRefusal> {
        for lens in lenses {
            match self.resolve_lens(lens) {
                LensResolution::Unknown | LensResolution::DraftCustom => {
                    return Some(LensesRefusal::NotFound(unknown_lens_not_found(lens)));
                }
                LensResolution::Official | LensResolution::PublishedCustom => {}
            }
        }
        None
    }

    /// The stable list order `ListReviewTemplates` pages over: `TemplateArn`, which is
    /// also what [`Self::review_templates`] is keyed by.
    ///
    /// The model publishes no order for `ReviewTemplates`, so this is the emulator's own.
    /// What it has to be is TOTAL and stable — the `NextToken` carries the ARN of the
    /// last template a page served, so a key that moved or repeated would serve a
    /// template twice or skip it. The ARN is the only member of a template that can do
    /// neither: it is unique and it is fixed for the template's whole life, where
    /// `TemplateName` is a member `UpdateReviewTemplate` can move.
    pub fn review_template_order(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.review_templates.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// The stable list order `ListProfiles` pages over: `ProfileArn`, which is also what
    /// [`Self::profiles`] is keyed by.
    ///
    /// The model publishes no order for `ProfileSummaries`, so this is the emulator's own,
    /// chosen for the reason [`Self::review_template_order`] gives: the `NextToken` carries
    /// the key of the last profile a page served, so that key has to be unique and fixed
    /// for the profile's whole life. The ARN is both, where `ProfileName` is a member
    /// `UpdateProfile` can move.
    pub fn profile_order(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.profiles.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// The stable list order `ListWorkloads` pages over: `WorkloadId`, which is also what
    /// [`Self::workloads`] is keyed by.
    ///
    /// The model publishes no order for `WorkloadSummaries`, so this is the emulator's own,
    /// chosen for the reason [`Self::review_template_order`] gives: the `NextToken` carries
    /// the key of the last workload a page served, so both the order and that key have to
    /// be unique and fixed for the workload's whole life, or a value moving between two
    /// pages would serve a workload twice or skip it. The id is both, where `WorkloadName`
    /// is a member `UpdateWorkload` can move.
    pub fn workload_order(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.workloads.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Drop a workload and everything keyed to it: the name that reserved uniqueness
    /// and any create token that resolved to it.
    ///
    /// [`Self::delete_workload_tokens`] is NOT among them: a token that names the delete
    /// itself has to survive the resource the delete removed, or the retry it was issued
    /// for could not be replayed.
    pub fn remove_workload(&mut self, workload_id: &str) -> Option<WorkloadRecord> {
        let record = self.workloads.remove(workload_id)?;
        self.workload_name_index
            .remove(&normalize_unique_name(&record.workload_name));
        self.create_workload_tokens
            .retain(|token| token.workload_id != workload_id);
        // A milestone belongs to its workload, so the whole set goes with it — and with
        // it every create token that resolved to one of them.
        self.milestones.remove(workload_id);
        self.create_milestone_tokens
            .retain(|token| token.workload_id != workload_id);
        // A share shares THIS workload, so it cannot outlive it either.
        self.workload_shares.remove(workload_id);
        self.create_workload_share_tokens
            .retain(|token| token.workload_id != workload_id);
        // An upgrade token resolves to the upgrade of an association OF this workload, so
        // it goes with the associations the record above carried.
        self.upgrade_profile_version_tokens
            .retain(|token| token.workload_id != workload_id);
        // A sync token resolves to a sync OF this workload's lens reviews, so it goes with
        // the reviews the record above carried.
        self.update_integration_tokens
            .retain(|token| token.workload_id != workload_id);
        // A lens-review upgrade token resolves to a lens review OF this workload, so it
        // cannot outlive it either: the next request carrying one acts afresh.
        self.upgrade_lens_review_tokens
            .retain(|token| token.resource_id != workload_id);
        Some(record)
    }

    /// Drop a review template and everything keyed to it: any create token that resolved
    /// to it, so a retry of the create that minted it is no longer a replay of a template
    /// that is gone.
    ///
    /// Everything else a template owns — its lens reviews' notes and the answers recorded
    /// against them — lives INSIDE [`ReviewTemplateRecord`], so it goes with the record
    /// itself. A workload created from a template copied that template's lenses and notes
    /// at create time (`CreateWorkload`), so it keeps them: the workload is not a child of
    /// the template.
    pub fn remove_review_template(&mut self, template_arn: &str) -> Option<ReviewTemplateRecord> {
        let record = self.review_templates.remove(template_arn)?;
        self.create_review_template_tokens
            .retain(|token| token.template_arn != template_arn);
        // An upgrade token resolves to a lens review OF this template, so it cannot
        // outlive it either: the next request carrying one acts afresh.
        self.upgrade_review_template_lens_review_tokens
            .retain(|token| token.resource_id != template_arn);
        // A share shares THIS template, so it cannot outlive it either — the same rule
        // [`Self::remove_workload`] applies to a workload's shares.
        self.template_shares.remove(template_arn);
        self.create_template_share_tokens
            .retain(|token| token.template_arn != template_arn);
        Some(record)
    }

    /// Drop a profile and everything keyed to it: any create token that resolved to it,
    /// so a retry of the create that minted it is no longer a replay of a profile that is
    /// gone, and the association every workload held it by, since an association reports
    /// the ARN and the version of a profile that has to resolve for the association to
    /// mean anything ([`WorkloadProfileRecord`]) — the same reason a share does not
    /// outlive the resource it shares in [`Self::remove_workload`]. The questions a
    /// profile answers and the tags it carries live INSIDE [`ProfileRecord`], so they go
    /// with the record itself.
    ///
    /// [`Self::delete_profile_tokens`] is NOT among them, exactly as in
    /// [`Self::remove_workload`]: a token that names the delete itself has to survive the
    /// resource the delete removed, or the retry it was issued for could not be replayed.
    ///
    /// `at` is the instant the removal is served at, and every workload that LOSES an
    /// association here records it as its `UpdatedAt`: the profiles are a member of the
    /// workload (`Workload$Profiles`), so dropping one changes the workload, and
    /// `Workload$UpdatedAt` is what every read of the workload reports that change by —
    /// the same rule the operation that drops an association directly writes it under.
    /// A workload that held no association with this profile is not changed, so its own
    /// instant stands where it stood.
    pub fn remove_profile(&mut self, profile_arn: &str, at: SystemTime) -> Option<ProfileRecord> {
        let record = self.profiles.remove(profile_arn)?;
        self.create_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        for workload in self.workloads.values_mut() {
            if !workload.associates_profile(profile_arn) {
                continue;
            }
            workload.disassociate_profile(profile_arn);
            workload.updated_at = at;
        }
        // An upgrade token resolves to the upgrade of an association OF this profile, so
        // it goes with the associations dropped just above.
        self.upgrade_profile_version_tokens
            .retain(|token| token.profile_arn != profile_arn);
        // A share shares THIS profile, so it cannot outlive it either — the same rule
        // [`Self::remove_workload`] applies to a workload's shares.
        self.profile_shares.remove(profile_arn);
        self.create_profile_share_tokens
            .retain(|token| token.profile_arn != profile_arn);
        Some(record)
    }

    /// Drop a custom lens and everything keyed to it: the import token that resolved to
    /// the lens and every publish token that resolved to a version of it, so a retry
    /// carrying one acts afresh rather than replaying a lens that is gone, and the shares
    /// recorded against it with the create tokens that resolved into them — the same rule
    /// [`Self::remove_workload`] applies to a workload's shares, which also takes the
    /// invitations those shares raised, since an invitation is reached only through the
    /// share that offers it. The lens's draft content, its published versions and its tags
    /// live INSIDE [`LensRecord`], so they go with the record itself.
    ///
    /// [`Self::delete_lens_tokens`] is NOT among them, exactly as in
    /// [`Self::remove_workload`]: a token that names the delete itself has to survive the
    /// resource the delete removed, or the retry it was issued for could not be replayed.
    pub fn remove_lens(&mut self, lens_id: &str) -> Option<LensRecord> {
        let record = self.lenses.remove(lens_id)?;
        self.import_lens_tokens
            .retain(|token| token.lens_id != lens_id);
        self.create_lens_version_tokens
            .retain(|token| token.lens_id != lens_id);
        self.lens_shares.remove(lens_id);
        self.create_lens_share_tokens
            .retain(|token| token.lens_id != lens_id);
        Some(record)
    }

    /// Drop every version published from a custom lens, leaving the lens itself and the
    /// content it carries — which is then content no version has been published from, so
    /// the lens reports [`LENS_STATUS_DRAFT`] again.
    ///
    /// The publish tokens that resolved to those versions go with them, so a retry
    /// carrying one publishes afresh rather than replaying a version that is gone — the
    /// rule [`Self::remove_lens`] applies to every token of a lens it removes.
    ///
    /// `false` when no lens is stored under `lens_id`, so nothing was dropped.
    pub fn remove_lens_published_versions(&mut self, lens_id: &str) -> bool {
        let Some(record) = self.lenses.get_mut(lens_id) else {
            return false;
        };
        record.versions.clear();
        self.create_lens_version_tokens
            .retain(|token| token.lens_id != lens_id);
        true
    }

    /// The workload already holding `name` under the service's own loose comparison
    /// ("spaces and capitalization are ignored"): its name AS STORED and its id. The id
    /// is what a collision points the caller at ([`workload_name_conflict`], whose
    /// sentence quotes the name the REQUEST submitted); the stored name is what a caller
    /// comparing the two spellings reads.
    pub fn workload_holding_name(&self, name: &str) -> Option<(&str, &str)> {
        let workload_id = self.workload_name_index.get(&normalize_unique_name(name))?;
        let record = self.workloads.get(workload_id)?;
        Some((
            record.workload_name.as_str(),
            record.workload_id.as_str(),
        ))
    }

    /// The review template already holding `name` under the service's own loose
    /// comparison ([`normalize_unique_name`]), if one holds it: a review template's name
    /// identifies it within the account, so the scope of the lookup is the whole store
    /// and a spelling differing only in capitalization or whitespace finds the same
    /// template.
    ///
    /// The record it answers with is the template AS STORED, whose ARN is what
    /// [`review_template_name_conflict`] points the caller at; the name that refusal
    /// quotes is the one the request submitted. The scan is over a store bounded by
    /// [`MAX_REVIEW_TEMPLATES`], exactly as
    /// [`Self::milestone_holding_name`] scans the milestones of one workload.
    pub fn review_template_holding_name(&self, name: &str) -> Option<&ReviewTemplateRecord> {
        let normalized = normalize_unique_name(name);
        self.review_templates
            .values()
            .find(|record| normalize_unique_name(&record.template_name) == normalized)
    }

    /// The milestone `number` recorded for `workload_id`, if one is.
    pub fn milestone(&self, workload_id: &str, number: i32) -> Option<&MilestoneRecord> {
        self.milestones.get(workload_id)?.get(&number)
    }

    /// The milestone of `workload_id` already holding `name` under the service's own
    /// loose comparison ([`normalize_unique_name`]), if one holds it: "Milestone names
    /// must be unique within a workload" (`smithy_query(query="shape",
    /// shape="MilestoneName")`), so the scope of the lookup is ONE workload and a
    /// spelling differing only in capitalization or whitespace finds the same milestone.
    ///
    /// The record it answers with is the milestone AS STORED, whose NUMBER is what
    /// [`milestone_name_conflict`] points the caller at; the name that refusal quotes is
    /// the one the request submitted.
    pub fn milestone_holding_name(
        &self,
        workload_id: &str,
        name: &str,
    ) -> Option<&MilestoneRecord> {
        let normalized = normalize_unique_name(name);
        self.milestones.get(workload_id)?.values().find(|record| {
            normalize_unique_name(&record.milestone_name) == normalized
        })
    }

    /// The number the next milestone of `workload_id` takes: one past the highest ever
    /// handed out, starting at 1. Can exceed [`MAX_MILESTONES_PER_WORKLOAD`], which is
    /// how a caller meets the quota.
    pub fn next_milestone_number(&self, workload_id: &str) -> i32 {
        self.milestones
            .get(workload_id)
            .and_then(|recorded| recorded.keys().next_back().copied())
            .unwrap_or(0)
            .saturating_add(1)
    }

    /// Record a milestone of the workload `workload_id` as it stands at `now`, under
    /// `milestone_name`, and answer the `MilestoneNumber` it took.
    ///
    /// The ONE place a milestone comes into existence, so every operation that records one
    /// — `CreateMilestone` and the upgrades that keep the review as it stood before they
    /// moved it — records it under the same rules and refuses it with the same wording
    /// ([`MilestoneRefusal`]): "A milestone records the state of a workload at a
    /// particular point in time" (`CreateMilestone` doc), so the workload is copied BY
    /// VALUE and a later update cannot change what this milestone reports; "milestone
    /// names must be unique within a workload" (`smithy_query(query="shape",
    /// shape="MilestoneName")`), compared the way this service compares a name it meters
    /// for uniqueness ([`normalize_unique_name`]); and a workload holds at most
    /// [`MAX_MILESTONES_PER_WORKLOAD`] of them.
    ///
    /// Every refusal is decided BEFORE anything is written, and a refused request consumes
    /// no milestone number, so a caller that is refused leaves the workload exactly as it
    /// stood.
    pub fn record_milestone(
        &mut self,
        workload_id: &str,
        milestone_name: String,
        now: SystemTime,
    ) -> Result<i32, MilestoneRefusal> {
        if !self.workloads.contains_key(workload_id) {
            return Err(MilestoneRefusal::NotFound(workload_not_found(workload_id)));
        }
        if let Some(existing) = self.milestone_holding_name(workload_id, &milestone_name) {
            let existing_number = existing.milestone_number;
            return Err(MilestoneRefusal::NameTaken(milestone_name_conflict(
                &milestone_name,
                workload_id,
                existing_number,
            )));
        }
        let milestone_number = self.next_milestone_number(workload_id);
        if milestone_number > MAX_MILESTONES_PER_WORKLOAD {
            return Err(MilestoneRefusal::QuotaExceeded(
                crate::ops::types::ServiceQuotaExceededException {
                    message: format!(
                        "The workload with ID {workload_id} already holds the maximum of \
                         {MAX_MILESTONES_PER_WORKLOAD} milestones"
                    ),
                    resource_id: Some(workload_id.to_string()),
                    resource_type: Some(WORKLOAD_RESOURCE_TYPE.to_string()),
                    // The model publishes no numeric Service Quotas code for this quota,
                    // so it is named by what it meters; the service code is this
                    // service's own signing name.
                    quota_code: "MilestonesPerWorkload".to_string(),
                    service_code: ARN_NAMESPACE.to_string(),
                },
            ));
        }
        let Some(workload) = self.workloads.get(workload_id).cloned() else {
            // Defensive, and worded as the first lookup above is: that lookup already
            // resolved the workload under this same borrow, so nothing can have removed
            // it in between, and a milestone is recorded only of a workload that is
            // there.
            return Err(MilestoneRefusal::NotFound(workload_not_found(workload_id)));
        };
        self.milestones
            .entry(workload_id.to_string())
            .or_default()
            .insert(
                milestone_number,
                MilestoneRecord {
                    milestone_number,
                    milestone_name,
                    recorded_at: now,
                    workload,
                },
            );
        Ok(milestone_number)
    }

    /// Every share recorded against `workload_id`, in `ShareId` order.
    pub fn workload_shares_of(&self, workload_id: &str) -> Vec<&WorkloadShareRecord> {
        self.workload_shares
            .get(workload_id)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of `workload_id`, if the workload holds it.
    pub fn workload_share(
        &self,
        workload_id: &str,
        share_id: &str,
    ) -> Option<&WorkloadShareRecord> {
        self.workload_shares.get(workload_id)?.get(share_id)
    }

    /// The share `share_id` of `workload_id` for modification, if the workload holds it.
    /// Resolved through the same two-level key as [`Self::workload_share`], so a share id
    /// is only ever changed within the workload it belongs to.
    pub fn workload_share_mut(
        &mut self,
        workload_id: &str,
        share_id: &str,
    ) -> Option<&mut WorkloadShareRecord> {
        self.workload_shares.get_mut(workload_id)?.get_mut(share_id)
    }

    /// Every share invitation addressed to `principal` that is still waiting on an
    /// answer, in `ShareInvitationId` order — what `ListShareInvitations` reports.
    ///
    /// An invitation is offered to the principal a resource was shared WITH, so a share
    /// is reported to that principal and not to the one that created it; it is
    /// outstanding while its share is still in the status a new share carries
    /// ([`DEFAULT_SHARE_STATUS`], `PENDING`), and is gone once the recipient has
    /// answered it. `ShareInvitationId` order is the stable order the pages are cut on,
    /// which is why the enumeration sorts here rather than at the caller.
    ///
    /// Workload, LENS, PROFILE and REVIEW-TEMPLATE invitations can exist in this
    /// emulator, and every kind is reported through this one enumeration.
    pub fn pending_share_invitations_for(&self, principal: &str) -> Vec<ShareInvitationSummary> {
        let mut invitations: Vec<ShareInvitationSummary> = self
            .workload_shares
            .values()
            .flat_map(|shares| shares.values())
            .filter(|share| {
                share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
            })
            .map(|share| {
                let workload_name = self
                    .workloads
                    .get(&share.workload_id)
                    .map(|workload| workload.workload_name.as_str())
                    .unwrap_or_default();
                share.to_invitation_summary(workload_name)
            })
            .collect();
        // A lens share raises an invitation of its own kind, reported through the same
        // enumeration so one order and one `PENDING` rule cover every shared resource.
        invitations.extend(
            self.lens_shares
                .values()
                .flat_map(|shares| shares.values())
                .filter(|share| {
                    share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
                })
                .map(|share| {
                    // The lens's NAME is its content's, which a later import can change,
                    // so it is read here rather than copied onto the share.
                    let lens_name = self
                        .lenses
                        .get(&share.lens_id)
                        .and_then(|lens| lens.content())
                        .map(|content| content.definition.name.as_str());
                    share.to_invitation_summary(lens_name)
                }),
        );
        // A profile share raises an invitation of its own kind, reported through the same
        // enumeration for the same reason.
        invitations.extend(
            self.profile_shares
                .values()
                .flat_map(|shares| shares.values())
                .filter(|share| {
                    share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
                })
                .map(|share| {
                    // The profile's NAME is the profile's own, which `UpdateProfile` can
                    // change, so it is read here rather than copied onto the share.
                    let profile_name = self
                        .profiles
                        .get(&share.profile_arn)
                        .map(|profile| profile.profile_name.as_str());
                    share.to_invitation_summary(profile_name)
                }),
        );
        // A review-template share raises an invitation of its own kind, reported through
        // the same enumeration for the same reason.
        invitations.extend(
            self.template_shares
                .values()
                .flat_map(|shares| shares.values())
                .filter(|share| {
                    share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
                })
                .map(|share| {
                    // The template's NAME is the template's own, which
                    // `UpdateReviewTemplate` can change, so it is read here rather than
                    // copied onto the share.
                    let template_name = self
                        .review_templates
                        .get(&share.template_arn)
                        .map(|template| template.template_name.as_str());
                    share.to_invitation_summary(template_name)
                }),
        );
        invitations.sort_by(|left, right| {
            left.share_invitation_id
                .cmp(&right.share_invitation_id)
        });
        invitations
    }

    /// The share of `workload_id` the principal `shared_with` already holds, if it holds
    /// one: a principal holds at most one share of one workload, so this is what a second
    /// share request for the same principal collides with.
    pub fn workload_share_with(
        &self,
        workload_id: &str,
        shared_with: &str,
    ) -> Option<&WorkloadShareRecord> {
        self.workload_shares
            .get(workload_id)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// Every share recorded against the lens `lens_id`, in `ShareId` order — the lens
    /// counterpart of [`Self::workload_shares_of`].
    pub fn lens_shares_of(&self, lens_id: &str) -> Vec<&LensShareRecord> {
        self.lens_shares
            .get(lens_id)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of the lens `lens_id`, if the lens holds it.
    pub fn lens_share(&self, lens_id: &str, share_id: &str) -> Option<&LensShareRecord> {
        self.lens_shares.get(lens_id)?.get(share_id)
    }

    /// The share of the lens `lens_id` the principal `shared_with` already holds, if it
    /// holds one: a principal holds at most one share of one lens, so this is what a
    /// second share request for the same principal collides with.
    pub fn lens_share_with(
        &self,
        lens_id: &str,
        shared_with: &str,
    ) -> Option<&LensShareRecord> {
        self.lens_shares
            .get(lens_id)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// Every share recorded against the profile `profile_arn`, in `ShareId` order — the
    /// profile counterpart of [`Self::workload_shares_of`].
    pub fn profile_shares_of(&self, profile_arn: &str) -> Vec<&ProfileShareRecord> {
        self.profile_shares
            .get(profile_arn)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of the profile `profile_arn`, if the profile holds it.
    pub fn profile_share(
        &self,
        profile_arn: &str,
        share_id: &str,
    ) -> Option<&ProfileShareRecord> {
        self.profile_shares.get(profile_arn)?.get(share_id)
    }

    /// The share of the profile `profile_arn` the principal `shared_with` already holds,
    /// if it holds one: a principal holds at most one share of one profile, so this is
    /// what a second share request for the same principal collides with.
    pub fn profile_share_with(
        &self,
        profile_arn: &str,
        shared_with: &str,
    ) -> Option<&ProfileShareRecord> {
        self.profile_shares
            .get(profile_arn)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// Every share recorded against the review template `template_arn`, in `ShareId`
    /// order — the review-template counterpart of [`Self::workload_shares_of`].
    pub fn template_shares_of(&self, template_arn: &str) -> Vec<&TemplateShareRecord> {
        self.template_shares
            .get(template_arn)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of the review template `template_arn`, if the template holds
    /// it.
    pub fn template_share(
        &self,
        template_arn: &str,
        share_id: &str,
    ) -> Option<&TemplateShareRecord> {
        self.template_shares.get(template_arn)?.get(share_id)
    }

    /// The share of the review template `template_arn` the principal `shared_with`
    /// already holds, if it holds one: a principal holds at most one share of one
    /// template, so this is what a second share request for it collides with.
    pub fn template_share_with(
        &self,
        template_arn: &str,
        shared_with: &str,
    ) -> Option<&TemplateShareRecord> {
        self.template_shares
            .get(template_arn)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// Drop one share of a workload and everything keyed to it — the workload counterpart
    /// of [`Self::remove_lens_share`], sharing its removal through [`take_share`]: the
    /// create token that resolved to the share goes with it, so the next request carrying
    /// that token shares afresh rather than replaying a share that is gone.
    ///
    /// `None` when the workload holds no such share, which is what tells a delete that the
    /// share it was asked to remove does not exist.
    pub fn remove_workload_share(
        &mut self,
        workload_id: &str,
        share_id: &str,
    ) -> Option<WorkloadShareRecord> {
        let record = take_share(&mut self.workload_shares, workload_id, share_id)?;
        self.create_workload_share_tokens
            .retain(|token| !(token.workload_id == workload_id && token.share_id == share_id));
        Some(record)
    }

    /// Drop one share of a lens and everything keyed to it: the create token that
    /// resolved to it — so the next request carrying that token shares afresh rather than
    /// replaying a share that is gone — and the lens's share map once its last share is
    /// gone, so a lens nobody shares leaves nothing behind.
    ///
    /// `None` when the lens holds no such share, which is what tells a delete that the
    /// share it was asked to remove does not exist.
    ///
    /// The record and the empty map are taken by [`take_share`], which every share kind
    /// in this store shares; only the create ledger that resolves to a share of THIS kind
    /// is this method's own.
    pub fn remove_lens_share(&mut self, lens_id: &str, share_id: &str) -> Option<LensShareRecord> {
        let record = take_share(&mut self.lens_shares, lens_id, share_id)?;
        self.create_lens_share_tokens
            .retain(|token| !(token.lens_id == lens_id && token.share_id == share_id));
        Some(record)
    }

    /// Drop one share of a profile and everything keyed to it — the profile counterpart
    /// of [`Self::remove_lens_share`], sharing its removal through [`take_share`]: the
    /// create token that resolved to the share goes with it, so the next request carrying
    /// that token shares afresh rather than replaying a share that is gone.
    ///
    /// `None` when the profile holds no such share, which is what tells a delete that the
    /// share it was asked to remove does not exist.
    pub fn remove_profile_share(
        &mut self,
        profile_arn: &str,
        share_id: &str,
    ) -> Option<ProfileShareRecord> {
        let record = take_share(&mut self.profile_shares, profile_arn, share_id)?;
        self.create_profile_share_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.share_id == share_id));
        Some(record)
    }

    /// Drop one share of a review template and everything keyed to it — the
    /// review-template counterpart of [`Self::remove_profile_share`], sharing its removal
    /// through [`take_share`]: the create token that resolved to the share goes with it, so
    /// the next request carrying that token shares afresh rather than replaying a share
    /// that is gone.
    ///
    /// `None` when the template holds no such share, which is what tells a delete that the
    /// share it was asked to remove does not exist.
    pub fn remove_template_share(
        &mut self,
        template_arn: &str,
        share_id: &str,
    ) -> Option<TemplateShareRecord> {
        let record = take_share(&mut self.template_shares, template_arn, share_id)?;
        self.create_template_share_tokens
            .retain(|token| !(token.template_arn == template_arn && token.share_id == share_id));
        Some(record)
    }

    /// The share the invitation `share_invitation_id` mediates, of WHICHEVER resource kind,
    /// if it was offered to `principal` — what `UpdateShareInvitation` resolves, for
    /// writing.
    ///
    /// "Update a workload or custom lens share invitation. This API operation can be
    /// called independently of any resource" (`UpdateShareInvitation` doc): an invitation
    /// names no resource and carries no resource kind, so it is resolved by its own id
    /// across every share this store holds — exactly the shares
    /// [`Self::pending_share_invitations_for`] reports invitations for, so the two sides
    /// of one invitation cannot disagree about whether it exists. Every share map this
    /// store holds is therefore looked in, under the same two conditions, and a kind whose
    /// shares it starts holding joins both sides at once: one more map to look in here and
    /// one more arm of [`ShareByInvitationMut`].
    ///
    /// An invitation addressed to any other principal resolves to nothing — the same
    /// `None` as an id no share carries, so neither discloses the other.
    pub fn share_by_invitation_mut(
        &mut self,
        share_invitation_id: &str,
        principal: &str,
    ) -> Option<ShareByInvitationMut<'_>> {
        // The share is LOCATED first, in one borrow-free pass per map
        // ([`share_key_by_invitation`] answers owned keys), so exactly ONE mutable borrow
        // is taken afterwards, out of the map that matched: a `&mut` taken while searching
        // would borrow the whole store for as long as the answer lives, and no later map
        // could be looked in. The two keys a pass answers were just read out of that same
        // map under the one lock this store is reached through, so the lookup that follows
        // them finds the share; `None` from it would mean the map changed in between, and
        // is answered as the invitation not being resolvable rather than assumed away.
        if let Some((workload_id, share_id)) = share_key_by_invitation(
            &self.workload_shares,
            share_invitation_id,
            principal,
            |share| (&share.share_invitation_id, &share.shared_with),
        ) {
            return self
                .workload_shares
                .get_mut(&workload_id)?
                .get_mut(&share_id)
                .map(ShareByInvitationMut::Workload);
        }
        if let Some((lens_id, share_id)) = share_key_by_invitation(
            &self.lens_shares,
            share_invitation_id,
            principal,
            |share| (&share.share_invitation_id, &share.shared_with),
        ) {
            return self
                .lens_shares
                .get_mut(&lens_id)?
                .get_mut(&share_id)
                .map(ShareByInvitationMut::Lens);
        }
        if let Some((profile_arn, share_id)) = share_key_by_invitation(
            &self.profile_shares,
            share_invitation_id,
            principal,
            |share| (&share.share_invitation_id, &share.shared_with),
        ) {
            return self
                .profile_shares
                .get_mut(&profile_arn)?
                .get_mut(&share_id)
                .map(ShareByInvitationMut::Profile);
        }
        let (template_arn, share_id) = share_key_by_invitation(
            &self.template_shares,
            share_invitation_id,
            principal,
            |share| (&share.share_invitation_id, &share.shared_with),
        )?;
        self.template_shares
            .get_mut(&template_arn)?
            .get_mut(&share_id)
            .map(ShareByInvitationMut::Template)
    }
}

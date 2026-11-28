// SPDX-License-Identifier: CC0-1.0

//! The FEED record and the outputs it holds: the single copy every feed handler reads
//! and writes.
//!
//! `CreateFeed` mints one, `GetFeed`/`ListFeeds` report it, `UpdateFeed` reconfigures it,
//! `AssociateFeed`/`DisassociateFeed` claim and release it and `DeleteFeed` retires it —
//! all on this record, so the shapes those handlers hand back are built from it. Its
//! resource-based policy and its schedule actions live on it rather than in stores of
//! their own, so neither can outlive the feed it belongs to or disagree with it.

use super::*;

/// One output of a feed: an Elemental Inference feature plus where its results go.
///
/// Two outputs are EQUAL when every member of the stored record matches, which is what
/// tells an output a request would really add from one the feed already holds exactly as
/// asked (`Feed::outputs_to_append`): the comparison is made on the stored form of both,
/// so a request that leaves an optional member to the service's default
/// ([`output_config_with_defaults`]) still matches the output that default produced. It
/// is the same comparison that tells a create this Region has already served from a new
/// one ([`ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub name: String,
    pub output_config: OutputConfig,
    pub status: String,
    pub description: Option<String>,
    /// True only for an output that entered the feed through `AssociateFeed`;
    /// `CreateFeed` and `UpdateFeed` outputs are false (`GetOutput$fromAssociation`).
    pub from_association: bool,
}

impl Output {
    /// The id of the dictionary this output references, if it references one — the
    /// stored output's view of [`dictionary_reference`], the one definition of which
    /// ids an output config names.
    pub fn referenced_dictionary(&self) -> Option<&str> {
        dictionary_reference(&self.output_config)
    }

    /// One output of a feed as it is STORED, from the output a create or update
    /// request carried.
    ///
    /// The config is stored with the service's own defaults already materialised
    /// ([`output_config_with_defaults`]), so every later read of the feed reports the
    /// configuration the service actually applied rather than only the members the
    /// request happened to carry.
    pub fn new(
        name: String,
        output_config: OutputConfig,
        status: String,
        description: Option<String>,
        from_association: bool,
    ) -> Self {
        Self {
            name,
            output_config: output_config_with_defaults(output_config),
            status,
            description,
            from_association,
        }
    }

    /// The `GetOutput` projection of this output, as every feed response reports it.
    pub fn to_get_output(&self) -> GetOutput {
        GetOutput {
            name: self.name.clone(),
            output_config: self.output_config.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            from_association: Some(self.from_association),
        }
    }
}

/// A feed: the target a calling application streams live media to.
#[derive(Debug, Clone)]
pub struct Feed {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region whose endpoint this feed was created through, and so the only one it
    /// exists for: the Region of the request that minted it ([`request_region`]), which
    /// is also the Region field of [`Self::arn`].
    pub region: String,
    /// The media inputs this feed exposes, by name: the calling application sends media
    /// to them, and the media data plane addresses one by its position in this set
    /// ([`Self::audio_input_at`]). Minted WITH the feed ([`minted_audio_inputs`]) — a
    /// feed always exposes at least one — and not a modelled member of any response:
    /// the model declares no inputs member on a feed shape.
    pub audio_inputs: Vec<String>,
    pub outputs: Vec<Output>,
    pub access_role_arn: Option<String>,
    pub status: FeedStatus,
    /// The resource associated with the feed; `None` until `AssociateFeed` runs.
    pub association: Option<FeedAssociation>,
    /// The name of the resource whose RELEASE this feed has already served, and `None`
    /// while no release of it has been served.
    ///
    /// `DisassociateFeed` is `@idempotent` and its `associatedResourceName` is the
    /// operation's `@idempotencyToken`, so a retry of the same release — what an SDK
    /// sends when it never saw the first response — has to answer the first call's
    /// success again instead of being judged against the state that first call left
    /// behind. This member is what tells a retry from a new request: it records the name
    /// the served release carried, so a request naming it is that same release once more
    /// and any other name is a request of its own.
    ///
    /// It lives on the feed record rather than in a store of its own, so it is bounded by
    /// the feeds that exist and leaves the store with the feed it belongs to — a released
    /// feed that is then deleted is simply not found, as any absent feed is.
    pub released_resource_name: Option<String>,
    /// The resource-based policy document attached to the feed, or `None` while the
    /// feed carries no policy.
    ///
    /// A feed holds AT MOST ONE policy: `PutFeedPolicy` "attaches or replaces" it, so
    /// a second put overwrites this member rather than adding beside it,
    /// `GetFeedPolicy` reports exactly the document the last put stored and
    /// `DeleteFeedPolicy` clears it. It lives on
    /// the feed record, so a policy is reachable only for as long as the feed it is
    /// attached to is, and the feed's removal takes its policy with it — there is no
    /// second map that could outlive the resource or disagree with it.
    pub policy: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The schedule actions on this feed: the configuration changes it applies to itself
    /// at the times a caller named ([`FeedScheduleAction`]).
    ///
    /// They live on the feed record because that is what they change: an action's
    /// operations are written into [`Self::outputs`], so the configuration a read of the
    /// feed reports is the configuration the actions that have run left behind, with no
    /// second copy to disagree with it. BOUNDED at [`MAX_SCHEDULE_ACTIONS_PER_FEED`]
    /// entries, which is also the most a listing can report, and a feed that leaves the
    /// store takes its actions with it ("when you delete or archive a feed, Elemental
    /// Inference deletes all the schedule actions on that feed").
    pub schedule_actions: Vec<FeedScheduleAction>,
    /// When the clock owes this feed's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
    /// The status this feed goes back into when the in-flight reconfiguration it is
    /// carrying falls due, and `None` while no reconfiguration is in flight.
    ///
    /// A reconfiguration does not CHANGE which lifecycle status a feed is in — it
    /// suspends it for the span of the work — so the status the feed was reconfigured
    /// from has to be remembered somewhere for the settle pass to put it back. It is
    /// kept on the feed record beside [`Self::transition_due_at`], so the instant the
    /// move falls due and the status it moves to are one fact about one feed and cannot
    /// drift apart.
    pub settles_into: Option<FeedStatus>,
}

impl Feed {
    /// Put this feed into a transient status the clock will move on by itself, and
    /// return the instant that move falls due. The caller hands that instant to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`], which is
    /// what lets the settle pass skip the store entirely while nothing is owed.
    #[must_use]
    pub fn enter_transient(&mut self, status: FeedStatus, now: Instant) -> Instant {
        let due = now + TRANSITION_SPAN;
        self.status = status;
        self.transition_due_at = Some(due);
        // Whatever this feed was going to settle back into, it is this transition's
        // outcome that decides where the feed goes now.
        self.settles_into = None;
        due
    }

    /// Put this feed into `UPDATING` for the span of a reconfiguration of its payload,
    /// remembering the status the clock settles it back into, and return the instant
    /// that move falls due (handed to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`] exactly as
    /// [`Self::enter_transient`]'s is).
    ///
    /// A reconfiguration that arrives while another is still in flight restarts the
    /// span and keeps the SETTLED status already remembered: the status to go back to is
    /// the one the feed held before any of the work started, never the transient one it
    /// is carrying now. A REPEAT of the reconfiguration already in flight is not such a
    /// request and never reaches here — see [`Self::reconfigure`], the one writer of a
    /// feed's payload.
    #[must_use]
    pub fn begin_reconfiguration(&mut self, now: Instant) -> Instant {
        let settled = self.settles_into.unwrap_or(self.status);
        let due = self.enter_transient(FeedStatus::Updating, now);
        self.settles_into = Some(settled);
        due
    }

    /// Store the payload a full-overwrite update of this feed states — its name, its
    /// whole output list and its access role — and return the instant the
    /// reconfiguration the feed now carries falls due.
    ///
    /// The payload REPLACES the stored one, so a feed reconfigured this way carries the
    /// transient status for the span of the work ([`Self::begin_reconfiguration`]) and
    /// an access role the request omits is cleared.
    ///
    /// EXCEPT when the request is the reconfiguration already in flight once more:
    /// `UpdateFeed` is `@idempotent` and carries no `@idempotencyToken`, so the
    /// request's own members are its key, exactly as a create's are
    /// ([`ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]). A
    /// request stating the payload this feed already holds, while the reconfiguration
    /// that stored it is still in flight, is that same request once more — what an SDK
    /// repeats when it never saw the first response — and several identical requests
    /// have the effect of ONE. Such a repeat therefore writes nothing and the move the
    /// clock owes stays due at the instant the first call set: retrying cannot re-enter
    /// the transient status, nor postpone the feed's return to the status that call
    /// suspended, any more than repeating a delete inside its own window can postpone
    /// the removal ([`Self::enter_transient`] is not reached at all). A request stating a
    /// DIFFERENT payload is a reconfiguration of its own, whatever status the feed is in.
    #[must_use]
    pub fn reconfigure(
        &mut self,
        name: String,
        outputs: Vec<Output>,
        access_role_arn: Option<String>,
        now: Instant,
    ) -> Instant {
        let repeats_the_work_in_flight = self.status == FeedStatus::Updating
            && self.name == name
            && self.outputs == outputs
            && self.access_role_arn == access_role_arn;
        if let Some(due) = self
            .transition_due_at
            .filter(|_| repeats_the_work_in_flight)
        {
            return due;
        }
        self.name = name;
        self.outputs = outputs;
        self.access_role_arn = access_role_arn;
        self.begin_reconfiguration(now)
    }

    /// Put this feed into a SETTLED status, immediately — the counterpart of
    /// [`Self::enter_transient`] for an operation that COMPLETES when it answers, so the
    /// feed owes the clock nothing once it has.
    ///
    /// As with a transient move, it is this outcome that decides where the feed goes, so
    /// a reconfiguration whose span had not yet fallen due no longer has a status to
    /// settle back into ([`Self::settles_into`]) — the payload that reconfiguration
    /// stored is on the record already, and the status it suspended has been overtaken
    /// by this one.
    pub fn settle_into(&mut self, status: FeedStatus) {
        self.status = status;
        self.transition_due_at = None;
        self.settles_into = None;
    }

    /// The media input at `position` in this feed's input set, or `None` when the feed
    /// exposes no input there.
    ///
    /// The media data plane addresses an input by its position
    /// (`/v1/feed/{id}/input/{n}/…`), so a position that names no input of the feed is
    /// exactly a position past the input set the feed was minted with
    /// ([`minted_audio_inputs`]).
    pub fn audio_input_at(&self, position: usize) -> Option<&str> {
        self.audio_inputs.get(position).map(String::as_str)
    }

    /// The output of this feed named `name`, or `None` when it holds none.
    pub fn output_named(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|output| output.name == name)
    }

    /// This feed's SETTLED lifecycle status: the status the clock settles it into while
    /// a transition it owes is still in flight ([`Self::settles_into`]), and the stored
    /// status otherwise.
    ///
    /// What an operation that COMPLETES when it answers reports: a reconfiguration
    /// suspends the feed's lifecycle status without changing it, so the status such a
    /// call hands back is the one the feed is in once the work it answered for is done.
    pub fn settled_status(&self) -> FeedStatus {
        self.settles_into.unwrap_or(self.status)
    }

    /// Whether any of this feed's outputs references `dictionary_id`, i.e. names it in
    /// its subtitling config ([`Output::referenced_dictionary`]).
    pub fn references_dictionary(&self, dictionary_id: &str) -> bool {
        self.outputs
            .iter()
            .any(|output| output.referenced_dictionary() == Some(dictionary_id))
    }

    /// Whether any of this feed's outputs references the function `function_arn`, i.e.
    /// names it in its function config ([`function_reference`]).
    ///
    /// Enabled or disabled alike: "the IDs of the feeds that have a function output,
    /// enabled or disabled, that references this function"
    /// (`CreateFunctionResponse$references`), so an output's own status does not change
    /// whether the feed references the function.
    pub fn references_function(&self, function_arn: &str) -> bool {
        self.outputs
            .iter()
            .any(|output| function_reference(&output.output_config) == Some(function_arn))
    }

    /// Whether any of this feed's outputs references ONE version of `question_set_id`,
    /// i.e. names that set at that version in a contextual-metadata output's custom
    /// questions ([`question_set_references`]).
    pub fn references_question_set_version(&self, question_set_id: &str, version: &str) -> bool {
        self.outputs.iter().any(|output| {
            question_set_references(&output.output_config)
                .into_iter()
                .any(|reference| {
                    reference.question_set_id == question_set_id && reference.version == version
                })
        })
    }

    /// Whether any of this feed's outputs references ANY version of `question_set_id` —
    /// what holds a `DeleteQuestionSet` back.
    pub fn references_question_set(&self, question_set_id: &str) -> bool {
        self.outputs.iter().any(|output| {
            question_set_references(&output.output_config)
                .into_iter()
                .any(|reference| reference.question_set_id == question_set_id)
        })
    }

    /// The outputs of this feed as the `GetOutputList` every feed response carries.
    pub fn get_outputs(&self) -> Vec<GetOutput> {
        self.outputs.iter().map(Output::to_get_output).collect()
    }

    /// Mark every output of this feed `DISABLED`.
    ///
    /// `DisassociateFeed` "releases the resource (the source media) that is associated
    /// with this feed. The outputs in the feed become DISABLED."
    pub fn disable_outputs(&mut self) {
        for output in &mut self.outputs {
            output.status = OUTPUT_STATUS_DISABLED.to_string();
        }
    }

    /// Which of `requested` — the outputs a request asks this feed to hold, already in
    /// their STORED form ([`Output::new`]) — an append would really add.
    ///
    /// An output the feed ALREADY holds exactly as `requested` describes it is in place
    /// already, so it is neither appended a second time nor refused: `AssociateFeed` is
    /// `@idempotent` and its `associatedResourceName` is the operation's
    /// `@idempotencyToken`, so the request an SDK repeats when it never saw the first
    /// response is that one request once more, and its effect has to be applied ONCE. A
    /// repeat whose outputs are all in place therefore adds nothing, and one that also
    /// carries an output the feed does not hold yet adds exactly that one.
    ///
    /// An output whose NAME the feed holds under a DIFFERENT definition is not in place
    /// and is not appendable either: appending it would MODIFY the stored output, which
    /// this operation cannot do ("You can't modify an existing output (for example to
    /// change its name). Instead, use UpdateFeed"). That one is refused, and the `Err`
    /// carries the name the REQUEST supplied for it, which is all the refusal's sentence
    /// ([`output_not_appendable_message`]) names.
    pub fn outputs_to_append(&self, requested: Vec<Output>) -> Result<Vec<Output>, String> {
        let mut to_append = Vec::with_capacity(requested.len());
        for candidate in requested {
            match self.outputs.iter().find(|held| held.name == candidate.name) {
                // Already in place, exactly as this request describes it.
                Some(held) if *held == candidate => {}
                // The same name carrying something else: a modification.
                Some(_) => return Err(candidate.name),
                None => to_append.push(candidate),
            }
        }
        Ok(to_append)
    }

    /// The name of the resource currently associated with this feed, if any.
    pub fn associated_resource_name(&self) -> Option<&str> {
        self.association
            .as_ref()
            .map(|association| association.associated_resource_name.as_str())
    }

    /// Whether a release naming `resource_name` has ALREADY been served for this feed,
    /// i.e. whether a `DisassociateFeed` carrying that name is the same release once more
    /// rather than a new request ([`Self::released_resource_name`]).
    pub fn has_served_release_of(&self, resource_name: &str) -> bool {
        self.released_resource_name.as_deref() == Some(resource_name)
    }

    /// The tags of this feed as the `TagMap` every feed response carries.
    ///
    /// As with [`Dictionary::tag_map`], an untagged feed reports an EMPTY map rather
    /// than an absent member.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// Whether this feed is still EXACTLY what a `CreateFeed` left behind, in every
    /// respect a create does not state: in a lifecycle status a create produces, with
    /// no resource associated, no release served, no policy attached, no schedule action
    /// on it, nothing else owed to the clock and no deletion accepted.
    ///
    /// A feed some later call has moved on is not a record a repeated create can answer
    /// with ([`ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]):
    /// the caller of that first create saw its response and acted on it, so a request
    /// arriving now is a create of its own and not that one retried. What it reads is
    /// the record's CURRENT content, not its history, so a feed an update changed and
    /// then changed back is once more a feed that create would have produced.
    pub fn is_as_created(&self) -> bool {
        // A create mints the feed in `CREATING` and the clock settles it into
        // `AVAILABLE` ([`FEED_CREATION_SPAN`]), so either of those lifecycle statuses is
        // one a create produces — as the dictionary's counterpart also holds
        // ([`Dictionary::is_as_created`]). Every other status is some later call's.
        let lifecycle_a_create_produces = match self.status {
            FeedStatus::Creating => true,
            FeedStatus::Available => self.transition_due_at.is_none(),
            FeedStatus::Active | FeedStatus::Updating | FeedStatus::Deleting
            | FeedStatus::Archived => false,
        };
        lifecycle_a_create_produces
            && self.association.is_none()
            && self.released_resource_name.is_none()
            && self.policy.is_none()
            // A feed a `CreateScheduleAction` has put an action on is one a later call
            // moved on, exactly as an association or a policy makes it: a create leaves
            // the feed with no schedule action.
            && self.schedule_actions.is_empty()
    }

    /// The `FeedSummary` projection `ListFeeds` reports.
    pub fn summary(&self) -> FeedSummary {
        FeedSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            association: self.association.clone(),
            status: self.status.as_wire().to_string(),
        }
    }
}

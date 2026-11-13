// SPDX-License-Identifier: CC0-1.0

//! One repository's own behaviour: how a repository record is built, how its
//! references resolve, how its history is walked and how its stored objects and
//! tree snapshots are read and written.
//!
//! Split out of [`super`] so the state root holds the service's records and its
//! shared vocabulary while the repository's operations live together in one
//! file. Everything here is reached through [`super::Repository`], so the module
//! adds no new names.

use super::*;

impl Repository {
    /// Build a repository, deriving every generated field from the name, the
    /// region and the creation instant.
    pub fn new(
        name: &str,
        description: Option<String>,
        kms_key_id: Option<String>,
        tags: BTreeMap<String, String>,
        region: &str,
        created: SystemTime,
    ) -> Self {
        Self {
            repository_id: repository_id(name, created),
            account_id: ::engine::emulator_account_id().to_string(),
            name: name.to_string(),
            region: region.to_string(),
            description,
            default_branch: None,
            arn: ::engine::emulator_arn("codecommit", region, name),
            // Every repository is encrypted, so a request that names no key of
            // its own takes the service's managed key rather than none.
            kms_key_id: canonical_encryption_key(
                kms_key_id.as_deref().unwrap_or(DEFAULT_ENCRYPTION_KEY_ALIAS),
                region,
            ),
            tagging_id: tagging_id(name, created),
            creation_date: created,
            last_modified_date: created,
            tags,
            branches: BTreeMap::new(),
            git_tags: BTreeMap::new(),
            commits: BTreeMap::new(),
            objects: BTreeMap::new(),
            associated_templates: BTreeSet::new(),
            next_commit_sequence: 0,
            comments: BTreeMap::new(),
            comment_tokens: BTreeMap::new(),
            next_comment_sequence: 0,
            unreferenced_commits: BTreeSet::new(),
            pull_requests: BTreeMap::new(),
            pull_request_tokens: BTreeMap::new(),
            merge_commits: BTreeMap::new(),
            trigger_configuration: None,
        }
    }

    /// Point `branch` at `commit_id`, and record the move on every OPEN pull
    /// request that reads from that branch.
    ///
    /// The ONE place a branch tip is set, because moving a branch moves more
    /// than the reference: "If the pull request branch is updated by a push
    /// while the pull request is open, the commit ID changes to reflect the new
    /// tip of the branch", so each pull request whose SOURCE this branch is
    /// follows it — the new tip, and the merge base that tip has with the
    /// destination — and that update is appended to the pull request's event
    /// log. `actor` is the identity that moved the reference, for a transport
    /// that carries one; the git transport authenticates with git credentials
    /// rather than a caller's own, so a push names none.
    ///
    /// A pull request that is no longer open is a record of what it was merged
    /// or closed at, so a later move of its source branch does not touch it.
    ///
    /// Answers the change each updated pull request recorded, for the caller to
    /// publish once this repository's store guard is gone
    /// ([`pull_request_events::publish_all`]): a reference move is observable to a
    /// consumer of the account's default event bus, and this is the one place that
    /// knows which pull requests it moved. A move that updated no pull request —
    /// a branch no open pull request reads from — answers nothing.
    #[must_use = "a recorded pull-request change is published onto the default event bus"]
    pub fn set_branch_tip(
        &mut self,
        branch: &str,
        commit_id: &str,
        at: SystemTime,
        actor: Option<&str>,
    ) -> Vec<pull_request_events::PullRequestStateChange> {
        let previous = self
            .branches
            .insert(
                branch.to_string(),
                Branch {
                    name: branch.to_string(),
                    commit_id: commit_id.to_string(),
                },
            )
            .map(|replaced| replaced.commit_id);
        if previous.as_deref() == Some(commit_id) {
            // The reference already pointed there, so nothing moved — and
            // nothing reading from it changed either.
            return Vec::new();
        }

        let reference = qualified_reference(branch);
        // What each affected target becomes, resolved while the repository is
        // only READ: the new merge base comes from the history this branch now
        // reaches. OFF THE ASYNC WORKER and charged to ONE budget for the whole
        // move, exactly as the merge base a new pull request reports is, so the
        // walk this costs is bounded however many pull requests read from the
        // branch.
        //
        // Each entry's merge base is `Some(resolved)` — `Some(None)` where the
        // two histories share no commit at all — or `None` where the walk could
        // not finish within what is left of the budget. The two are NOT the
        // same answer: a base the request could not resolve leaves the stored
        // one alone, so what a pull request reports does not depend on how many
        // other pull requests happen to read the same branch.
        let mut budget = merge::MergeBudget::for_one_request();
        let moved: Vec<(u64, usize, String, Option<Option<String>>)> =
            crate::service::off_the_async_worker(|| {
                let mut moved = Vec::new();
                for (pull_request_id, pull_request) in &self.pull_requests {
                    if pull_request.status != PULL_REQUEST_STATUS_OPEN {
                        continue;
                    }
                    for (index, target) in pull_request.targets.iter().enumerate() {
                        if target.source_reference != reference
                            || target.source_commit == commit_id
                        {
                            continue;
                        }
                        let merge_base = match merge::merge_base(
                            self,
                            commit_id,
                            &target.destination_commit,
                            &mut budget,
                        ) {
                            Ok(base) => Some(Some(base)),
                            Err(merge::MergeProblem::NoMergeBase) => Some(None),
                            Err(_) => None,
                        };
                        moved.push((
                            *pull_request_id,
                            index,
                            target.source_commit.clone(),
                            merge_base,
                        ));
                    }
                }
                moved
            });

        let repository_name = self.name.clone();
        let mut updated: Vec<u64> = Vec::new();
        for (pull_request_id, index, before_commit_id, resolved_base) in moved {
            let Some(pull_request) = self.pull_requests.get_mut(&pull_request_id) else {
                continue;
            };
            let mut merge_base = None;
            if let Some(target) = pull_request.targets.get_mut(index) {
                target.source_commit = commit_id.to_string();
                if let Some(resolved_base) = resolved_base {
                    target.merge_base = resolved_base;
                }
                merge_base = target.merge_base.clone();
            }
            // The reviewable state of the pull request moved with its source
            // tip, so the next read of it reports another `revisionId`.
            pull_request.revision = pull_request.revision.saturating_add(1);
            pull_request.record_event(
                at,
                actor.map(str::to_string),
                PullRequestEventDetail::SourceReferenceUpdated {
                    repository_name: repository_name.clone(),
                    before_commit_id,
                    after_commit_id: commit_id.to_string(),
                    merge_base,
                },
            );
            updated.push(pull_request_id);
        }
        // Rendered from the pull requests as they now stand, so what is published
        // is what the next read of each of them reports.
        updated
            .into_iter()
            .flat_map(|pull_request_id| self.pull_request_changes(pull_request_id, 1))
            .collect()
    }

    /// Keep a commit no ref names readable: record it as a retained unreferenced
    /// commit, so [`Repository::prune_history`] treats it as a root.
    ///
    /// The oldest retained ids are released once there are more than
    /// [`MAX_UNREFERENCED_COMMITS`] of them, which is what keeps a repository's
    /// retained history bounded when nothing ever references these commits.
    pub fn retain_unreferenced(&mut self, commit_id: String) {
        self.unreferenced_commits.insert(commit_id);
        while self.unreferenced_commits.len() > MAX_UNREFERENCED_COMMITS {
            // The commit with the lowest write sequence is the oldest; an id
            // whose projection is already gone is older than any still held.
            let oldest = self
                .unreferenced_commits
                .iter()
                .min_by_key(|id| {
                    self.commits
                        .get(id.as_str())
                        .map_or(0, |commit| commit.sequence.saturating_add(1))
                })
                .cloned();
            match oldest {
                Some(id) => {
                    self.unreferenced_commits.remove(&id);
                }
                None => break,
            }
        }
    }

    /// How many of this repository's pull requests are still open — what
    /// [`MAX_OPEN_PULL_REQUESTS_PER_REPOSITORY`] bounds.
    pub fn open_pull_requests(&self) -> usize {
        self.pull_requests
            .values()
            .filter(|pull_request| pull_request.status == PULL_REQUEST_STATUS_OPEN)
            .count()
    }

    /// This repository as the modeled `RepositoryMetadata` shape — the one place
    /// that projection is written, so `CreateRepository` and `GetRepository`
    /// cannot disagree about a stored repository.
    ///
    /// The clone URLs are derived from `endpoint`, the address the ASKING request
    /// arrived at, because that is the address this emulator's git transport
    /// answers on; they are not stored, so they cannot go stale when the emulator
    /// is reached on a different endpoint than the one that created the
    /// repository.
    pub fn metadata(&self, endpoint: &ServingEndpoint) -> RepositoryMetadata {
        RepositoryMetadata {
            account_id: Some(self.account_id.clone()),
            repository_id: Some(self.repository_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_description: self.description.clone(),
            default_branch: self.default_branch.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            clone_url_http: Some(endpoint.clone_url_http(&self.name)),
            clone_url_ssh: Some(endpoint.clone_url_ssh(&self.name)),
            arn: Some(self.arn.clone()),
            kms_key_id: Some(self.kms_key_id.clone()),
        }
    }

    /// Rename this repository, recomputing every field CodeCommit derives from the
    /// repository name — the ARN addresses a repository BY name — and moving the
    /// last-modified instant to the rename. Both clone URLs name the repository too,
    /// and follow the rename for free: they are derived from the stored name and the
    /// asking request's endpoint on every read (see [`Repository::metadata`]).
    ///
    /// Everything that belongs to the resource rather than to its name (the
    /// repository id, the creation date, the description, the KMS key, the tags, the
    /// branches, the tags, the commits and the git objects) carries over untouched,
    /// so a renamed repository is the same repository under a new name. The caller
    /// re-keys the store, which is keyed by `repositoryName`.
    pub fn rename(&mut self, new_name: &str, renamed: SystemTime) {
        self.name = new_name.to_string();
        self.arn = ::engine::emulator_arn("codecommit", &self.region, new_name);
        self.last_modified_date = renamed;
    }

    /// Point this repository at another KMS encryption key, answering the key it
    /// was encrypted under before — the `originalKmsKeyId` a rotation reports.
    /// A repository always carries a key, so there is always one to report.
    ///
    /// Both keys are the [`canonical_encryption_key`] form of their selector, so
    /// a rotation reports the same key the metadata of this repository reports
    /// before and after it.
    ///
    /// Only which key the repository DEPENDS on changes: its identity, its
    /// metadata and its whole history carry over, and the last-modified instant
    /// records the change. A request naming the key already in effect changes
    /// nothing at all, so it leaves that instant where it is too.
    pub fn rotate_encryption_key(&mut self, kms_key_id: &str, rotated: SystemTime) -> String {
        let canonical = canonical_encryption_key(kms_key_id, &self.region);
        let original = ::std::mem::replace(&mut self.kms_key_id, canonical);
        if original != self.kms_key_id {
            self.last_modified_date = rotated;
        }
        original
    }

    /// The commit a `commitSpecifier` names. `CommitName` documents the forms as
    /// "the branch, tag, HEAD, or other fully qualified reference used to identify
    /// a commit", so a branch name, the fully qualified reference of a branch
    /// (`refs/heads/<branch>`), `HEAD` (the repository's default branch) and the id
    /// of a commit this repository holds all resolve here. `None` when the
    /// specifier names none of them. This repository holds no tags, so a tag
    /// reference resolves to nothing until tags are stored.
    pub fn resolve_commit(&self, specifier: &str) -> Option<&Commit> {
        // A branch of that literal name comes first, so the reference forms below
        // only resolve what the bare name could not — a repository that really
        // holds a branch called `HEAD` is still addressable by name.
        if let Some(branch) = self.branches.get(specifier) {
            return self.commits.get(&branch.commit_id);
        }
        let referenced = match specifier {
            "HEAD" => self.default_branch.as_deref(),
            other => other.strip_prefix("refs/heads/"),
        };
        if let Some(branch) = referenced.and_then(|name| self.branches.get(name)) {
            return self.commits.get(&branch.commit_id);
        }
        self.commits.get(specifier)
    }

    /// The BRANCH a reference specifier names, in the same order
    /// [`Repository::resolve_commit`] resolves one: a branch of that literal
    /// name, then the fully qualified reference form (`refs/heads/<branch>`),
    /// then `HEAD` (the repository's default branch).
    ///
    /// `None` where the specifier names no branch of this repository — a commit
    /// id and a tag reference name a commit without naming a branch — so an
    /// operation that has to MOVE a reference can tell which one the caller
    /// named.
    pub fn resolve_branch(&self, specifier: &str) -> Option<&str> {
        if let Some(branch) = self.branches.get(specifier) {
            return Some(branch.name.as_str());
        }
        let referenced = match specifier {
            "HEAD" => self.default_branch.as_deref(),
            other => other.strip_prefix(HEADS_PREFIX),
        };
        referenced
            .and_then(|name| self.branches.get(name))
            .map(|branch| branch.name.as_str())
    }

    /// The object this name addresses.
    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.get(id)
    }

    /// The bytes of a blob, or `None` when this name is not a blob this
    /// repository holds.
    pub fn blob(&self, id: &str) -> Option<&[u8]> {
        self.objects
            .get(id)
            .filter(|object| object.kind == Kind::Blob)
            .map(|object| object.data.as_slice())
    }

    /// Store an object, reporting its name. Storing the same object twice is the
    /// same as storing it once: the name IS the content.
    pub fn insert_object(&mut self, object: Object) -> String {
        let id = object.id();
        self.objects.entry(id.clone()).or_insert(object);
        id
    }

    /// Store a file's bytes as a blob, reporting its `blobId`.
    pub fn insert_blob(&mut self, content: Vec<u8>) -> String {
        self.insert_object(Object::new(Kind::Blob, content))
    }

    /// Total payload bytes the object store holds.
    pub fn object_bytes(&self) -> usize {
        self.objects
            .values()
            .map(|object| object.data.len())
            .fold(0usize, usize::saturating_add)
    }

    /// Commit a whole tree snapshot: write the tree objects and the commit
    /// object, record the commit projection, and report `(commitId, treeId)`.
    ///
    /// The blobs the snapshot names must already be stored (see
    /// [`Repository::insert_blob`]), which is what makes the commit's tree
    /// complete in the object store and therefore serveable to a git client —
    /// and what makes every entry's `blobId` an object name, so the tree encodes.
    /// `None` when one does not.
    pub fn commit_snapshot(
        &mut self,
        files: BTreeMap<String, FileEntry>,
        parents: Vec<String>,
        author: UserInfo,
        committer: UserInfo,
        message: Option<&str>,
    ) -> Option<(String, String)> {
        let (tree_id, trees) = tree_objects(&files)?;
        for tree in trees {
            self.insert_object(tree);
        }
        let commit = git_object::encode_commit(&git_object::CommitFields {
            tree: tree_id.clone(),
            parents: parents.clone(),
            author: git_identity(&author),
            committer: git_identity(&committer),
            message: message.unwrap_or_default().to_string(),
        });
        let commit_id = self.insert_object(commit);
        let sequence = self.next_commit_sequence;
        self.next_commit_sequence = sequence.saturating_add(1);
        self.commits.insert(
            commit_id.clone(),
            Commit {
                commit_id: commit_id.clone(),
                tree_id: tree_id.clone(),
                parents,
                message: message.map(str::to_string),
                author,
                committer,
                files,
                sequence,
            },
        );
        Some((commit_id, tree_id))
    }

    /// Project the commit object `id` — and its ancestors, as far as the objects
    /// present and `budget` allow — into [`Repository::commits`], so the control
    /// plane can read what the git transport stored.
    ///
    /// `budget` is ONE request's whole projection allowance, not one call's, so a
    /// request carrying many ref updates cannot multiply this work: see
    /// [`ProjectionBudget`].
    ///
    /// `Err` names the first commit whose tree the projection cannot hold, or the
    /// budget it ran past, which is what lets a push be refused instead of
    /// accepted into a state the control plane could not read.
    pub fn project_history(
        &mut self,
        id: &str,
        budget: &mut ProjectionBudget,
    ) -> Result<(), String> {
        // Oldest-last walk from the tip, stopping at a commit already projected.
        let mut order: Vec<String> = Vec::new();
        let mut queue: Vec<String> = vec![id.to_string()];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(next) = queue.pop() {
            // Charged BEFORE the skips below: every parent id comes from a pushed
            // commit object, so a history whose parents name nothing this
            // repository holds would otherwise drain its whole queue uncharged.
            // This is also where the deadline is checked for this walk.
            budget.entry()?;
            if !seen.insert(next.clone()) || self.commits.contains_key(&next) {
                continue;
            }
            if order.len() >= MAX_COMMITS_RETAINED {
                break;
            }
            let Some(object) = self.objects.get(&next) else {
                // An ancestor this repository does not hold: the history ends
                // here as far as the projection is concerned.
                continue;
            };
            if object.kind != Kind::Commit {
                return Err(format!("{next} is not a commit"));
            }
            let fields = git_object::decode_commit(&object.data)
                .ok_or_else(|| format!("{next} is not a well-formed commit object"))?;
            order.push(next);
            for parent in fields.parents {
                queue.push(parent);
            }
        }
        // Ancestors first, so a commit's sequence number grows with history and
        // `prune_history` drops the oldest first.
        for commit_id in order.into_iter().rev() {
            self.project_commit(&commit_id, budget)?;
        }
        Ok(())
    }

    /// Project one commit object into [`Repository::commits`].
    pub(super) fn project_commit(&mut self, id: &str, budget: &mut ProjectionBudget) -> Result<(), String> {
        budget.commit()?;
        let object = self
            .objects
            .get(id)
            .ok_or_else(|| format!("the object {id} is missing"))?;
        let fields = git_object::decode_commit(&object.data)
            .ok_or_else(|| format!("{id} is not a well-formed commit object"))?;
        let files = self.flatten_tree(&fields.tree, budget)?;
        let (author_name, author_email, author_date) = git_object::split_identity(&fields.author);
        let (committer_name, committer_email, committer_date) =
            git_object::split_identity(&fields.committer);
        let sequence = self.next_commit_sequence;
        self.next_commit_sequence = sequence.saturating_add(1);
        self.commits.insert(
            id.to_string(),
            Commit {
                commit_id: id.to_string(),
                tree_id: fields.tree,
                parents: fields.parents,
                message: Some(fields.message).filter(|message| !message.is_empty()),
                author: UserInfo {
                    name: Some(author_name),
                    email: Some(author_email),
                    date: Some(author_date),
                },
                committer: UserInfo {
                    name: Some(committer_name),
                    email: Some(committer_email),
                    date: Some(committer_date),
                },
                files,
                sequence,
            },
        );
        Ok(())
    }

    /// The flattened snapshot a tree object describes: every file in it, keyed by
    /// its path from the root.
    ///
    /// A gitlink entry (a submodule's commit embedded in a tree) names no blob
    /// and so is not a file; it is left out of the snapshot.
    ///
    /// The walk is charged to `budget`, ONE request's whole projection allowance,
    /// entry by entry — NOT just bounded by the depth and the file count: a tree
    /// is a DAG, so a handful of pushed tree objects can name one another and
    /// present exponentially many paths while inserting no file at all (every leaf
    /// a gitlink, or an empty tree), and neither the depth nor the file count
    /// would ever trip. A visited set would be the wrong bound here: two
    /// directories with identical content legitimately SHARE one tree object, and
    /// both have to appear in the snapshot, so a repeated subtree must be walked
    /// again — what is refused is the total work, not the repetition.
    pub fn flatten_tree(
        &self,
        root: &str,
        budget: &mut ProjectionBudget,
    ) -> Result<BTreeMap<String, FileEntry>, String> {
        let mut files = BTreeMap::new();
        self.flatten_tree_into(root, "", 0, budget, &mut files)?;
        Ok(files)
    }

    pub(super) fn flatten_tree_into(
        &self,
        tree_id: &str,
        prefix: &str,
        depth: usize,
        budget: &mut ProjectionBudget,
        files: &mut BTreeMap<String, FileEntry>,
    ) -> Result<(), String> {
        if depth >= MAX_PATH_SEGMENTS {
            return Err(format!(
                "the tree {tree_id} is nested deeper than {MAX_PATH_SEGMENTS} levels"
            ));
        }
        let object = self
            .objects
            .get(tree_id)
            .ok_or_else(|| format!("the tree {tree_id} is missing"))?;
        if object.kind != Kind::Tree {
            return Err(format!("{tree_id} is not a tree"));
        }
        let entries = git_object::decode_tree(&object.data)
            .ok_or_else(|| format!("{tree_id} is not a well-formed tree object"))?;
        for entry in entries {
            // Every entry costs one, whether or not it becomes a file, which is
            // what bounds a shared-subtree DAG.
            budget.entry()?;
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if entry.is_tree() {
                self.flatten_tree_into(&entry.id, &path, depth + 1, budget, files)?;
                continue;
            }
            if entry.mode == git_object::MODE_GITLINK {
                continue;
            }
            let size = self
                .objects
                .get(&entry.id)
                .filter(|object| object.kind == Kind::Blob)
                .ok_or_else(|| format!("the blob {} for {path} is missing", entry.id))?
                .data
                .len();
            if files.len() >= MAX_FILES_PER_TREE {
                return Err(format!(
                    "the tree {tree_id} holds more than the {MAX_FILES_PER_TREE} files this emulator keeps per tree"
                ));
            }
            files.insert(
                path,
                FileEntry {
                    blob_id: entry.id,
                    file_mode: file_mode_of(&entry.mode).to_string(),
                    size: i64::try_from(size).unwrap_or(i64::MAX),
                },
            );
        }
        Ok(())
    }

    /// Every commit id reachable from `commit_id` by following parents, including
    /// `commit_id` itself: the history a read at that commit sees. An id whose
    /// commit [`Repository::prune_history`] has dropped is still reported, because
    /// a retained commit's `parents` keeps naming it.
    ///
    /// The UNCHARGED form, for a walk whose size is already bounded by what the
    /// projection retained ([`MAX_COMMITS_RETAINED`] commits and the parents they
    /// name). A walk a caller can drive past that — one that resolves a merge
    /// base over a history of any shape — charges every commit it visits to its
    /// own request budget instead; see [`merge`].
    pub fn ancestry(&self, commit_id: &str) -> BTreeSet<String> {
        let mut reachable = BTreeSet::new();
        let mut pending = vec![commit_id.to_string()];
        while let Some(id) = pending.pop() {
            if !reachable.insert(id.clone()) {
                continue;
            }
            if let Some(commit) = self.commits.get(&id) {
                pending.extend(commit.parents.iter().cloned());
            }
        }
        reachable
    }

    /// Every ref this repository holds, as `(full refname, object name)`, in
    /// refname order — the order a git client is advertised them in.
    pub fn refs(&self) -> Vec<(String, String)> {
        let mut refs: Vec<(String, String)> = self
            .branches
            .values()
            .map(|branch| (format!("{HEADS_PREFIX}{}", branch.name), branch.commit_id.clone()))
            .chain(
                self.git_tags
                    .iter()
                    .map(|(name, id)| (format!("{TAGS_PREFIX}{name}"), id.clone())),
            )
            .collect();
        refs.sort();
        refs
    }

    /// The commit an annotated tag (or a chain of them) finally points at, or
    /// `None` when the object is not a tag and not a commit this repository
    /// holds.
    pub fn peel(&self, id: &str) -> Option<String> {
        let mut current = id.to_string();
        // A tag of a tag of a tag … is bounded, so a cycle cannot spin here.
        for _ in 0..16 {
            let object = self.objects.get(&current)?;
            match object.kind {
                Kind::Commit => return Some(current),
                Kind::Tag => current = git_object::decode_tag(&object.data)?.object,
                _ => return None,
            }
        }
        None
    }

    /// The first object name `roots` reach that this repository does NOT hold, or
    /// `None` when the closure is complete.
    ///
    /// What a git host checks before it moves a ref: a ref whose closure is
    /// incomplete is one a later fetch cannot serve, so the push that would create
    /// it is refused instead.
    pub fn first_missing_object(&self, roots: &[String]) -> Option<String> {
        self.reachable_from(roots)
            .into_iter()
            .find(|id| !self.objects.contains_key(id))
    }

    /// [`Repository::first_missing_object`], charged to ONE request's budget.
    ///
    /// The form a caller-driven walk uses. A single request may ask for this once
    /// per ref it updates, so every walk charges the same [`ProjectionBudget`] —
    /// what is bounded is the whole request's walking, not each walk — and that
    /// budget's deadline stops a walk that runs long however few entries it has
    /// visited. `Err` carries the reason, which the caller reports as the refusal.
    pub fn first_missing_object_within(
        &self,
        roots: &[String],
        budget: &mut ProjectionBudget,
    ) -> Result<Option<String>, String> {
        Ok(self
            .walk_reachable(roots, Some(budget))?
            .into_iter()
            .find(|id| !self.objects.contains_key(id)))
    }

    /// Every object name reachable from `roots`, following commits to their
    /// trees and parents, trees to their entries and tags to their targets.
    ///
    /// Unbudgeted, for this service's OWN walks rather than a caller's: one call
    /// visits each object at most once, so its cost is bounded by what the
    /// repository holds ([`MAX_OBJECTS_PER_REPOSITORY`],
    /// [`MAX_OBJECT_BYTES_PER_REPOSITORY`]). A walk a REQUEST can drive repeatedly
    /// goes through [`Repository::first_missing_object_within`], which charges a
    /// budget, because the number of walks is then the caller's to choose.
    pub fn reachable_from(&self, roots: &[String]) -> BTreeSet<String> {
        // No budget is charged, so the walk cannot be refused.
        self.walk_reachable(roots, None).unwrap_or_default()
    }

    /// The one reachability walk, charged to `budget` when there is one.
    pub(super) fn walk_reachable(
        &self,
        roots: &[String],
        mut budget: Option<&mut ProjectionBudget>,
    ) -> Result<BTreeSet<String>, String> {
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut queue: Vec<String> = roots.to_vec();
        while let Some(next) = queue.pop() {
            if let Some(budget) = budget.as_deref_mut() {
                // Charged per object TAKEN from the queue, which counts the
                // entries every tree pushed onto it, so a tree naming many
                // entries costs what it enqueued.
                budget.entry()?;
            }
            if !reached.insert(next.clone()) {
                continue;
            }
            let Some(object) = self.objects.get(&next) else {
                continue;
            };
            match object.kind {
                Kind::Blob => {}
                Kind::Commit => {
                    if let Some(fields) = git_object::decode_commit(&object.data) {
                        queue.push(fields.tree);
                        queue.extend(fields.parents);
                    }
                }
                Kind::Tree => {
                    if let Some(entries) = git_object::decode_tree(&object.data) {
                        queue.extend(entries.into_iter().map(|entry| entry.id));
                    }
                }
                Kind::Tag => {
                    if let Some(fields) = git_object::decode_tag(&object.data) {
                        queue.push(fields.object);
                    }
                }
            }
        }
        Ok(reached)
    }

    /// Release what no ref reaches, and keep the commit projection bounded.
    ///
    /// Objects are kept exactly when a ref reaches them, which is what git's own
    /// garbage collection does: an object a force-push or a branch deletion
    /// orphaned is released. The projection additionally drops the oldest commits
    /// beyond [`MAX_COMMITS_RETAINED`] (never a ref's tip), so an older commit id
    /// becomes one the repository no longer holds while every ref stays readable.
    pub fn prune_history(&mut self) {
        // Every ref, AND every unreferenced commit this repository was asked to
        // keep: an id `CreateUnreferencedMergeCommit` handed a caller has to stay
        // readable, so it roots its objects just as a ref does.
        let roots: Vec<String> = self
            .refs()
            .into_iter()
            .map(|(_, id)| id)
            .chain(self.unreferenced_commits.iter().cloned())
            .collect();
        let reachable = self.reachable_from(&roots);
        self.objects.retain(|id, _| reachable.contains(id));
        self.commits.retain(|id, _| reachable.contains(id));

        let tips: BTreeSet<String> = roots
            .iter()
            .filter_map(|id| self.peel(id))
            .chain(roots.iter().cloned())
            .collect();
        if self.commits.len() > MAX_COMMITS_RETAINED {
            let mut oldest_first: Vec<(u64, String)> = self
                .commits
                .values()
                .map(|commit| (commit.sequence, commit.commit_id.clone()))
                .collect();
            oldest_first.sort_unstable();
            let mut excess = self.commits.len() - MAX_COMMITS_RETAINED;
            for (_, commit_id) in oldest_first {
                if excess == 0 {
                    break;
                }
                if tips.contains(&commit_id) {
                    continue;
                }
                self.commits.remove(&commit_id);
                excess -= 1;
            }
        }

        // A recorded merge names a commit, so the records follow the projection:
        // one whose merge commit was just released is released with it.
        self.release_stale_merge_commits();
    }
}

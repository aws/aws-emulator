// SPDX-License-Identifier: CC0-1.0

//! The AD DIRECTORY INVENTORY: which Active Directory directories the account holds and
//! what the CONTROL plane knows about each of them, so the directory DATA plane can tell a
//! directory of this account from one that is not, and can answer from facts only that
//! plane holds.
//!
//! A directory is created by the AWS Directory Service CONTROL plane (`CreateDirectory`,
//! `CreateMicrosoftAD`, `ConnectDirectory`) and deleted by it (`DeleteDirectory`); the
//! Directory Service DATA plane — which reads and writes the users, groups and memberships
//! INSIDE a directory — creates none and so cannot know from its own records whether a
//! `DirectoryId` a request names is one the account holds at all. It has to know, because
//! every one of its operations answers a directory the caller cannot reach by refusing the
//! call rather than by reporting anything about the directory.
//!
//! Four more facts of a directory are the control plane's alone, and the data plane's
//! answers rest on them:
//!
//! * whether that plane has turned the directory's DATA API on, and from WHEN, which is
//!   what decides whether the data plane may answer for it at all;
//! * the directory's own DOMAIN NAME, which is the realm every object of it is reported in
//!   and the suffix its principal and distinguished names are built on;
//! * the directory's NetBIOS SHORT NAME, which names the organizational unit a managed
//!   directory delegates to the account, and so the container the objects the data plane
//!   creates are placed in;
//! * whether an account's PASSWORD has been set, which is what makes a domain account
//!   usable: an account the data plane creates has no password — its create carries none —
//!   so the directory leaves it inactive until the control plane's password reset sets one.
//!
//! This is that record: ONE inventory in the shared engine, written by the control plane
//! when it creates a directory, when it toggles the data API, when it sets an account's
//! password and when it deletes the directory, and by the data plane for the accounts it
//! creates and deletes; read by the data plane on every call it serves. Nothing here names
//! a wire shape — the inventory holds ids, a domain name and account names — and nothing
//! here decides what a reader does with the answer: WHICH error a data-plane request gets
//! for a directory the inventory does not hold, or holds shut, is that service's own.
//!
//! The data-API toggle is recorded as the CHANGE the control plane accepted and the instant
//! it takes effect, because both toggles answer before the directory's data API has moved:
//! the clock applies it, at the same instant the control plane's own transient status
//! settles, so the two planes never disagree about whether the data API is in service. A
//! directory the control plane has just created is out of service, which is the state every
//! create leaves it in.
//!
//! One more fact belongs to the record, read by a service that BUILDS on a directory rather
//! than by the data plane: WHEN the create brings the directory into service. A create
//! answers before the directory's servers exist, so a directory the account already holds is
//! not yet one another service can be registered against; the create records the instant its
//! build finishes and the CLOCK — not a later call — is what brings it into service, for the
//! reason the data-API toggle above carries its own due instant.
//!
//! The inventory is a record of what this emulator CREATED, so a reader asking about an id
//! gets a yes or a no about this account's own directories and nothing else. It is BOUNDED
//! in both dimensions a caller can grow — see [`MAX_RECORDED_DIRECTORIES`], far above the
//! control plane's own directory quota, and [`MAX_RECORDED_ACCOUNTS_PER_DIRECTORY`] — so
//! nothing a caller loops on can grow it without end; the oldest registration is dropped
//! first, which keeps the directory, and the account, a live workflow just created
//! resolvable.
//!
//! LOCK ORDER: a service takes its OWN store first and this inventory second, and nothing
//! holds this inventory while taking another store — so two services cannot invert the
//! order between them. A reader that needs the inventory BEFORE it reaches its own store
//! takes a copy of what it needs and releases this one first.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::SystemTime;

/// How many directories the inventory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this inventory lives in memory for the lifetime
/// of the process, so without a bound a caller creating directories in a loop would grow
/// it without end. It sits far above the control plane's own per-account directory quota,
/// and past it the OLDEST registration is dropped — never a newer one — so a directory a
/// running workflow just created is always still resolvable.
pub const MAX_RECORDED_DIRECTORIES: usize = 1_000;

/// How many accounts ONE directory's record accounts for.
///
/// An EMULATOR ceiling, and the companion of [`MAX_RECORDED_DIRECTORIES`]: without it a
/// caller creating objects in a loop could grow one record without end. Past it the
/// account recorded EARLIEST is dropped, so the accounts a running workflow just created
/// are the ones that stay resolvable.
pub const MAX_RECORDED_ACCOUNTS_PER_DIRECTORY: usize = 10_000;

/// Whether one directory's data API is in service, and the change the clock still owes it.
///
/// The control plane's enable and disable both answer BEFORE the directory's data API has
/// moved, so what a toggle records here is the change and the instant it takes effect; the
/// clock — not a later call — is what applies it, at the same instant the control plane's
/// own transient status settles. A directory a create has just registered is out of
/// service: a create opens nothing.
#[derive(Debug, Clone, Copy, Default)]
struct DataAccess {
    /// Whether the data API answers for the directory, before any change still owed.
    in_service: bool,
    /// The change the clock owes — whether the data API will answer, and the instant that
    /// takes effect — or `None` when the toggle has nothing outstanding.
    owed: Option<(bool, SystemTime)>,
}

impl DataAccess {
    /// Whether the data API answers for the directory at `now`: the change the clock owes
    /// once its instant has arrived, and what the toggle last settled on until then.
    fn in_service_at(self, now: SystemTime) -> bool {
        match self.owed {
            Some((in_service, effective_at)) if now >= effective_at => in_service,
            _ => self.in_service,
        }
    }

    /// This toggle with a change the control plane just accepted recorded on it: the data
    /// API is `in_service` from `effective_at` on, and until then stands as it stands at
    /// `now` — so a change recorded while an earlier one is still outstanding does not
    /// apply that earlier one early.
    fn changing_to(self, in_service: bool, effective_at: SystemTime, now: SystemTime) -> Self {
        Self {
            in_service: self.in_service_at(now),
            owed: Some((in_service, effective_at)),
        }
    }
}

/// What the inventory holds about one account of a directory.
#[derive(Debug, Default)]
struct AccountRecord {
    /// Whether the control plane has set this account's password. An account whose
    /// password has never been set has no credential to authenticate with, which is what
    /// leaves it inactive.
    password_set: bool,
}

/// What the inventory holds about one directory.
#[derive(Debug, Default)]
struct DirectoryRecord {
    /// Whether this directory's data API is in service, and the change the clock owes it.
    data_access: DataAccess,
    /// When the create that registered this directory brings it into service — the instant
    /// its build finishes — or `None` for a registration that recorded no build to wait
    /// for.
    in_service_at: Option<SystemTime>,
    /// The fully qualified domain name the directory was created with, as the control
    /// plane's create named it. Empty for a directory registered without one.
    domain_name: String,
    /// The NetBIOS (short) name the directory was created with, as the control plane's
    /// create named it. Empty for a directory registered without one.
    short_name: String,
    /// The directory's accounts the data plane created, keyed by the account name folded
    /// to lower case — a directory account name is case insensitive.
    accounts: BTreeMap<String, AccountRecord>,
    /// The order the accounts were recorded in, so the ceiling drops the oldest first.
    account_order: VecDeque<String>,
}

/// The inventory itself: every `DirectoryId` the control plane created and has not deleted,
/// with what that plane knows about each.
///
/// Registered in the shared engine under THIS type, so the control plane that creates a
/// directory and the data plane that serves the objects inside it read one record instead
/// of each keeping its own copy.
#[derive(Debug, Default)]
pub struct AdDirectoryInventory {
    /// What the inventory holds per directory id: one lookup answers every question a
    /// reader asks — does the account hold this directory, does its data API answer, what
    /// domain is it in, and which of its accounts hold a password.
    held: HashMap<String, DirectoryRecord>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

/// The key an account name is held under: the name folded to lower case, because a
/// directory account name is case insensitive, so two spellings of one name are one
/// account.
fn account_key(account_name: &str) -> String {
    account_name.trim().to_lowercase()
}

impl AdDirectoryInventory {
    /// Record that the account holds the directory `directory_id`, its data API out of
    /// service as a create leaves it.
    ///
    /// Recording an id the inventory already holds changes nothing — neither its place in
    /// the eviction order, nor its data-access toggle, nor its domain or its accounts — so
    /// a control plane that re-registers a directory it already told the inventory about
    /// cannot age another one out, and cannot withdraw a directory whose data access it
    /// opened.
    pub fn register(&mut self, directory_id: &str) {
        if self.held.contains_key(directory_id) {
            return;
        }
        self.held
            .insert(directory_id.to_string(), DirectoryRecord::default());
        self.order.push_back(directory_id.to_string());
        while self.order.len() > MAX_RECORDED_DIRECTORIES {
            if let Some(oldest) = self.order.pop_front() {
                self.held.remove(&oldest);
            }
        }
    }

    /// [`Self::register`], with the two NAMES the directory was created with: the `Name` the
    /// control plane's create was called with, which the data plane reports as the realm of
    /// every object inside it, and the NetBIOS `ShortName` of the same create, which names
    /// the organizational unit the directory delegates to the account. `None` for a create
    /// that recorded no short name.
    ///
    /// Idempotent in the same way: a re-registration leaves the names, the toggle and the
    /// accounts as they stand.
    pub fn register_in_domain(
        &mut self,
        directory_id: &str,
        domain_name: &str,
        short_name: Option<&str>,
    ) {
        let fresh = !self.held.contains_key(directory_id);
        self.register(directory_id);
        if fresh {
            if let Some(record) = self.held.get_mut(directory_id) {
                record.domain_name = domain_name.trim().to_string();
                record.short_name = short_name.unwrap_or_default().trim().to_string();
            }
        }
    }

    /// Drop the record of `directory_id`: the directory is gone, so the inventory no longer
    /// claims the account holds it, and nothing it held about it describes anything.
    ///
    /// A no-op for an id the inventory holds no record of, so a repeated delete is harmless.
    pub fn forget(&mut self, directory_id: &str) {
        if self.held.remove(directory_id).is_some() {
            self.order.retain(|recorded| recorded != directory_id);
        }
    }

    /// Whether the account holds the directory `directory_id`.
    #[must_use]
    pub fn holds(&self, directory_id: &str) -> bool {
        self.held.contains_key(directory_id)
    }

    /// Record WHEN the create that registered `directory_id` brings it into service: the
    /// instant the directory's build finishes and it reaches the stage a service that
    /// builds on it can be registered against.
    ///
    /// A no-op for an id the inventory holds no record of — a directory the account does not
    /// hold has no build to finish, and recording one would claim the account holds it — and
    /// for one whose instant is already recorded, so a control plane that re-registers a
    /// directory it already told the inventory about cannot restart that directory's build.
    pub fn record_in_service_at(&mut self, directory_id: &str, in_service_at: SystemTime) {
        if let Some(record) = self.held.get_mut(directory_id) {
            record.in_service_at.get_or_insert(in_service_at);
        }
    }

    /// Whether the account holds `directory_id` AND the create that registered it has
    /// brought it into service by `now` — the question a service that builds on a directory
    /// of this account asks, because a create answers while the directory is still being
    /// built.
    ///
    /// A record carrying no in-service instant is a directory registered with no build to
    /// wait for, so it is in service for as long as it is held; a directory the inventory
    /// does not hold is in service at no instant at all.
    #[must_use]
    pub fn holds_in_service(&self, directory_id: &str, now: SystemTime) -> bool {
        self.held
            .get(directory_id)
            .is_some_and(|record| record.in_service_at.is_none_or(|due| now >= due))
    }

    /// The fully qualified domain name `directory_id` was created with, or `None` for an
    /// id the inventory holds nothing for and for one registered without a domain.
    #[must_use]
    pub fn domain_name(&self, directory_id: &str) -> Option<&str> {
        self.held
            .get(directory_id)
            .map(|record| record.domain_name.as_str())
            .filter(|domain_name| !domain_name.is_empty())
    }

    /// The NetBIOS short name `directory_id` was created with, or `None` for an id the
    /// inventory holds nothing for and for one registered without a short name.
    #[must_use]
    pub fn short_name(&self, directory_id: &str) -> Option<&str> {
        self.held
            .get(directory_id)
            .map(|record| record.short_name.as_str())
            .filter(|short_name| !short_name.is_empty())
    }

    /// Record the control plane's ENABLE of the directory's data API: it is in service from
    /// `effective_at` on, which is when the transition that enable owes falls due.
    ///
    /// A no-op for an id the inventory holds no record of: a directory the account does not
    /// hold has no data API to put in service, and recording one would claim the account
    /// holds it.
    pub fn enable_data_access(
        &mut self,
        directory_id: &str,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        self.change_data_access(directory_id, true, now, effective_at);
    }

    /// Record the control plane's DISABLE of it: the data API is out of service from
    /// `effective_at` on, which is when the transition that disable owes falls due, and
    /// serves nothing for the directory again until another enable.
    ///
    /// A no-op for an id the inventory holds no record of, as [`Self::enable_data_access`]
    /// is.
    pub fn disable_data_access(
        &mut self,
        directory_id: &str,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        self.change_data_access(directory_id, false, now, effective_at);
    }

    /// The one writer behind both toggles, so an enable and a disable record a change the
    /// same way and differ only in what they change the data API to.
    fn change_data_access(
        &mut self,
        directory_id: &str,
        in_service: bool,
        now: SystemTime,
        effective_at: SystemTime,
    ) {
        if let Some(record) = self.held.get_mut(directory_id) {
            record.data_access = record.data_access.changing_to(in_service, effective_at, now);
        }
    }

    /// Whether the directory's data API answers at `now`: the account holds the directory
    /// and the control plane's toggle has it in service at that instant.
    ///
    /// `false` while an enable's transition is still outstanding and `true` while a
    /// disable's is, so the data API is in service at exactly the instants the control
    /// plane reports its data access as settled on — nothing is served for a directory the
    /// account does not hold at all.
    #[must_use]
    pub fn data_access_is_in_service(&self, directory_id: &str, now: SystemTime) -> bool {
        self.held
            .get(directory_id)
            .is_some_and(|record| record.data_access.in_service_at(now))
    }

    /// Record that the data plane created `account_name` in `directory_id`.
    ///
    /// A no-op for a directory the inventory holds nothing for: it records the accounts of
    /// the directories the control plane created, and the data plane creates no directory
    /// record of its own.
    pub fn record_account(&mut self, directory_id: &str, account_name: &str) {
        let key = account_key(account_name);
        let Some(record) = self.held.get_mut(directory_id) else {
            return;
        };
        if key.is_empty() || record.accounts.contains_key(&key) {
            return;
        }
        record.accounts.insert(key.clone(), AccountRecord::default());
        record.account_order.push_back(key);
        while record.account_order.len() > MAX_RECORDED_ACCOUNTS_PER_DIRECTORY {
            if let Some(oldest) = record.account_order.pop_front() {
                record.accounts.remove(&oldest);
            }
        }
    }

    /// Record that `account_name` is no longer an account of `directory_id`.
    pub fn forget_account(&mut self, directory_id: &str, account_name: &str) {
        let key = account_key(account_name);
        let Some(record) = self.held.get_mut(directory_id) else {
            return;
        };
        record.accounts.remove(&key);
        record.account_order.retain(|held| held != &key);
    }

    /// Whether `directory_id` holds an account named `account_name`, whatever its case.
    #[must_use]
    pub fn holds_account(&self, directory_id: &str, account_name: &str) -> bool {
        self.held
            .get(directory_id)
            .is_some_and(|record| record.accounts.contains_key(&account_key(account_name)))
    }

    /// Record that the control plane set the password of `account_name` in `directory_id`,
    /// and answer whether there was such an account to set it on.
    pub fn record_password_set(&mut self, directory_id: &str, account_name: &str) -> bool {
        let key = account_key(account_name);
        let Some(account) = self
            .held
            .get_mut(directory_id)
            .and_then(|record| record.accounts.get_mut(&key))
        else {
            return false;
        };
        account.password_set = true;
        true
    }

    /// Whether the password of `account_name` in `directory_id` has been set.
    #[must_use]
    pub fn password_set(&self, directory_id: &str, account_name: &str) -> bool {
        self.held
            .get(directory_id)
            .and_then(|record| record.accounts.get(&account_key(account_name)))
            .is_some_and(|account| account.password_set)
    }

    /// Every account of `directory_id` whose password has been set, by folded name.
    ///
    /// For a reader that has to answer "does this one have a password" for MANY accounts at
    /// once — a listing, a search — so it takes the inventory's answer once rather than
    /// asking it per account, and does not hold this inventory while reading its own store.
    #[must_use]
    pub fn accounts_with_password(&self, directory_id: &str) -> BTreeSet<String> {
        self.held
            .get(directory_id)
            .into_iter()
            .flat_map(|record| record.accounts.iter())
            .filter(|(_, account)| account.password_set)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// How many directories the inventory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether the inventory holds no directory at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use super::{
        AdDirectoryInventory, MAX_RECORDED_ACCOUNTS_PER_DIRECTORY, MAX_RECORDED_DIRECTORIES,
    };

    /// A `DirectoryId` of the form the models declare (`^d-[0-9a-f]{10}$`), one per rank.
    fn directory(rank: usize) -> String {
        format!("d-{rank:010x}")
    }

    /// A domain name a directory of that rank was created with.
    fn domain(rank: usize) -> String {
        format!("corp{rank}.example.com")
    }

    /// The NetBIOS short name a directory of that rank was created with: a NetBIOS name is
    /// written in upper case and is not a label of the domain name.
    fn short_name(rank: usize) -> String {
        format!("CORP{rank}")
    }

    /// The span a control-plane toggle takes to fall due in these cases.
    const SETTLES_IN: Duration = Duration::from_millis(500);

    fn at() -> SystemTime {
        SystemTime::now()
    }

    /// RULE: the inventory answers for exactly the directories registered and not forgotten,
    /// whatever order the two happen in.
    #[test]
    fn the_inventory_holds_what_was_registered_and_not_forgotten() {
        let mut inventory = AdDirectoryInventory::default();
        inventory.register(&directory(1));
        inventory.register(&directory(2));
        // Registering twice is the same fact, not a second record.
        inventory.register(&directory(2));

        for (case, id, expected) in [
            ("a registered directory", directory(1), true),
            ("a directory registered twice", directory(2), true),
            ("a directory never registered", directory(3), false),
        ] {
            assert_eq!(
                inventory.holds(&id),
                expected,
                "case {case:?}: expected holds({id:?}) to be {expected}, got {}",
                inventory.holds(&id)
            );
        }
        assert_eq!(
            inventory.len(),
            2,
            "case two distinct directories: expected 2 records, got {}",
            inventory.len()
        );

        inventory.forget(&directory(1));
        // Forgetting an id the inventory never held changes nothing.
        inventory.forget(&directory(9));
        assert!(
            !inventory.holds(&directory(1)) && inventory.holds(&directory(2)),
            "case one directory forgotten: expected only the other to be held, got {} record(s)",
            inventory.len()
        );

        inventory.forget(&directory(2));
        assert!(
            inventory.is_empty(),
            "case every directory forgotten: expected an empty inventory, got {} record(s)",
            inventory.len()
        );
    }

    /// RULE: a directory is in service from the instant its create recorded and not one
    /// instant before, whatever build the create owed — and at no instant at all once the
    /// account no longer holds it.
    #[test]
    fn a_directory_is_in_service_from_the_instant_its_create_recorded() {
        let mut inventory = AdDirectoryInventory::default();
        let created_at = at();
        // The rows differ along the one thing the rule generalises over: how long the
        // create's build takes before the directory is in service.
        let builds: [(&str, usize, Duration); 3] = [
            ("a build of one span", 1, SETTLES_IN),
            ("a build of four spans", 2, SETTLES_IN * 4),
            ("a build of ten spans", 3, SETTLES_IN * 10),
        ];
        for (case, rank, build) in builds {
            let id = directory(rank);
            inventory.register_in_domain(&id, &domain(rank), Some(&short_name(rank)));
            inventory.record_in_service_at(&id, created_at + build);
            // A second recording is the same build, not a restarted one.
            inventory.record_in_service_at(&id, created_at + build + SETTLES_IN * 100);
            for (moment, asked_at, expected) in [
                ("the instant of the create", created_at, false),
                ("one span short of due", created_at + build - SETTLES_IN, false),
                ("the instant it falls due", created_at + build, true),
                ("long after it is due", created_at + build + SETTLES_IN * 100, true),
            ] {
                let in_service = inventory.holds_in_service(&id, asked_at);
                assert_eq!(
                    in_service, expected,
                    "case {case:?} at {moment:?}: expected holds_in_service({id:?}) to be \
                     {expected}, got {in_service}"
                );
            }
        }

        // A registration that recorded no build to wait for is in service for as long as it
        // is held, and an id the inventory holds nothing for is in service at no instant.
        inventory.register(&directory(4));
        for (case, id, expected) in [
            ("a registration with no build recorded", directory(4), true),
            ("an id never registered", directory(9), false),
        ] {
            let in_service = inventory.holds_in_service(&id, created_at);
            assert_eq!(
                in_service, expected,
                "case {case:?}: expected holds_in_service({id:?}) to be {expected}, got \
                 {in_service}"
            );
        }

        // A deleted directory is in service at no instant: the account no longer holds it.
        inventory.forget(&directory(1));
        let after_delete = inventory.holds_in_service(&directory(1), created_at + SETTLES_IN * 100);
        assert!(
            !after_delete,
            "case \"a deleted directory\": expected holds_in_service to be false, got \
             {after_delete}"
        );
    }

    /// RULE: the inventory reports the two names each directory was created with — its
    /// domain name and its NetBIOS short name — and a registration that named neither
    /// reports neither.
    #[test]
    fn the_inventory_reports_the_domain_each_directory_was_created_in() {
        let mut inventory = AdDirectoryInventory::default();
        inventory.register_in_domain(&directory(1), &domain(1), Some(&short_name(1)));
        inventory.register_in_domain(&directory(2), &domain(2), Some(&short_name(2)));
        // A re-registration leaves the names as they stand.
        inventory.register_in_domain(&directory(2), &domain(9), Some(&short_name(9)));
        // A registration that named no domain reports none.
        inventory.register(&directory(3));
        // Nor does a create that named a domain but no short name.
        inventory.register_in_domain(&directory(5), &domain(5), None);

        for (case, id, expected) in [
            (
                "a directory registered in a domain",
                directory(1),
                Some(domain(1)),
            ),
            ("another one", directory(2), Some(domain(2))),
            ("a registration that named no domain", directory(3), None),
            ("a directory never registered", directory(4), None),
            (
                "a create that named no short name",
                directory(5),
                Some(domain(5)),
            ),
        ] {
            assert_eq!(
                inventory.domain_name(&id).map(str::to_string),
                expected,
                "case {case:?}: expected the domain the create named, got {:?}",
                inventory.domain_name(&id)
            );
        }

        for (case, id, expected) in [
            (
                "a directory registered with a short name",
                directory(1),
                Some(short_name(1)),
            ),
            ("another one", directory(2), Some(short_name(2))),
            (
                "a registration that named no short name",
                directory(5),
                None,
            ),
            ("a registration that named neither", directory(3), None),
            ("a directory never registered", directory(4), None),
        ] {
            assert_eq!(
                inventory.short_name(&id).map(str::to_string),
                expected,
                "case {case:?}: expected the short name the create named, got {:?}",
                inventory.short_name(&id)
            );
        }

        // A forgotten directory takes its names with it.
        inventory.forget(&directory(1));
        assert!(
            inventory.domain_name(&directory(1)).is_none()
                && inventory.short_name(&directory(1)).is_none(),
            "case a deleted directory: expected no domain and no short name, got {:?} and {:?}",
            inventory.domain_name(&directory(1)),
            inventory.short_name(&directory(1))
        );
    }

    /// RULE: a directory the control plane never registered carries none of its facts, and
    /// no write about one brings it into the inventory.
    #[test]
    fn a_directory_the_control_plane_never_created_carries_no_fact() {
        let mut inventory = AdDirectoryInventory::default();
        inventory.enable_data_access(&directory(1), at(), at());
        inventory.record_account(&directory(1), "someone");
        assert!(
            inventory.is_empty()
                && !inventory.holds(&directory(1))
                && !inventory.data_access_is_in_service(&directory(1), at())
                && inventory.domain_name(&directory(1)).is_none()
                && inventory.short_name(&directory(1)).is_none()
                && !inventory.holds_account(&directory(1), "someone")
                && !inventory.record_password_set(&directory(1), "someone"),
            "case an unregistered directory: expected the inventory to hold nothing, got {} \
             record(s)",
            inventory.len()
        );
    }

    /// RULE: an account answers to every spelling of its name, and its password is recorded
    /// under any of them.
    #[test]
    fn an_accounts_password_is_recorded_under_any_spelling_of_its_name() {
        let mut inventory = AdDirectoryInventory::default();
        let id = directory(3);
        inventory.register_in_domain(&id, &domain(3), Some(&short_name(3)));
        inventory.record_account(&id, "Ada.Lovelace");

        // The rows are spellings of the one account name: a directory account name is case
        // insensitive, so every row names the account the create recorded.
        for spelling in ["Ada.Lovelace", "ada.lovelace", "ADA.LOVELACE"] {
            assert!(
                inventory.holds_account(&id, spelling),
                "case {spelling:?}: expected the recorded account to answer to this spelling"
            );
            assert!(
                !inventory.password_set(&id, spelling),
                "case {spelling:?}: expected no password before one is set"
            );
        }
        assert!(
            !inventory.holds_account(&id, "someone.else"),
            "case a name nothing recorded: expected no account"
        );

        assert!(
            inventory.record_password_set(&id, "ADA.LOVELACE"),
            "case a password set under another spelling: expected the account to be found"
        );
        assert!(
            inventory.password_set(&id, "ada.lovelace")
                && inventory.accounts_with_password(&id)
                    == BTreeSet::from(["ada.lovelace".to_string()]),
            "case a password set: expected it reported under every spelling, got {:?}",
            inventory.accounts_with_password(&id)
        );

        // A forgotten account takes its password with it, and a fresh account of the same
        // name starts without one.
        inventory.forget_account(&id, "ada.lovelace");
        inventory.record_account(&id, "ada.lovelace");
        assert!(
            inventory.holds_account(&id, "ada.lovelace")
                && !inventory.password_set(&id, "ada.lovelace"),
            "case an account created again: expected it to start without a password"
        );

        // And a forgotten DIRECTORY takes every account with it.
        inventory.forget(&id);
        assert!(
            !inventory.holds_account(&id, "ada.lovelace")
                && inventory.accounts_with_password(&id).is_empty(),
            "case a deleted directory: expected the inventory to hold no account of it"
        );
    }

    /// RULE: the inventory is bounded in both dimensions, and past either ceiling the
    /// OLDEST record is the one dropped, so what a workflow just created is still
    /// resolvable.
    #[test]
    fn the_inventory_is_bounded_and_drops_its_oldest_registration() {
        let mut inventory = AdDirectoryInventory::default();
        for rank in 0..MAX_RECORDED_DIRECTORIES {
            inventory.register_in_domain(&directory(rank), &domain(rank), Some(&short_name(rank)));
        }
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case an inventory filled to the ceiling: expected {MAX_RECORDED_DIRECTORIES} \
             records, got {}",
            inventory.len()
        );

        inventory.register_in_domain(
            &directory(MAX_RECORDED_DIRECTORIES),
            &domain(MAX_RECORDED_DIRECTORIES),
            Some(&short_name(MAX_RECORDED_DIRECTORIES)),
        );
        assert_eq!(
            inventory.len(),
            MAX_RECORDED_DIRECTORIES,
            "case one past the ceiling: expected the ceiling to hold, got {}",
            inventory.len()
        );
        assert!(
            !inventory.holds(&directory(0)),
            "case one past the ceiling: expected the oldest registration to be dropped"
        );
        assert!(
            inventory.holds(&directory(MAX_RECORDED_DIRECTORIES))
                && inventory.holds(&directory(1)),
            "case one past the ceiling: expected the newest and the next-oldest to be held"
        );
        assert_eq!(
            inventory.domain_name(&directory(MAX_RECORDED_DIRECTORIES)),
            Some(domain(MAX_RECORDED_DIRECTORIES).as_str()),
            "case one past the ceiling: expected the newest registration's own domain, got {:?}",
            inventory.domain_name(&directory(MAX_RECORDED_DIRECTORIES))
        );

        let id = directory(MAX_RECORDED_DIRECTORIES);
        for index in 0..=MAX_RECORDED_ACCOUNTS_PER_DIRECTORY {
            inventory.record_account(&id, &format!("account-{index}"));
        }
        assert!(
            !inventory.holds_account(&id, "account-0")
                && inventory.holds_account(
                    &id,
                    &format!("account-{MAX_RECORDED_ACCOUNTS_PER_DIRECTORY}")
                ),
            "case one account past the per-directory ceiling: expected the earliest account \
             dropped and the newest kept"
        );
    }

    /// RULE: a directory's data API is in service exactly while the control plane's toggle
    /// has it so — from the instant an enable's transition falls due until a disable's does
    /// — and a registration alone puts nothing in service.
    #[test]
    fn data_access_is_in_service_exactly_while_the_control_planes_toggle_has_it_so() {
        let span = SETTLES_IN;
        let enabled_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let enabled_from = enabled_at + span;
        let hour = Duration::from_secs(3_600);

        let mut inventory = AdDirectoryInventory::default();
        inventory.register(&directory(1));
        inventory.register(&directory(2));

        // (case, the instant asked about, whether the data API answers)
        let before_any_enable: [(&str, SystemTime, bool); 3] = [
            ("the instant of the create", enabled_at, false),
            ("one span later", enabled_from, false),
            ("an hour later", enabled_from + hour, false),
        ];
        for (case, now, expected) in before_any_enable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on a registered directory nothing enabled: expected \
                 {expected}, got {in_service}"
            );
        }

        inventory.enable_data_access(&directory(1), enabled_at, enabled_from);
        // (case, the instant asked about, whether the data API answers)
        let after_enable: [(&str, SystemTime, bool); 4] = [
            ("the instant of the enable", enabled_at, false),
            (
                "a moment before the enable's transition falls due",
                enabled_from - Duration::from_millis(1),
                false,
            ),
            ("the instant it falls due", enabled_from, true),
            ("well past it", enabled_from + hour, true),
        ];
        for (case, now, expected) in after_enable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on an enabled directory: expected {expected}, got {in_service}"
            );
        }

        // The toggle is per directory: the other registration is untouched.
        assert!(
            !inventory.data_access_is_in_service(&directory(2), enabled_from),
            "case another directory of the same account: expected its data API to stay out \
             of service"
        );

        // Re-registering a directory whose data access is on — which a control plane does
        // for a directory it already recorded — must not withdraw it.
        inventory.register(&directory(1));
        assert!(
            inventory.data_access_is_in_service(&directory(1), enabled_from),
            "case a re-registered enabled directory: expected its data API to stay in service"
        );

        // A disable takes effect when ITS transition falls due, and not before: the data
        // API is still in service the instant the disable is recorded.
        let disabled_at = enabled_from + hour;
        let disabled_from = disabled_at + span;
        inventory.disable_data_access(&directory(1), disabled_at, disabled_from);
        // (case, the instant asked about, whether the data API answers)
        let after_disable: [(&str, SystemTime, bool); 4] = [
            ("the instant of the disable", disabled_at, true),
            (
                "a moment before the disable's transition falls due",
                disabled_from - Duration::from_millis(1),
                true,
            ),
            ("the instant it falls due", disabled_from, false),
            ("well past it", disabled_from + hour, false),
        ];
        for (case, now, expected) in after_disable {
            let in_service = inventory.data_access_is_in_service(&directory(1), now);
            assert_eq!(
                in_service, expected,
                "case {case:?} on a disabled directory: expected {expected}, got {in_service}"
            );
        }

        // A disable recorded while an ENABLE is still outstanding does not apply that
        // enable early: the data API stays out of service throughout.
        inventory.register(&directory(3));
        inventory.enable_data_access(&directory(3), enabled_at, enabled_from);
        let cancelled_from = enabled_at + Duration::from_millis(100) + span;
        inventory.disable_data_access(
            &directory(3),
            enabled_at + Duration::from_millis(100),
            cancelled_from,
        );
        for (case, now) in [
            ("the instant the first enable would have fallen due", enabled_from),
            ("the instant the disable falls due", cancelled_from),
            ("an hour later", cancelled_from + hour),
        ] {
            assert!(
                !inventory.data_access_is_in_service(&directory(3), now),
                "case {case:?} on a directory disabled while its enable was outstanding: \
                 expected the data API to stay out of service"
            );
        }

        // A re-enable puts it back in service on its own transition, and forgetting the
        // directory closes every question about it.
        let re_enabled_at = disabled_from + hour;
        let re_enabled_from = re_enabled_at + span;
        inventory.enable_data_access(&directory(1), re_enabled_at, re_enabled_from);
        assert!(
            inventory.data_access_is_in_service(&directory(1), re_enabled_from)
                && !inventory.data_access_is_in_service(&directory(1), re_enabled_at),
            "case a re-enabled directory: expected the second transition to decide, not the \
             first"
        );
        inventory.forget(&directory(1));
        assert!(
            !inventory.data_access_is_in_service(&directory(1), re_enabled_from),
            "case a forgotten directory: expected no data API at all"
        );

        // A directory the inventory never held gains no record from either toggle.
        inventory.enable_data_access(&directory(7), re_enabled_at, re_enabled_from);
        inventory.disable_data_access(&directory(8), re_enabled_at, re_enabled_from);
        assert!(
            !inventory.holds(&directory(7)) && !inventory.holds(&directory(8)),
            "case the toggle on an unheld directory: expected no registration, got {} record(s)",
            inventory.len()
        );
    }
}

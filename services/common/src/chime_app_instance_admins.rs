// SPDX-License-Identifier: CC0-1.0

//! The APP-INSTANCE ADMIN directory: which identity administers an Amazon Chime SDK app
//! instance.
//!
//! An `AppInstanceAdmin` is a resource of ONE service — Chime SDK Identity promotes an
//! identity to it — while the role it grants is read by ANOTHER: Chime SDK Messaging
//! reserves actions for an administrator of the app instance a channel belongs to (listing
//! an account's private channels, deleting a channel message). The messaging service
//! cannot measure that role, because the promotion was given to the identity service, so
//! the identity service RECORDS it here and the messaging service reads it.
//!
//! One directory under ONE shared type, because the engine keys state by type: a copy
//! inside either service's own state struct would be invisible to the other, and the two
//! would disagree about who administers an app instance.
//!
//! Nothing here names a wire shape: the directory holds the app instance ARN, the
//! administering identity's ARN and the instant the promotion was recorded, and each
//! reader projects those onto the shapes ITS model declares.
//!
//! The directory is a record of what this emulator PROMOTED, so it is partial by
//! construction: an identity it holds no record of simply is not an administrator here.
//! It is also BOUNDED — see [`MAX_REGISTERED_ADMINS`] — so a caller looping on promote
//! cannot grow it without end; the oldest registration is dropped first, which keeps the
//! promotions a live workflow just made readable.
//!
//! A promotion belongs to the identity it names and to the app instance it scopes, so the
//! service that owns those resources keeps the directory in step with them on every write:
//! a demotion, a deleted identity ([`AppInstanceAdminDirectory::demote_identity`]) and a
//! deleted app instance ([`AppInstanceAdminDirectory::forget_admins_of`]) each leave no
//! promotion behind.

use std::collections::{BTreeMap, VecDeque};

/// How many promotions the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so without a bound a caller promoting identities in a loop would grow
/// it without end. It is set far above any ordinary workflow, and past it the OLDEST
/// registration is dropped — never a newer one — so an identity a running workflow just
/// promoted is always still an administrator.
pub const MAX_REGISTERED_ADMINS: usize = 10_000;

/// One recorded promotion: an identity that administers an app instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInstanceAdminRecord {
    /// The `AppInstanceArn` the identity administers.
    pub app_instance_arn: String,
    /// The administering identity's ARN (an `AppInstanceUser` or `AppInstanceBot`).
    pub admin_arn: String,
    /// When the promotion was recorded, as epoch milliseconds — the resolution the
    /// restJson1 timestamps of both services carry.
    pub created_millis: i64,
}

/// The directory itself: every recorded promotion, keyed by app instance and identity.
///
/// `BTreeMap` (not `HashMap`) so a reader that lists an app instance's administrators
/// walks them in a stable order, which is what makes a paginated answer resumable.
#[derive(Debug, Default)]
pub struct AppInstanceAdminDirectory {
    admins: BTreeMap<(String, String), AppInstanceAdminRecord>,
    /// The order the entries were written in, so the oldest can be dropped first.
    order: VecDeque<(String, String)>,
}

impl AppInstanceAdminDirectory {
    /// Record that `admin_arn` administers `app_instance_arn`, and answer the record the
    /// directory now holds for it.
    ///
    /// Promotion is a SET: recording one the directory already holds keeps the FIRST
    /// record, so the instant it reports does not move every time a caller repeats the
    /// request. Past [`MAX_REGISTERED_ADMINS`] the oldest registration is dropped.
    pub fn promote(
        &mut self,
        app_instance_arn: &str,
        admin_arn: &str,
        created_millis: i64,
    ) -> AppInstanceAdminRecord {
        let key = (app_instance_arn.to_string(), admin_arn.to_string());
        if let Some(existing) = self.admins.get(&key) {
            return existing.clone();
        }
        let record = AppInstanceAdminRecord {
            app_instance_arn: app_instance_arn.to_string(),
            admin_arn: admin_arn.to_string(),
            created_millis,
        };
        self.admins.insert(key.clone(), record.clone());
        self.order.push_back(key);
        while self.order.len() > MAX_REGISTERED_ADMINS {
            if let Some(oldest) = self.order.pop_front() {
                self.admins.remove(&oldest);
            }
        }
        record
    }

    /// The recorded promotion of `admin_arn` over `app_instance_arn`, if the directory
    /// holds one.
    pub fn record(
        &self,
        app_instance_arn: &str,
        admin_arn: &str,
    ) -> Option<&AppInstanceAdminRecord> {
        self.admins
            .get(&(app_instance_arn.to_string(), admin_arn.to_string()))
    }

    /// Whether `admin_arn` administers `app_instance_arn`.
    pub fn administers(&self, app_instance_arn: &str, admin_arn: &str) -> bool {
        self.record(app_instance_arn, admin_arn).is_some()
    }

    /// Every recorded administrator of `app_instance_arn`, in a stable order.
    pub fn admins_of(&self, app_instance_arn: &str) -> Vec<&AppInstanceAdminRecord> {
        self.admins
            .values()
            .filter(|record| record.app_instance_arn == app_instance_arn)
            .collect()
    }

    /// Drop `admin_arn`'s promotion over `app_instance_arn`, answering whether the
    /// directory held one.
    pub fn demote(&mut self, app_instance_arn: &str, admin_arn: &str) -> bool {
        let key = (app_instance_arn.to_string(), admin_arn.to_string());
        self.order.retain(|held| *held != key);
        self.admins.remove(&key).is_some()
    }

    /// Drop every promotion the directory holds FOR the identity `admin_arn`, and answer
    /// how many were dropped — what the service that owns the identity calls when the
    /// identity itself is deleted.
    ///
    /// A promotion is a record OF one identity, so it exists only for as long as that
    /// identity does: once the identity is gone, the role it was given is gone with it,
    /// here as well as in the store that owns the identity. The directory is keyed by app
    /// instance AND identity, so every record naming this identity goes, whatever app
    /// instance scoped it. Without that the reader of the role would keep reporting a
    /// deleted identity as an administrator.
    pub fn demote_identity(&mut self, admin_arn: &str) -> usize {
        let dropped: Vec<(String, String)> = self
            .admins
            .keys()
            .filter(|(_, admin)| admin == admin_arn)
            .cloned()
            .collect();
        for (instance, admin) in &dropped {
            self.demote(instance, admin);
        }
        dropped.len()
    }

    /// Drop every promotion recorded over `app_instance_arn`, and answer how many were
    /// dropped — what the service that owns the app instance calls when the instance
    /// itself is deleted.
    ///
    /// An administrator is an administrator OF one app instance, so a promotion exists only
    /// for as long as the app instance that scopes it does: once the instance is gone, the
    /// role it granted is gone with it, here as well as in the store that owns the instance.
    /// Without that cascade the reader of the role would keep honouring a grant over an app
    /// instance nobody holds any more.
    pub fn forget_admins_of(&mut self, app_instance_arn: &str) -> usize {
        let dropped: Vec<(String, String)> = self
            .admins
            .keys()
            .filter(|(instance, _)| instance == app_instance_arn)
            .cloned()
            .collect();
        for (instance, admin) in &dropped {
            self.demote(instance, admin);
        }
        dropped.len()
    }

    /// How many promotions the directory currently holds, over every app instance.
    pub fn registered_admins(&self) -> usize {
        self.admins.len()
    }
}

#[cfg(test)]
mod tests {
    //! The directory over what a promotion's LIFETIME generalises over: a promotion
    //! recorded and read back, one demoted on its own, the cascade a deleted app instance
    //! owes — which reaches every promotion that instance scoped and no other's — and the
    //! drop a deleted IDENTITY owes, which reaches every promotion held for it.

    use super::*;

    const INSTANCE: &str = "arn:aws:chime:us-east-1:000000000000:app-instance/ai1";
    const OTHER_INSTANCE: &str = "arn:aws:chime:us-east-1:000000000000:app-instance/ai2";

    fn identity(instance: &str, segment: &str, id: &str) -> String {
        format!("{instance}/{segment}/{id}")
    }

    #[test]
    fn deleting_an_app_instance_drops_every_promotion_it_scoped_and_no_other() {
        // Rows: the two identity kinds an app instance has, in the numbers one instance can
        // hold them in, plus a second app instance whose promotions the cascade must leave
        // standing.
        let cases = [
            ("one user of the instance", INSTANCE, vec![("user", "u1")]),
            (
                "several identities of both kinds",
                INSTANCE,
                vec![("user", "u2"), ("user", "u3"), ("bot", "b1")],
            ),
            (
                "another app instance's own administrator",
                OTHER_INSTANCE,
                vec![("user", "u4")],
            ),
        ];
        let mut directory = AppInstanceAdminDirectory::default();
        for (case, instance, identities) in &cases {
            for (segment, id) in identities {
                let arn = identity(instance, segment, id);
                directory.promote(instance, &arn, 1_700_000_000_000);
                assert!(
                    directory.administers(instance, &arn),
                    "case {case:?}: expected {arn} to administer {instance} once promoted"
                );
            }
        }

        let dropped = directory.forget_admins_of(INSTANCE);
        assert_eq!(
            dropped, 4,
            "case {:?}: expected every promotion of that app instance dropped, got {dropped}",
            "the cascade of a deleted app instance"
        );
        assert!(
            directory.admins_of(INSTANCE).is_empty(),
            "case {:?}: expected no promotion left over the deleted app instance, got {:?}",
            "the cascade of a deleted app instance",
            directory.admins_of(INSTANCE)
        );
        let survivor = identity(OTHER_INSTANCE, "user", "u4");
        assert!(
            directory.administers(OTHER_INSTANCE, &survivor),
            "case {:?}: expected another app instance's promotion kept, held {}",
            "the cascade of a deleted app instance",
            directory.registered_admins()
        );
        assert_eq!(
            directory.registered_admins(),
            1,
            "case {:?}: expected exactly the surviving promotion held, got {}",
            "the cascade of a deleted app instance",
            directory.registered_admins()
        );

        // The cascade is a removal from a SET: an app instance whose promotions are already
        // gone drops none, and one that never had any drops none either.
        for (case, instance) in [
            ("the same app instance twice", INSTANCE),
            (
                "an app instance the directory never held a promotion over",
                "arn:aws:chime:us-east-1:000000000000:app-instance/never",
            ),
        ] {
            let again = directory.forget_admins_of(instance);
            assert_eq!(
                again, 0,
                "case {case:?}: expected nothing left to drop, got {again}"
            );
        }
    }

    /// One promotion of each identity kind under [`INSTANCE`], plus one under
    /// [`OTHER_INSTANCE`] that no row of the identity table names.
    fn populated() -> AppInstanceAdminDirectory {
        let mut directory = AppInstanceAdminDirectory::default();
        for (instance, segment, id) in [
            (INSTANCE, "user", "u1"),
            (INSTANCE, "bot", "b1"),
            (OTHER_INSTANCE, "user", "u4"),
        ] {
            directory.promote(instance, &identity(instance, segment, id), 1_700_000_000_000);
        }
        directory
    }

    #[test]
    fn deleting_an_identity_drops_every_promotion_held_for_it_and_no_other() {
        // Rows: the two identity kinds that can be promoted at all.
        for (case, segment, id) in [
            ("a promoted AppInstanceUser", "user", "u1"),
            ("a promoted AppInstanceBot", "bot", "b1"),
        ] {
            let mut directory = populated();
            let admin = identity(INSTANCE, segment, id);
            let dropped = directory.demote_identity(&admin);
            assert_eq!(
                dropped, 1,
                "case {case:?}: expected the one promotion held for it dropped, got {dropped}"
            );
            assert!(
                !directory.administers(INSTANCE, &admin),
                "case {case:?}: expected {admin} to administer nothing, it still does"
            );
            let remaining = directory.registered_admins();
            assert_eq!(
                remaining, 2,
                "case {case:?}: expected the other two promotions kept, got {remaining}"
            );
            let elsewhere = identity(OTHER_INSTANCE, "user", "u4");
            assert!(
                directory.administers(OTHER_INSTANCE, &elsewhere),
                "case {case:?}: expected another identity's promotion kept, it was dropped"
            );
            // The drop is a removal from a SET: an identity the directory holds no
            // promotion for drops none.
            let again = directory.demote_identity(&admin);
            assert_eq!(
                again, 0,
                "case {case:?}: expected nothing left to drop the second time, got {again}"
            );
        }

        let mut directory = populated();
        let never_promoted = identity(INSTANCE, "user", "never");
        let none = directory.demote_identity(&never_promoted);
        assert_eq!(
            none, 0,
            "case {:?}: expected nothing to drop, got {none}",
            "an identity the directory never held a promotion for"
        );
    }

    #[test]
    fn a_dropped_promotion_frees_its_place_in_the_eviction_order() {
        let mut directory = AppInstanceAdminDirectory::default();
        for index in 0..MAX_REGISTERED_ADMINS {
            directory.promote(
                INSTANCE,
                &identity(INSTANCE, "user", &format!("u{index:06}")),
                1_700_000_000_000,
            );
        }
        let oldest = identity(INSTANCE, "user", "u000000");
        let dropped = directory.demote_identity(&oldest);
        assert_eq!(
            dropped, 1,
            "case {:?}: expected the oldest promotion dropped, got {dropped}",
            "a full directory"
        );

        // The freed place is what the next promotion takes, so no OTHER record is evicted
        // to make room for it.
        let fresh = identity(INSTANCE, "user", "fresh");
        directory.promote(INSTANCE, &fresh, 1_700_000_000_001);
        let held = directory.registered_admins();
        assert_eq!(
            held, MAX_REGISTERED_ADMINS,
            "case {:?}: expected the directory back at its bound, got {held}",
            "a full directory"
        );
        for (case, admin) in [
            ("the promotion that took the freed place", &fresh),
            (
                "the promotion that was second-oldest",
                &identity(INSTANCE, "user", "u000001"),
            ),
        ] {
            assert!(
                directory.administers(INSTANCE, admin),
                "case {case:?}: expected {admin} still an administrator, it is not"
            );
        }
    }
}

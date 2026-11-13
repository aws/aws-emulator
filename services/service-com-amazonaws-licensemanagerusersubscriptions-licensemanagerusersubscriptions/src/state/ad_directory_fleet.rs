// SPDX-License-Identifier: CC0-1.0

//! Resolving the AWS Directory Service directory an identity-provider registration names
//! against the directories the account actually has.
//!
//! `ActiveDirectoryIdentityProvider.DirectoryId` is "the directory id of the Active
//! Directory" the registration is for, bounded by nothing but its `@pattern`
//! `^(d|sd)-[0-9a-f]{10}$` — no closed value set — so the only way a call here can
//! establish that the account HAS the directory a request names is to read the record of
//! the plane that creates directories. That plane is AWS Directory Service
//! (`ds:CreateDirectory`, `ds:CreateMicrosoftAD`, `ds:ConnectDirectory` create one and
//! `ds:DeleteDirectory` deletes it), and the record it publishes for every other service of
//! this emulator to read is the shared AD directory inventory
//! ([`::service_common::ad_directories::AdDirectoryInventory`]) in the engine both services
//! are mounted on. So a directory a client created a moment ago is one a registration can
//! be built on, and one it deleted stops being one at once: nothing is cached here, and
//! this service keeps no copy of another plane's resources.
//!
//! TWO sources, neither authoritative over the other, exactly as the instance a user
//! association names is resolved ([`crate::state::ec2_fleet::authorizes_instance`]):
//!
//! * this service's OWN fleet seam
//!   ([`crate::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::resolves_active_directory`]),
//!   consulted first because it costs no second lock and is what answers in a process that
//!   mounts this service with no Directory Service beside it;
//! * the shared inventory the plane that CREATES directories writes.
//!
//! A directory either of them reports is one the account has, so the resolution fails
//! closed only when NEITHER holds the id.
//!
//! IN SERVICE, not merely created: a create answers while the directory is still being
//! built, and an identity provider cannot be registered for a directory that has not
//! reached its active stage — which is why the inventory is asked
//! ([`::service_common::ad_directories::AdDirectoryInventory::holds_in_service`]) at the
//! instant of the request rather than only whether it holds the id, and why the fleet seam
//! holds the stage each of its directories is in.
//!
//! LOCK ORDER: this service's own store first and the shared inventory second — the order
//! every service that reads the inventory keeps — and neither is held while the other is
//! taken.

use std::time::SystemTime;

/// The SHARED AD directory inventory in the engine this service is mounted on: call it with
/// a handler's `&*self.engine`.
///
/// `None` for an engine that carries none, which is an engine on which no directory can
/// have been created either.
pub fn ad_directory_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ad_directories::AdDirectoryInventory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::ad_directories::AdDirectoryInventory,
            >,
        )
}

/// Whether `directory_id` names a directory of this account an identity provider can be
/// registered for at `now`: one the account HAS, in service rather than still being built
/// or already deleted.
///
/// The ONE predicate every caller asks, so no two of them can resolve the same directory
/// differently. Constant work: one lookup per source, by id.
pub fn resolves_active_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    now: SystemTime,
) -> bool {
    // Taken and released before the inventory is locked: no store of this service is held
    // while another service's record is read.
    let held_locally = crate::state::service_state(engine)
        .is_some_and(|state| state.lock().resolves_active_directory(directory_id));
    if held_locally {
        return true;
    }
    ad_directory_inventory(engine)
        .is_some_and(|inventory| inventory.lock().holds_in_service(directory_id, now))
}

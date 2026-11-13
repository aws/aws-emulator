// SPDX-License-Identifier: CC0-1.0

//! What one instance's console shows.
//!
//! EC2 serves that console two ways — "gets the console output for the specified instance"
//! (`GetConsoleOutput`) as text, and "retrieve a JPG-format screenshot of a running instance"
//! (`GetConsoleScreenshot`) as an image — and both show the SAME console. So the lines live
//! here, in one place, and the two operations differ only in how they render them: one
//! encodes the text, the other draws it. Neither can then report one instance's console
//! differently.
//!
//! Every line is one the instance's own GUEST wrote while it came up: the kernel's and the
//! init system's lines, and the output of the instance's user data, which the boot ran
//! ([`super::guest_console`]). An instance whose guest has not come up yet has nothing on
//! its console at all.

use std::time::SystemTime;

use super::InstanceRecord;

/// Resolve the instance whose CONSOLE a request asks for, or refuse the id: HTTP 400
/// `InvalidInstanceID.Malformed`, in one of the two wordings that code carries.
///
/// EC2's per-operation validators do NOT agree about an instance id they cannot resolve, and
/// the console readers take the strict one: a WELL-FORMED id that names no instance of the
/// account is answered `InvalidInstanceID.Malformed` — the instance ID is malformed — rather
/// than reported as not found. The evidence is the recorded contract of BOTH console
/// readers, which probe an id the account does not hold inside the long form
/// [`super::instance_id_is_well_formed`] accepts and are answered the malformed half; so the
/// reading lives beside the console they share rather than once per operation, and the
/// shared [`super::require_known_instance`] stays as it is for the operations that DO draw
/// the distinction.
///
/// A value that is not an instance id AT ALL is refused one step earlier, by the id space
/// itself, with the echoed-value message
/// [`crate::scaffolded::Scaffolded::invalid_instance_id_malformed`] carries — the same order
/// and the same two wordings `CreateSnapshots` uses, so one code is worded one way per half
/// across the crate rather than per operation.
///
/// The resolved record is handed back, so a handler reads the instance ONCE rather than
/// looking it up again behind an unreachable branch.
pub fn require_console_instance<'store, E>(
    store: &'store super::ComAmazonawsEc2Amazonec2State,
    instance_id: &str,
) -> Result<&'store InstanceRecord, crate::scaffolded::Scaffolded<E>> {
    // A value that is not an instance id at all never reaches the store: it is refused by
    // the id space itself, with the echoed-value message every EC2 id space answers for one.
    if !super::instance_id_is_well_formed(instance_id) {
        return Err(crate::scaffolded::Scaffolded::invalid_instance_id_malformed(
            instance_id,
        ));
    }
    store
        .instances
        .get(instance_id)
        .ok_or_else(|| crate::scaffolded::Scaffolded::instance_id_is_malformed(instance_id))
}

/// The lines of text one instance's console shows: what that instance's GUEST wrote to its
/// console, one block per BOOT it has had.
///
/// "For Linux instances, the instance console output displays the exact console output that
/// would normally be displayed on a physical monitor attached to a computer"
/// (`GetConsoleOutput`), so every line here is a line the guest wrote while it came up — the
/// kernel's and the init system's own lines, and the output of the instance's user data,
/// which the boot ran ([`super::guest_console`]). Nothing about the API's own view of the
/// instance is written here: a console is a device of the guest, not a report of the record.
///
/// The blocks are in the order the guest had its boots ([`super::instance_boot_lines`]): a
/// console is APPENDED to, so an instance that has restarted shows what the earlier boots
/// wrote with the new boot's lines after them, and an instance whose guest has not come up
/// yet shows NOTHING — which is what a console of an instance that is still starting has on
/// it.
pub fn instance_console_lines(instance: &InstanceRecord) -> Vec<String> {
    super::instance_boots(instance)
        .iter()
        .flat_map(|boot| super::instance_boot_lines(boot).iter().cloned())
        .collect()
}

/// One instance's console as TEXT: [`instance_console_lines`], one per line, each ended by a
/// newline the way a console device ends a line it has written.
///
/// What `GetConsoleOutput` serves (base64-encoded, as that operation documents). Derived from
/// the stored instance on every call rather than kept beside it, so two reads of an instance
/// nothing has moved answer exactly the same bytes and reading a console writes nothing.
pub fn instance_console_text(instance: &InstanceRecord) -> String {
    instance_console_lines(instance)
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// When one instance's console was last WRITTEN: the latest instant the console
/// [`instance_console_lines`] renders says it was written at — the instance's launch, and the
/// instant each boot its guest has had came up ([`super::instance_boots`], which is what
/// appends a block to the buffer).
///
/// What `GetConsoleOutput` reports as "the time at which the output was last updated"
/// (`GetConsoleOutputResult$Timestamp`). Derived from the SAME stored console the text is,
/// so the instant a client reads is the latest one the output it was handed shows, and an
/// instant nothing wrote the console at is never reported as an update of it: a reboot that
/// is only QUEUED — `RebootInstances` "only queues a request to reboot the specified
/// instances", so the guest comes up a moment later — has written nothing yet, and the
/// buffer still reads exactly as it did.
///
/// Derived on every call rather than kept beside the console, so two reads of an instance
/// nothing has moved report the same instant and reading one writes nothing.
pub fn instance_console_updated_at(instance: &InstanceRecord) -> SystemTime {
    super::instance_boots(instance)
        .iter()
        .map(|boot| boot.booted_at)
        .chain(::std::iter::once(instance.launch_time))
        .max()
        .unwrap_or(instance.launch_time)
}

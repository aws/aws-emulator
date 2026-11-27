// SPDX-License-Identifier: CC0-1.0

//! Running a program in the GUEST of one running instance, for the agent that is already in
//! it.
//!
//! # Why this lives here
//!
//! An instance's guest is EC2's: its storage, its host name and the identity the kernel
//! published to it are facts of the instance this store holds, and the one place a program of
//! that guest runs is the bounded, in-process shell session the boot's own user-data program
//! runs in ([`super::guest_console`], which states why that session and not a host process).
//! So a service that has an agent inside the guest — Systems Manager's Run Command, which
//! delivers a document's commands to the agent — asks THIS store to perform them, rather than
//! keeping a second notion of what the machine is or what it holds.
//!
//! # What a run is, and what it is not
//!
//! The commands are ONE SCRIPT, performed by one session, in a tree holding exactly this
//! machine's storage and its devices: an agent in the guest writes the lines it was delivered
//! into a script file and runs that file with the guest's shell, so a construct that spans
//! lines is the script's own — a here-document opened on one line carries the lines under it,
//! a compound command is read to its end — and a variable one line sets and a file one line
//! writes are there for the lines after it, and still there on the next run and on the next
//! boot, because the tree IS the instance's storage. What the run ANSWERS is that script's
//! own exit status and its two streams. Every command the session provides no program for —
//! anything needing a process, a socket or a host path — runs nothing and answers the status
//! a shell answers for a utility it could not find
//! ([`::service_common::posix_shell_session::Statuses::could_not_run`]), with the diagnostic a
//! shell writes for it on standard error, exactly as it does on a machine whose shell has no
//! such program. Nothing here starts a process, opens a host file or reaches a socket, and
//! nothing is written for a command that did not run.
//!
//! No thread of this process waits for a program either, so how long a run TOOK is what its
//! commands state they took ([`GuestRun::took`]): a caller whose own work is not finished
//! until the program is — an agent running a deployment's hook on the host — spends that
//! time, and one that reports no duration ignores it.
//!
//! # A guest has to be up
//!
//! A program runs in a guest that is RUNNING and has BOOTED: an instance that is still
//! `pending`, one that is stopped or gone, and one whose first boot the clock has not
//! materialised yet have no operating system to run anything
//! ([`ComAmazonawsEc2Amazonec2State::boot_due_instance_guests`] is what brings one up), so
//! each of those answers `None` — "there was no guest to run this in" — rather than an
//! outcome nothing produced.
//!
//! # An agent's own files
//!
//! Not everything such an agent does is a program: it also PUTS files on the machine — the
//! artifact it fetched for the work it was sent, and the files that work installs — and reads
//! back what an earlier run left there. Those writes go through the same tree and the same
//! bounds as a program's ([`ComAmazonawsEc2Amazonec2State::put_in_instance_guest`],
//! [`ComAmazonawsEc2Amazonec2State::read_in_instance_guest`]), so the agent and the guest's
//! own programs read and write one machine's storage rather than two notions of it.

use ::service_common::posix_shell_session::{
    layered, Bounds, EnvironmentPrograms, OutputLimits, SessionConfig, ShellSession,
};

use super::guest_console::{guest_environment, guest_volume, GuestIdentity};
use super::{ComAmazonawsEc2Amazonec2State, InstanceStateName};

/// How many commands ONE run performs before the session is given up on.
///
/// The bound on the work one run costs, and it matters where this runs: a run is performed
/// with this store's guard held, so what a caller-supplied program can make one cost is what
/// every other caller of the store waits for. The delivered lines are ONE script, so this is
/// the budget the whole script spends — every line of it, every command substitution one of
/// them opens and every iteration of a loop inside it — rather than a budget per line, which
/// is why it is the budget ONE PROGRAM is given everywhere else in this workspace
/// ([`Bounds::default`]) rather than a number of its own. Every one of those commands is
/// performed in process over this machine's in-memory tree, and what one of them may expand,
/// write and keep is bounded beside this ([`Bounds`], [`MAX_GUEST_RUN_OUTPUT_LINES`]).
pub const MAX_GUEST_RUN_COMMANDS: usize = 1_000;

/// How long ONE script delivered to a guest may be, and how many nested groups it may carry.
///
/// The script is read by the grammar in one piece ([`::service_common::posix_shell_session`],
/// whose own bounds are written for a single line), so both are stated for a whole script
/// here: a document's lines, the here-documents they carry and the words inside them come to
/// far more of each than one line does, and the shared nesting bound is a COUNT of the groups
/// a program carries rather than the depth it nests to. Both are the emulator's numbers — the
/// script is caller-supplied text — and what reaches them is bounded upstream by the lines
/// Systems Manager resolves for a step.
///
/// The nesting bound is what the grammar's own recursion is kept in range by, so the run is
/// performed on a stack sized for it ([`MAX_GUEST_SCRIPT_STACK_BYTES`]) and the walk over what
/// was parsed is bounded by the same number
/// ([`::service_common::posix_shell_session::Bounds::nesting`], which the session spends per
/// nested compound command).
pub const MAX_GUEST_SCRIPT_BYTES: usize = 256 * 1024;

/// See [`MAX_GUEST_SCRIPT_BYTES`].
pub const MAX_GUEST_SCRIPT_NESTING: usize = 512;

/// The stack the thread ONE script is performed on is given: far above what a program nesting
/// [`MAX_GUEST_SCRIPT_NESTING`] groups costs the grammar's recursive descent and this module's
/// own walk over what it parsed (a few kilobytes per level between them), so a script inside
/// that bound is answered by the bound rather than by the stack it ran on — a stack a thread
/// runs out of is not a refusal, it is the process gone.
///
/// It is the same size the other caller-supplied work of this workspace is given a thread for
/// (the Systems Manager automation script runner's `SCRIPT_STACK_BYTES`).
pub const MAX_GUEST_SCRIPT_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many lines of output ONE run keeps, and how many bytes one of those lines carries.
///
/// The output is caller-sized — the program is the caller's text — so what one run can
/// retain is bounded here. A run that wrote more keeps the first lines of it, which is what
/// a caller reading the head of a command's output sees.
pub const MAX_GUEST_RUN_OUTPUT_LINES: usize = 256;

/// See [`MAX_GUEST_RUN_OUTPUT_LINES`].
pub const MAX_GUEST_RUN_OUTPUT_LINE_BYTES: usize = 2_048;

/// How many bytes ONE stage of a pipeline in a delivered script may hand the stage after it, and
/// how much of a redirected command's output is collected for the file it goes to.
///
/// Both are the EMULATOR's numbers, stated for a script that performs real BLOCK I/O: a guest reads
/// a payload back off a device and digests it in one pipeline ([`super::guest_disk_io`]), so a
/// stage carries one copy's worth ([`super::guest_disk_io::MAX_VOLUME_TRANSFER_BYTES`]) rather than
/// the few kilobytes a line of text comes to. What the whole RUN may move through those copies is
/// bounded beside it ([`super::guest_disk_io::MAX_GUEST_RUN_TRANSFER_BYTES`]), so the commands a
/// script carries cannot multiply it.
///
/// A copy's worth is the bytes it moved AND the newline the LAST of its lines is written with: a
/// program's output reaches the stage after it one line at a time, each closed by a newline, so a
/// payload whose own last byte is not one costs that byte more than the payload itself. Without it
/// a read of exactly what one copy may move is a stage that carries NOTHING — the digest a client
/// takes of the bytes it just read back would be the digest of no bytes at all — so the stage is as
/// wide as a whole copy's worth and that one byte.
pub const MAX_GUEST_RUN_PIPE_BYTES: usize =
    super::guest_disk_io::MAX_VOLUME_TRANSFER_BYTES + LINE_TERMINATOR_BYTES;

/// The one byte a line a program wrote is CLOSED by (see [`MAX_GUEST_RUN_PIPE_BYTES`]).
const LINE_TERMINATOR_BYTES: usize = 1;

/// What one program run in a guest answered: the lines it wrote, the lines it reported it
/// could not do and the status it exited with.
#[derive(Debug, Clone, Default)]
pub struct GuestRun {
    /// The lines the program wrote to its standard output, in the order it wrote them.
    pub output: Vec<String>,
    /// The lines the program wrote to its standard ERROR, in the order it wrote them: what a
    /// command reported it could not do, which for a command naming a program the guest does
    /// not have is the shell's own diagnostic. Kept apart from the output above, because the
    /// agent that delivered the script reports the two streams separately.
    pub errors: Vec<String>,
    /// The status the program exited with — the shell's own: zero for a program that ran to
    /// success, and the status a shell answers for a utility it could not find for a command
    /// this guest has no program for.
    pub status: i32,
    /// How long the program STATED it took.
    ///
    /// The session performs the commands in process and spends none of this host's clock on
    /// them, so the time a program took is the time it says it took
    /// ([`::service_common::posix_shell_session::ShellSession::slept`]) — zero for one that
    /// says nothing. A caller whose own API reports a duration, or whose work is not finished
    /// until the program is, honours it; one that reports no duration ignores it.
    pub took: ::std::time::Duration,
}

/// What performing one script in a session came to: what the run ANSWERED ([`GuestRun`]) and
/// the tree it left behind, which becomes the machine's own storage.
struct Performed {
    /// What the run answered, as the caller reads it.
    run: GuestRun,
    /// The tree the script left behind.
    volume: ::service_common::posix_shell_volume::ShellVolume,
}

/// One guest SESSION with its script still to come: everything the run needs, taken out from
/// under the store's guard so the run itself holds none
/// ([`ComAmazonawsEc2Amazonec2State::prepare_instance_guest_run`] states why it has to).
#[derive(Debug)]
pub struct PreparedGuestRun {
    /// The instance whose guest this is, which is where what the run leaves goes back.
    instance_id: String,
    /// The session the script is performed by, holding the machine's tree and its programs.
    session: ShellSession,
    /// The services the machine is running, which a command of the run may change.
    services: super::guest_services::GuestServices,
    /// The devices the run may read and write through.
    services_disks: super::guest_disk_io::GuestDisks,
}

/// What one run LEFT, as the store takes it back
/// ([`ComAmazonawsEc2Amazonec2State::commit_instance_guest_run`]).
#[derive(Debug)]
pub struct PerformedGuestRun {
    /// The instance whose guest ran it.
    instance_id: String,
    /// What the run answered, as the caller reads it.
    run: GuestRun,
    /// The files it left on the machine's storage.
    files: ::std::collections::BTreeMap<String, String>,
    /// What it left RUNNING there.
    services: super::GuestRuntime,
    /// What it wrote through a device, for the volume behind that device.
    disks: super::guest_disk_io::GuestDiskWrites,
}

impl PreparedGuestRun {
    /// Perform `commands` in this session as ONE script, with NO guard of the store held, and
    /// answer what the run left — `None` for a run this process could not perform at all,
    /// which leaves the machine exactly as it was.
    ///
    /// The lines are one script, read by the grammar in one piece: that is what makes a
    /// here-document opened on one line carry the lines under it, and what makes the script's
    /// own exit status the status of its LAST command rather than of whichever line happened to
    /// be last in the list.
    #[must_use]
    pub fn performed(self, commands: &[String]) -> Option<PerformedGuestRun> {
        let Self {
            instance_id,
            session,
            services,
            services_disks,
        } = self;
        let script = commands.join("\n");
        let Performed { run, volume } = performed(session, &script)?;
        Some(PerformedGuestRun {
            instance_id,
            run,
            files: volume
                .stored_files()
                .map(|(path, contents)| (path.to_string(), contents.to_string()))
                .collect(),
            services: services.taken(),
            disks: services_disks.taken(),
        })
    }
}

/// Perform `script` in `session` and answer what it wrote on each stream, the status it ended
/// on, how long it stated it took, and the tree it left behind — `None` for a run this process
/// could not perform at all.
///
/// The run happens on a THREAD OF ITS OWN, given a stack sized for the nesting bound the
/// session was constructed with ([`MAX_GUEST_SCRIPT_STACK_BYTES`]): reading a script costs the
/// grammar's recursive descent, and performing one costs the session's own walk, a level of
/// each per group the script nests — so a program inside that bound is answered by the bound
/// and never by the stack of whichever thread served the request. The thread is joined before
/// this answers, so nothing of the run outlives the call and no work goes on behind it; a
/// thread this process could not start, or one that ended without answering, is `None` — a run
/// that performed nothing, which leaves the machine's storage exactly as it was rather than
/// reporting an outcome nothing produced.
///
/// "Ended without answering" includes a run that unwound: the run is performed inside
/// [`::std::panic::catch_unwind`] ON that thread, so an unwind becomes the `None` above
/// instead of travelling anywhere. Joining the thread is not what contains it — a scope
/// re-raises a scoped thread's unwind when its closure returns, however the handle was
/// joined — and this is performed with the store's guard held, so an unwind reaching the
/// thread that served the request would leave that guard poisoned for every later caller
/// of the store: one script would take the whole emulator's EC2 state with it.
fn performed(mut session: ShellSession, script: &str) -> Option<Performed> {
    let run = move || {
        let mut status = session.run(script);
        if let Some(exited) = session.exit_status() {
            // The script ended the session itself: that status is the run's.
            status = exited;
        }
        let output = session.output().to_vec();
        let errors = session.errors().to_vec();
        // What the script SAID it took, read before the session is consumed.
        let took = session.slept();
        Performed {
            run: GuestRun {
                output,
                errors,
                status,
                took,
            },
            volume: session.into_volume().unwrap_or_default(),
        }
    };
    // The run, contained on the thread it happens on: whatever the script made the session
    // do, this thread answers either what it produced or nothing at all.
    let contained = move || ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(run)).ok();
    ::std::thread::scope(|scope| {
        ::std::thread::Builder::new()
            .stack_size(MAX_GUEST_SCRIPT_STACK_BYTES)
            .spawn_scoped(scope, contained)
            .ok()
            .and_then(|thread| thread.join().ok())
            .flatten()
    })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Perform `commands` in the guest of the instance `instance_id` names, as ONE script,
    /// answering what the guest wrote on each of its two streams and the status it ended on
    /// — or `None` when there is no running, booted guest to perform them in.
    ///
    /// The lines are one script in one session, so a construct spanning lines is the script's
    /// own and a variable one line sets and a file one line writes are there for the next; the
    /// files the session leaves behind become the instance's own storage, the way the boot's
    /// program's do. The run stops early when the script ends the session (`exit`), which is
    /// what a script's own `exit` does.
    pub fn run_in_instance_guest(
        &mut self,
        instance_id: &str,
        commands: &[String],
    ) -> Option<GuestRun> {
        self.run_in_instance_guest_with(instance_id, commands, None, &GuestIdentity::default())
    }

    /// [`Self::run_in_instance_guest`], with `supplied` as the programs the CALLER installs in
    /// the guest beside the machine's own ([`super::guest_metadata::guest_programs`]) and
    /// `identity` as the part of the machine's own identity another service owns.
    ///
    /// A guest has programs whose answers come from state this store cannot read — an agent's
    /// resolver client, which answers out of the account's DNS — and the component that CAN
    /// read that state is the one that reaches into the guest. So it hands those programs over
    /// here rather than this store keeping a second notion of what they answer; the machine's
    /// own programs are asked first, so nothing the caller supplies can shadow one of them
    /// ([`::service_common::posix_shell_session::layered`]). The one FACT of the machine in
    /// the same position — the name of the role its instance profile carries, which IAM holds
    /// — travels the same way, resolved by the caller
    /// ([`super::instance_profile_role_name`]).
    pub fn run_in_instance_guest_with(
        &mut self,
        instance_id: &str,
        commands: &[String],
        supplied: Option<Box<dyn EnvironmentPrograms>>,
        identity: &GuestIdentity,
    ) -> Option<GuestRun> {
        let prepared = self.prepare_instance_guest_run(instance_id, supplied, identity)?;
        let performed = prepared.performed(commands)?;
        Some(self.commit_instance_guest_run(performed))
    }

    /// The SESSION one script is to be performed in, built out of this store and handed back
    /// so the run itself happens with NO guard of this store held
    /// ([`PreparedGuestRun::performed`]) — and `None` when there is no running, booted guest to
    /// perform anything in ([`Self::run_in_instance_guest_with`] states what a guest has to be).
    ///
    /// Why the three steps exist: a program installed in that session by the caller that
    /// reached into the guest may CALL this emulator — an agent's command that moves a file
    /// into an object store, a script's own SDK call — and such a call is served by this
    /// process, on the very thread the script is being performed on. A guard of this store held
    /// across the run would therefore be a guard the call cannot take: the script would wait
    /// for an answer that waits for the guard. So the guard is taken to BUILD the session, given
    /// up for the run, and taken again to keep what the run left
    /// ([`Self::commit_instance_guest_run`]). What the machine holds afterwards is what the run
    /// left, as it is for a run performed in one step: a write another caller makes to the same
    /// machine while a script of it is running is overwritten by the script's own tree, which is
    /// the same outcome the one-step form gives it.
    pub fn prepare_instance_guest_run(
        &self,
        instance_id: &str,
        supplied: Option<Box<dyn EnvironmentPrograms>>,
        identity: &GuestIdentity,
    ) -> Option<PreparedGuestRun> {
        let instance = self.instances.get(instance_id)?;
        if instance.state != InstanceStateName::Running {
            return None;
        }
        // The guest's current boot: the session reads that boot's own identity, and an
        // instance whose guest has not come up yet has none.
        let boot = instance.boots.last()?;
        let (boot_id, number, booted_at) = (boot.boot_id.clone(), boot.number, boot.booted_at);
        let files = instance.guest_files.clone();
        // The machine the program runs on, running as the role the caller resolved for it:
        // which role the instance's profile carries is a fact IAM holds, so it is handed in
        // rather than read here ([`GuestMachine::running_as`]).
        let machine = self.guest_machine(instance).running_as(identity);
        // The services the machine is running now, which a command of this run may install,
        // start or stop ([`super::guest_services`]).
        let services =
            super::guest_services::GuestServices::starting_from(&instance.guest_runtime);
        // The DEVICES this machine's guest can address, each holding the bytes its volume holds:
        // what a command of this run reads and writes through a device goes onto the VOLUME behind
        // it ([`super::guest_disk_io`]).
        let disks = self.guest_disks(&machine.block_devices);
        // How many bytes this machine's storage may hold, which is its own bound and never more
        // than the store has room for ([`super::guest_disk_io::MAX_GUEST_DATA_BYTES`]).
        let storage_bytes = self.guest_storage_allowance(instance_id);
        let volume = guest_volume(
            &machine,
            &boot_id,
            booted_at,
            &files,
            &services.taken(),
            storage_bytes,
        )
        // Where a datagram this script SENDS goes: the machine's own network stack, which
        // carries it to whatever of this guest is listening
        // ([`super::guest_services::GuestServices::send_datagram`]).
        .with_datagram_sink(::std::sync::Arc::new(services.clone()))
        // ... and where the bytes of its BLOCK DEVICES come from: the volumes attached to it, so
        // a command that reads a device with any utility — a digest, `cat`, `head` — reads what
        // that volume holds ([`super::guest_disk_io`]).
        .with_block_device_bytes(::std::sync::Arc::new(disks.clone()));
        // ... and where a write to the machine's own storage is reported: the volume that storage
        // IS, so the bytes a script writes into a file of the root filesystem are I/O on the root
        // volume ([`super::guest_disk_io::storage_write_sink`]).
        let volume = match super::guest_disk_io::storage_write_sink(&disks, &machine.block_devices)
        {
            Some(sink) => volume.with_file_write_sink(sink),
            None => volume,
        };
        let session = ShellSession::in_volume(
            guest_environment(&machine, number),
            SessionConfig {
                bounds: Bounds {
                    commands: MAX_GUEST_RUN_COMMANDS,
                    command_bytes: MAX_GUEST_SCRIPT_BYTES,
                    nesting: MAX_GUEST_SCRIPT_NESTING,
                    substitution_bytes: MAX_GUEST_RUN_PIPE_BYTES,
                    file_bytes: MAX_GUEST_RUN_PIPE_BYTES,
                    ..Bounds::default()
                },
                output: Some(OutputLimits {
                    lines: MAX_GUEST_RUN_OUTPUT_LINES,
                    line_bytes: MAX_GUEST_RUN_OUTPUT_LINE_BYTES,
                }),
                ..SessionConfig::default()
            },
            volume,
        )
        // The agent that delivered the script reports the two streams as members of its own,
        // so a diagnostic is not part of what the script wrote
        // ([`::service_common::posix_shell_session::ShellSession::reporting_both_streams`]).
        .reporting_both_streams()
        // The programs this machine has installed in it, which an agent's command reads the
        // instance's own facts with ([`super::guest_metadata`]) and installs and starts its
        // services with ([`super::guest_services`]) — the same programs the boot's own
        // program has — and, under them, the ones the caller installed.
        .with_environment_programs({
            let own = super::guest_services::installed_programs(&machine, &services, &disks);
            match supplied {
                Some(supplied) => layered(own, supplied),
                None => own,
            }
        });
        // The lines are ONE script, read by the grammar in one piece: that is what makes a
        // here-document opened on one line carry the lines under it, and what makes the
        // script's own exit status the status of its LAST command rather than of whichever
        // line happened to be last in the list. The session is handed back with the script
        // still to come, so the run happens with this store's guard given up.
        Some(PreparedGuestRun {
            instance_id: instance_id.to_string(),
            session,
            services,
            services_disks: disks,
        })
    }

    /// Keep what one run left on the machine it ran in, and answer what that run came to.
    ///
    /// The write half of [`Self::prepare_instance_guest_run`], taken with this store's guard
    /// held again: the files the script left are the machine's storage, what it left RUNNING is
    /// the machine's services, and what it wrote THROUGH a device goes onto the volume behind
    /// that device — the same three writes a run performed in one step makes. An instance this
    /// store no longer holds takes none of them; the run itself is still reported, because it
    /// happened.
    pub fn commit_instance_guest_run(&mut self, performed: PerformedGuestRun) -> GuestRun {
        let PerformedGuestRun {
            instance_id,
            run,
            files,
            services,
            disks,
        } = performed;
        if let Some(instance) = self.instances.get_mut(&instance_id) {
            let mut runtime = services;
            // The space a file held on the filesystem is the filesystem's again once the file
            // is gone ([`super::guest_filesystem::release_removed_files`]).
            super::guest_filesystem::release_removed_files(&mut runtime, &files);
            instance.guest_files = files;
            instance.guest_runtime = runtime;
        }
        self.apply_guest_disk_writes(disks, ::std::time::SystemTime::now());
        run
    }

    /// What the guest of the instance `instance_id` names holds at `path`, or `None` when it
    /// holds nothing there.
    ///
    /// The counterpart of [`Self::put_in_instance_guest`] for an agent that READS the
    /// machine's storage — the artifact it put there on an earlier run — rather than
    /// performing a program in it. The path is resolved the way the tree a program runs in
    /// resolves one ([`::service_common::posix_shell_volume::resolved_path`]), so the agent and
    /// the guest's own programs name one file by one key.
    pub fn read_in_instance_guest(&self, instance_id: &str, path: &str) -> Option<String> {
        let resolved = ::service_common::posix_shell_volume::resolved_path(path)?;
        self.instances
            .get(instance_id)?
            .guest_files
            .get(&resolved)
            .cloned()
    }

    /// Whether that guest holds anything at `path`: a file, or a directory something under
    /// it makes.
    ///
    /// What an agent asks before it writes, because a path the machine already holds is the
    /// difference between replacing a file and creating one.
    pub fn instance_guest_holds(&self, instance_id: &str, path: &str) -> bool {
        let Some(resolved) = ::service_common::posix_shell_volume::resolved_path(path) else {
            return false;
        };
        let under = format!("{}/", resolved.trim_end_matches('/'));
        self.instances.get(instance_id).is_some_and(|instance| {
            instance.guest_files.contains_key(&resolved)
                || instance
                    .guest_files
                    .keys()
                    .any(|held| held.starts_with(&under))
        })
    }

    /// Put `files` on the storage of that guest, as `(path, contents)` — what an AGENT
    /// inside it writes: the artifact it fetched for the work it was sent, and the files that
    /// work installs.
    ///
    /// Answering the paths the guest holds afterwards, or `None` when there is no running,
    /// booted guest to write to ([`Self::run_in_instance_guest_with`] states what a guest has
    /// to be). The writes go through the SAME tree a program of this guest runs in
    /// ([`super::guest_console::guest_volume`]), so one path is resolved one way, the
    /// directories above a file are made the way a machine that holds it has them, a write to
    /// a path the kernel publishes is refused as it is for the guest's own program, and every
    /// write is held to that tree's bounds: a write past them does not happen and its path is
    /// left out of the answer.
    pub fn put_in_instance_guest(
        &mut self,
        instance_id: &str,
        files: &[(String, String)],
    ) -> Option<Vec<String>> {
        self.in_instance_guest_volume(instance_id, |volume| {
            let mut written: Vec<String> = Vec::new();
            for (path, contents) in files {
                if volume.put_file(path, contents) {
                    if let Some(resolved) = volume.resolve(path) {
                        written.push(resolved);
                    }
                }
            }
            written
        })
    }

    /// Remove what `paths` names from the storage of that guest, as an AGENT inside it
    /// does: the files of an earlier run of its work, taken off before the next run puts
    /// its own there.
    ///
    /// Answering the paths the guest held and no longer holds, or `None` when there is no
    /// running, booted guest. The removals go through the SAME tree
    /// ([`Self::put_in_instance_guest`] states why), so a path the kernel publishes is not
    /// one an agent can take away and a path the guest does not hold is simply not in the
    /// answer.
    pub fn remove_in_instance_guest(
        &mut self,
        instance_id: &str,
        paths: &[String],
    ) -> Option<Vec<String>> {
        self.in_instance_guest_volume(instance_id, |volume| {
            let mut removed: Vec<String> = Vec::new();
            for path in paths {
                if let Some(resolved) = volume.resolve(path) {
                    if volume.remove_file(&resolved) {
                        removed.push(resolved);
                    }
                }
            }
            removed
        })
    }

    /// Let `change` change the TREE of that guest's storage, and keep what it left there.
    ///
    /// The one place an agent's own write of a machine reaches that tree, so the two seams
    /// above cannot differ about which guest has one, which tree it is
    /// ([`super::guest_console::guest_volume`]) or what the machine holds afterwards:
    /// `None` for an instance that is not running or whose operating system has not come up,
    /// and otherwise what `change` answered, with the files it left behind kept as that
    /// machine's storage — the same write a program of this guest's own makes
    /// ([`Self::run_in_instance_guest_with`]).
    fn in_instance_guest_volume<T>(
        &mut self,
        instance_id: &str,
        change: impl FnOnce(&mut ::service_common::posix_shell_volume::ShellVolume) -> T,
    ) -> Option<T> {
        let instance = self.instances.get(instance_id)?;
        if instance.state != InstanceStateName::Running {
            return None;
        }
        let boot = instance.boots.last()?;
        let (boot_id, booted_at) = (boot.boot_id.clone(), boot.booted_at);
        let held = instance.guest_files.clone();
        let runtime = instance.guest_runtime.clone();
        let machine = self.guest_machine(instance);
        let storage_bytes = self.guest_storage_allowance(instance_id);
        let mut volume = guest_volume(
            &machine,
            &boot_id,
            booted_at,
            &held,
            &runtime,
            storage_bytes,
        );
        let answered = change(&mut volume);
        let stored: ::std::collections::BTreeMap<String, String> = volume
            .stored_files()
            .map(|(path, contents)| (path.to_string(), contents.to_string()))
            .collect();
        if let Some(instance) = self.instances.get_mut(instance_id) {
            instance.guest_files = stored;
        }
        Some(answered)
    }

    /// Whether the instance `instance_id` names has a guest a program can run in now: it is
    /// running and its operating system has come up.
    ///
    /// Asked by a caller that reports WHETHER it delivered a program, so it does not have to
    /// read this store's instance shapes itself.
    pub fn instance_guest_is_up(&self, instance_id: &str) -> bool {
        self.instances.get(instance_id).is_some_and(|instance| {
            instance.state == InstanceStateName::Running && !instance.boots.is_empty()
        })
    }
}

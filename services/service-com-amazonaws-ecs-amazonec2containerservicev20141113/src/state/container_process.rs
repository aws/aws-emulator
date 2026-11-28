// SPDX-License-Identifier: CC0-1.0

//! What a container's PROCESS runs as: the settings its definition gives it, and what the
//! utilities that report a process's own session answer about them.
//!
//! # The rule this module implements
//!
//! A container's definition does not only say WHAT runs: it says what the process that runs
//! it is. `ContainerDefinition$workingDirectory` is "the working directory to run commands
//! inside the container in", `$user` "the user to use inside the container", `$ulimits` the
//! "`ulimit` settings to pass to the container", and the task's `ephemeralStorage` "the
//! amount of ephemeral storage to allocate for the task", which is the storage the container
//! writes on. Each of those is a fact ABOUT the running process, so the utilities a program
//! asks them with — `pwd` for the directory, `id` for the user, `ulimit` for a resource
//! limit, `df` for the space on the filesystem — answer the values the launch resolved,
//! exactly as a program inside a real task reads them back.
//!
//! The values come from the records this store already holds: the container definition the
//! run was launched from and the task's own resolved ephemeral storage
//! ([`super::resolved_ephemeral_storage_gib`]), with the filesystem's usage read off the
//! container's own layer ([`super::container_filesystem`]). Nothing here is invented for the
//! report.
//!
//! The one setting here that is not only REPORTED is the memory the definition gives the
//! container ([`ContainerProcess::memory_limit_bytes`]): `ContainerDefinition$memory` is "the
//! amount (in MiB) of memory to present to the container" and "if your container attempts to
//! exceed the memory specified here, the container is killed", so it is a limit the container
//! RUNS under rather than a value a utility writes — the session performing the program
//! enforces it ([`super::container_shell`]).
//!
//! # What is NOT answered
//!
//! The settings this emulator holds are the ones the TASK DEFINITION states; everything else
//! about the process comes from the image or the host, and this emulator holds neither. So:
//!
//! * a `user` named as a NAME rather than as an id needs the image's own user database to
//!   resolve to an id, so the numeric questions about it are not answered;
//! * a resource limit the definition does not set is the one "set by the operating system"
//!   (`Ulimit`), except for `nofile` ON FARGATE, whose default that same documentation states
//!   ([`FARGATE_DEFAULT_NOFILE_LIMIT`]) — a task on capacity registered to its cluster runs
//!   on a host whose limits this emulator does not hold;
//! * the space on a filesystem whose size is not the task's ephemeral storage — a volume
//!   mounted into the container, or the disk of a container instance this store holds no
//!   size for — is not answered either.
//!
//! Each of those is reported to the program as a command that did not run (the rule
//! [`super::container_shell`] states for one), rather than as a value nothing backs.

use crate::ops::types::Ulimit;

use super::*;

/// `Ulimit`: "Amazon ECS tasks hosted on Fargate use the default resource limit values set
/// by the operating system with the exception of the `nofile` resource limit parameter which
/// Fargate overrides ... The default `nofile` soft limit is `65535` and the default hard
/// limit is `65535`." Both halves are that one value, so one constant states it — and it is
/// the default of a task hosted on FARGATE alone, which is what that sentence scopes it to.
pub const FARGATE_DEFAULT_NOFILE_LIMIT: i64 = 65_535;

/// The `ulimit` value of a resource that is not limited at all, which is what the utility
/// writes for one: a limit set to this is reported as [`UNLIMITED_LIMIT_TEXT`].
const UNLIMITED_LIMIT: i64 = -1;

/// How the `ulimit` utility writes a resource that is not limited.
const UNLIMITED_LIMIT_TEXT: &str = "unlimited";

/// The user a container's process acts as when its definition narrows it to nobody, as the
/// id and the name of it. A container runtime runs a process as the superuser unless the
/// definition names another user — "don't run containers using the root user (UID 0)"
/// (`ContainerDefinition$user`) is advice about the default, so it is the default — which is
/// also the owner the files such a process creates belong to
/// ([`super::container_filesystem`]).
const DEFAULT_USER: (i64, &str) = (0, "root");

/// The group of that same default.
const DEFAULT_GROUP: (i64, &str) = (0, "root");

/// How many bytes of a filesystem one block of `df`'s default report is: the utility "writes
/// ... the amount of space in 1024-byte units", which is also what its `-k` option asks for.
const DF_BLOCK_BYTES: u64 = 1024;

/// How many of those blocks make up the unit `df -m` reports in.
const DF_MEBIBYTE_BLOCKS: u64 = 1024;

/// How many of those blocks make up one gibibyte, which is the unit a task's
/// `ephemeralStorage` is sized in and what `df -h` scales a filesystem of this size to.
const DF_GIBIBYTE_BLOCKS: u64 = 1024 * 1024;

/// The device a container's own writable layer is mounted from, as the container runtime
/// names it: a container's root filesystem is the union mount its storage driver assembles,
/// which is what `df` reports in its first column for a path on that layer.
const CONTAINER_LAYER_DEVICE: &str = "overlay";

/// Where that layer is mounted, which is the root of the container's filesystem.
const CONTAINER_LAYER_MOUNT_POINT: &str = "/";

/// How many bytes one mebibyte is, which is the unit a container's and a task's memory are
/// stated in (`ContainerDefinition$memory`, `TaskDefinition$memory`).
const BYTES_PER_MIB: u64 = 1024 * 1024;

/// How many bytes of a resource one `ulimit` BLOCK is: the unit the utility reports the
/// limits on a file's size in ("in 512-byte blocks"), while the modelled `Ulimit` value is
/// the byte count the runtime sets.
const ULIMIT_BLOCK_BYTES: i64 = 512;

/// How many bytes one kibibyte is, which is the unit `ulimit` reports the limits on a
/// process's memory in.
const ULIMIT_KIBIBYTE_BYTES: i64 = 1024;

/// ONE resource limit as the `ulimit` utility addresses it: the modelled `UlimitName`
/// (`smithy_query(query="shape",shape="UlimitName")`), the option letter the utility selects
/// that resource with, and how many bytes of it one unit of the utility's report is.
///
/// The letters and their units are the utility's own documentation: the limits on a file's
/// size are written "in 512-byte blocks" (`-c` a core file, `-f` a file the process writes),
/// the limits on memory "in kibibytes" (`-d` the data segment, `-l` what may be locked into
/// memory, `-m` the resident set, `-s` the stack), and every other limit is a count, a
/// number of seconds or a number of bytes — the same unit the modelled member carries, so
/// nothing is scaled.
const ULIMIT_RESOURCES: &[(&str, char, i64)] = &[
    ("core", 'c', ULIMIT_BLOCK_BYTES),
    ("cpu", 't', 1),
    ("data", 'd', ULIMIT_KIBIBYTE_BYTES),
    ("fsize", 'f', ULIMIT_BLOCK_BYTES),
    ("locks", 'x', 1),
    ("memlock", 'l', ULIMIT_KIBIBYTE_BYTES),
    ("msgqueue", 'q', 1),
    ("nice", 'e', 1),
    ("nofile", 'n', 1),
    ("nproc", 'u', 1),
    ("rss", 'm', ULIMIT_KIBIBYTE_BYTES),
    ("rtprio", 'r', 1),
    ("rttime", 'R', 1),
    ("sigpending", 'i', 1),
    ("stack", 's', ULIMIT_KIBIBYTE_BYTES),
];

/// The `UlimitName` whose default a Fargate task holds ([`FARGATE_DEFAULT_NOFILE_LIMIT`]).
const NOFILE_LIMIT: &str = "nofile";

/// The resource the `ulimit` utility reports when an invocation names none, which is its own
/// default: the limit on the size of a file the process writes.
const DEFAULT_ULIMIT_RESOURCE: char = 'f';

/// What ONE container's process runs as, taken off the records the run was launched from.
///
/// Held beside the session rather than in its variables: these are facts about the process,
/// not environment a program can change, and a program that asks for one asks the utility
/// for it.
#[derive(Debug, Default, Clone)]
pub struct ContainerProcess {
    /// `ContainerDefinition$user`, as written — `user`, `user:group`, `uid`, `uid:gid`,
    /// `user:gid` or `uid:group` — and `None` for a definition that narrows the process to
    /// nobody.
    user: Option<String>,
    /// `ContainerDefinition$ulimits`: the resource limits the definition passes to the
    /// container.
    ulimits: Vec<Ulimit>,
    /// The ephemeral storage, in GiB, the task's placement gave it — the size of the
    /// filesystem its containers write on — and `None` for a task whose capacity reports
    /// none.
    storage_gib: Option<i32>,
    /// Whether the task runs on FARGATE, which is the one capacity whose default for a
    /// resource limit this service holds ([`FARGATE_DEFAULT_NOFILE_LIMIT`]).
    on_fargate: bool,
    /// The memory HARD LIMIT this process runs under, in bytes, and `None` for a launch that
    /// states none ([`Self::memory_limit_bytes`]).
    memory_limit_bytes: Option<u64>,
}

impl ContainerProcess {
    /// What the process of the container `definition` describes runs as, in `task` — whose
    /// placement decides the ephemeral storage its containers write on and whether they run
    /// on Fargate ([`places_on_fargate`], the one predicate every reader of that question
    /// goes through).
    ///
    /// A launch this store holds no container definition for carries the settings a
    /// definition that states none would: nothing is assumed about such a container beyond
    /// its task's own capacity.
    pub(super) fn of(definition: Option<&ContainerDefinition>, task: &TaskRecord) -> Self {
        Self {
            // The memory the launch gives this container, which is the limit its program runs
            // under ([`container_memory_limit_bytes`]).
            memory_limit_bytes: container_memory_limit_bytes(definition, task),
            ..Self::of_settings(
                definition,
                task.ephemeral_storage_gib,
                places_on_fargate(
                    task.launch_type.as_deref(),
                    &placed_through_strategy(task.capacity_provider_name.as_deref()),
                ),
            )
        }
    }

    /// The same, off the three facts themselves rather than off a stored task.
    fn of_settings(
        definition: Option<&ContainerDefinition>,
        storage_gib: Option<i32>,
        on_fargate: bool,
    ) -> Self {
        Self {
            user: definition
                .and_then(|definition| definition.user.as_deref())
                .map(str::trim)
                .filter(|user| !user.is_empty())
                .map(str::to_string),
            ulimits: definition
                .and_then(|definition| definition.ulimits.clone())
                .unwrap_or_default(),
            storage_gib,
            on_fargate,
            // A launch read off the three facts alone states no memory limit: the one the
            // container runs under is read off the definition and the task together
            // ([`Self::of`], [`container_memory_limit_bytes`]).
            memory_limit_bytes: None,
        }
    }

    /// The memory HARD LIMIT the process of this container runs under, in bytes — and `None`
    /// for a launch that states none, which runs under no limit of its own.
    ///
    /// What a session performing the container's program is held to
    /// ([`super::container_shell`]): a program whose values would take the container past it
    /// is KILLED, because `ContainerDefinition$memory` is the amount of memory "to present to
    /// the container" and "if your container attempts to exceed the memory specified here, the
    /// container is killed".
    pub fn memory_limit_bytes(&self) -> Option<u64> {
        self.memory_limit_bytes
    }

    /// What `id` writes for this process, or `None` for an invocation it cannot answer: a
    /// form this session does not perform, or a question about a user this emulator holds no
    /// value for (see the module docs).
    ///
    /// The forms answered are the ones that ask for ONE value — `-u` the user id, `-g` the
    /// group id, and either of them with `-n` for "the name of the user or group ... instead
    /// of the numeric id" — written as the one line the utility writes. The listing a bare
    /// `id` writes carries the process's supplementary groups, which this emulator holds
    /// none of, so it is not written at all.
    pub fn identity_report(&self, arguments: &[String]) -> Option<String> {
        let mut want_group = false;
        let mut want_user = false;
        let mut by_name = false;
        for argument in arguments {
            let letters = argument.strip_prefix('-').filter(|rest| !rest.is_empty())?;
            for letter in letters.chars() {
                match letter {
                    'u' => want_user = true,
                    'g' => want_group = true,
                    'n' => by_name = true,
                    _ => return None,
                }
            }
        }
        // Exactly one of the two is asked for: `id -ug` is the usage this session does not
        // read, and a bare `id` writes the listing it holds no supplementary groups for.
        if want_user == want_group {
            return None;
        }
        let (id, name) = if want_user {
            self.user_identity()?
        } else {
            self.group_identity()?
        };
        let written = if by_name { name? } else { id?.to_string() };
        Some(format!("{written}\n"))
    }

    /// The user id and user name of this process, as much of each as is known: the id for a
    /// `user` member naming one and the name for one naming a name, and both for a process
    /// no member narrows ([`DEFAULT_USER`]). `None` where the member is written in a form
    /// this reads nothing out of.
    fn user_identity(&self) -> Option<Identity> {
        let Some(user) = self.user.as_deref() else {
            let (id, name) = DEFAULT_USER;
            return Some((Some(id), Some(name.to_string())));
        };
        identity_of(user.split(':').next().unwrap_or_default())
    }

    /// The group id and group name of this process, read the same way off the half of `user`
    /// after the separator — and, for a member naming no group at all, the group of the
    /// default ([`DEFAULT_GROUP`]), which is the group such a process is in.
    fn group_identity(&self) -> Option<Identity> {
        let group = self
            .user
            .as_deref()
            .and_then(|user| user.split_once(':'))
            .map(|(_, group)| group);
        let Some(group) = group else {
            let (id, name) = DEFAULT_GROUP;
            return Some((Some(id), Some(name.to_string())));
        };
        identity_of(group)
    }

    /// What `ulimit` writes for this process, or `None` for an invocation it cannot answer:
    /// a form this session does not perform (a listing, a limit to SET, more than one
    /// resource at a time), or a resource whose limit this emulator holds no value for (see
    /// the module docs).
    ///
    /// The form answered is the one that READS one limit: the option letter of the resource
    /// ([`ULIMIT_RESOURCES`]), with `-S` or `-H` selecting the soft or the hard limit — the
    /// soft limit is what the utility reports when neither is given.
    pub fn limit_report(&self, arguments: &[String]) -> Option<String> {
        let mut resource: Option<char> = None;
        let mut hard = false;
        for argument in arguments {
            let letters = argument.strip_prefix('-').filter(|rest| !rest.is_empty())?;
            for letter in letters.chars() {
                match letter {
                    'H' => hard = true,
                    'S' => hard = false,
                    // One resource per invocation: a second one, and the `-a` that asks for
                    // every limit at once, are the listing forms this writes none of.
                    _ if resource.is_some() => return None,
                    letter => resource = Some(letter),
                }
            }
        }
        let resource = resource.unwrap_or(DEFAULT_ULIMIT_RESOURCE);
        let (name, _, unit) = ULIMIT_RESOURCES
            .iter()
            .find(|(_, letter, _)| *letter == resource)?;
        let value = self.limit_value(name, hard)?;
        if value == UNLIMITED_LIMIT {
            return Some(format!("{UNLIMITED_LIMIT_TEXT}\n"));
        }
        Some(format!("{}\n", value / (*unit).max(1)))
    }

    /// The value of the limit `name` this process runs under — its hard limit when `hard`,
    /// else its soft limit — in the unit the modelled member carries.
    ///
    /// The definition's own setting first; else the one default this service states, for the
    /// one capacity it is the default of ([`FARGATE_DEFAULT_NOFILE_LIMIT`]); else nothing,
    /// because the value is then the operating system's and this emulator holds no host.
    fn limit_value(&self, name: &str, hard: bool) -> Option<i64> {
        if let Some(ulimit) = self.ulimits.iter().find(|ulimit| ulimit.name == name) {
            let limit = if hard {
                ulimit.hard_limit
            } else {
                ulimit.soft_limit
            };
            return Some(i64::from(limit));
        }
        (self.on_fargate && name == NOFILE_LIMIT).then_some(FARGATE_DEFAULT_NOFILE_LIMIT)
    }

    /// What `df` writes for the filesystem a container's program writes on — the task's
    /// ephemeral storage — with `used_bytes` of it held, or `None` for an invocation it
    /// cannot answer (see the module docs).
    ///
    /// `in_a_mount` is whether an operand named a path inside a volume mounted into the
    /// container: that is another filesystem, whose size is the volume's rather than the
    /// task's, so nothing is reported for it.
    ///
    /// The report is the utility's own: a header and one row per filesystem, each carrying
    /// the device, its size, how much of it is used, how much is available, how full it is
    /// and where it is mounted. `-k` and `-P` ask for the 1024-byte units the report is in
    /// anyway, `-m` for mebibytes and `-h` for the largest unit the size fits in.
    pub fn storage_report(
        &self,
        arguments: &[String],
        used_bytes: u64,
        in_a_mount: bool,
    ) -> Option<String> {
        let mut scale = Scale::Blocks;
        for argument in arguments {
            // An operand is a PATH, and which filesystem each one is on is what the caller
            // resolved (`in_a_mount`): every path outside a mount is on the container's own
            // layer, which is the one filesystem this report carries.
            let Some(letters) = argument.strip_prefix('-').filter(|rest| !rest.is_empty()) else {
                continue;
            };
            for letter in letters.chars() {
                match letter {
                    'k' | 'P' => scale = Scale::Blocks,
                    'm' => scale = Scale::Mebibytes,
                    'h' => scale = Scale::Human,
                    _ => return None,
                }
            }
        }
        if in_a_mount {
            return None;
        }
        let size = u64::try_from(self.storage_gib?).ok()? * DF_GIBIBYTE_BLOCKS;
        let used = used_bytes.div_ceil(DF_BLOCK_BYTES).min(size);
        let available = size - used;
        // The capacity the utility reports is rounded UP, so a filesystem holding anything
        // at all is never reported as empty.
        let capacity = if size == 0 {
            0
        } else {
            (used * 100).div_ceil(size)
        };
        let mut report = format!(
            "{:<20} {:>12} {:>12} {:>12} {:>4} {}\n",
            "Filesystem",
            scale.heading(),
            "Used",
            "Available",
            "Use%",
            "Mounted on"
        );
        report.push_str(&format!(
            "{:<20} {:>12} {:>12} {:>12} {:>3}% {}\n",
            CONTAINER_LAYER_DEVICE,
            scale.of(size),
            scale.of(used),
            scale.of(available),
            capacity,
            CONTAINER_LAYER_MOUNT_POINT
        ));
        Some(report)
    }
}

/// The id and the name one half of a `user` member names, as much of each as is known.
type Identity = (Option<i64>, Option<String>);

/// The unit a `df` report is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    /// The 1024-byte blocks the report is in when nothing asks for another unit, which is
    /// also what `-k` and `-P` ask for.
    Blocks,
    /// `-m`: mebibytes.
    Mebibytes,
    /// `-h`: the largest unit the size fits in, written with that unit's own letter.
    Human,
}

impl Scale {
    /// What the size column of the report is headed with, which names the unit the sizes in
    /// it are in — and, for a human-readable report, the sizes the utility labels one by
    /// one.
    fn heading(self) -> &'static str {
        match self {
            Self::Blocks => "1K-blocks",
            Self::Mebibytes => "1M-blocks",
            Self::Human => "Size",
        }
    }

    /// `blocks` 1024-byte blocks, written in this unit.
    fn of(self, blocks: u64) -> String {
        match self {
            Self::Blocks => blocks.to_string(),
            Self::Mebibytes => (blocks / DF_MEBIBYTE_BLOCKS).to_string(),
            // A filesystem holding nothing at all is written as the plain `0` the utility
            // writes for it, with no unit to scale it to.
            Self::Human if blocks == 0 => "0".to_string(),
            Self::Human if blocks >= DF_GIBIBYTE_BLOCKS => {
                let tenths = (blocks * 10).div_ceil(DF_GIBIBYTE_BLOCKS);
                format!("{}.{}G", tenths / 10, tenths % 10)
            }
            Self::Human if blocks >= DF_MEBIBYTE_BLOCKS => {
                let tenths = (blocks * 10).div_ceil(DF_MEBIBYTE_BLOCKS);
                format!("{}.{}M", tenths / 10, tenths % 10)
            }
            Self::Human => format!("{blocks}K"),
        }
    }
}

/// The id and the name one half of a `user` member names: "if specifying a UID or GID, you
/// must specify it as a positive integer", so a half written as digits is an id and no name
/// of that id is held here, while a half written as anything else is a name whose id is in
/// the image's own user database.
///
/// `None` for a half that names neither — an empty one.
fn identity_of(half: &str) -> Option<Identity> {
    if half.is_empty() {
        return None;
    }
    match half.parse::<i64>() {
        Ok(id) => Some((Some(id), None)),
        Err(_) => Some((None, Some(half.to_string()))),
    }
}

/// What `pwd` writes: "the absolute pathname of the current working directory", which is the
/// directory the container's definition runs its commands in. `None` for an invocation this
/// session does not perform — only the two options that choose how a symbolic link in the
/// path is written are read, and this filesystem holds none.
pub fn working_directory_report(arguments: &[String], working_directory: &str) -> Option<String> {
    for argument in arguments {
        if argument != "-P" && argument != "-L" {
            return None;
        }
    }
    Some(format!("{working_directory}\n"))
}

/// The memory HARD LIMIT the process of the container `definition` describes runs under in
/// `task`, in bytes — and `None` where neither states one, which is a container held to no
/// limit of its own.
///
/// `ContainerDefinition$memory` is "the amount (in MiB) of memory to present to the
/// container", and "if your container attempts to exceed the memory specified here, the
/// container is killed", so a definition that states it states the container's own ceiling.
/// A definition that states none is held to the TASK's: the same member documents that "if
/// you specify both a container-level `memory` and `memoryReservation` value" the hard limit
/// is the container's, while for a task whose containers state no memory of their own the
/// amount is the task-level one ("you must specify at least one of `memory` or
/// `memoryReservation` ... or at the task level"), which is the memory the whole task — and
/// so each container in it — may use.
///
/// `memoryReservation` is deliberately NOT read: it is "the soft limit (in MiB) of memory to
/// reserve for the container", which a container is not killed for exceeding, so it states no
/// ceiling at all.
fn container_memory_limit_bytes(
    definition: Option<&ContainerDefinition>,
    task: &TaskRecord,
) -> Option<u64> {
    let declared = definition
        .and_then(|definition| definition.memory)
        .map(i64::from)
        .or_else(|| task.memory.as_deref().and_then(memory_mib))
        .filter(|mib| *mib > 0)?;
    Some(
        u64::try_from(declared)
            .unwrap_or_default()
            .saturating_mul(BYTES_PER_MIB),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A definition naming `user` and `ulimits`.
    fn definition(user: Option<&str>, ulimits: Vec<Ulimit>) -> ContainerDefinition {
        ContainerDefinition {
            name: Some("app".to_string()),
            user: user.map(str::to_string),
            ulimits: Some(ulimits),
            ..ContainerDefinition::default()
        }
    }

    /// One `nofile` limit.
    fn nofile(soft_limit: i32, hard_limit: i32) -> Ulimit {
        Ulimit {
            name: NOFILE_LIMIT.to_string(),
            soft_limit,
            hard_limit,
        }
    }

    /// What one row of the identity table expects of each form of `id`.
    type IdentityRow = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    );

    /// RULE: `id` answers the id or the name the definition's `user` names, in each of the
    /// forms that member may be written in.
    #[test]
    fn the_identity_report_answers_the_user_the_definition_names() {
        // (case, the `user` member, `id -u`, `id -g`, `id -u -n`, `id -gn`)
        let rows: &[IdentityRow] = &[
            ("no user", None, Some("0"), Some("0"), Some("root"), Some("root")),
            ("uid", Some("1234"), Some("1234"), Some("0"), None, Some("root")),
            ("uid:gid", Some("1234:5678"), Some("1234"), Some("5678"), None, None),
            ("user", Some("appuser"), None, Some("0"), Some("appuser"), Some("root")),
            (
                "user:group",
                Some("appuser:staff"),
                None,
                None,
                Some("appuser"),
                Some("staff"),
            ),
            ("uid:group", Some("1234:staff"), Some("1234"), None, None, Some("staff")),
            (
                "user:gid",
                Some("appuser:5678"),
                None,
                Some("5678"),
                Some("appuser"),
                None,
            ),
        ];
        for (case, user, uid, gid, name, group) in rows {
            let process =
                ContainerProcess::of_settings(Some(&definition(*user, Vec::new())), None, true);
            for (form, expected) in [
                (vec!["-u"], uid),
                (vec!["-g"], gid),
                (vec!["-u", "-n"], name),
                (vec!["-gn"], group),
            ] {
                let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
                let written = process.identity_report(&arguments);
                let expected = expected.map(|value| format!("{value}\n"));
                assert_eq!(
                    written, expected,
                    "case {case:?} ({form:?}): expected {expected:?}, got {written:?}"
                );
            }
        }
    }

    /// RULE: a form of `id` this session does not perform is answered by nothing at all.
    #[test]
    fn the_identity_report_declines_the_forms_it_does_not_perform() {
        let process =
            ContainerProcess::of_settings(Some(&definition(Some("1234"), Vec::new())), None, true);
        for form in [vec![], vec!["-ug"], vec!["-G"], vec!["1234"], vec!["-u", "-z"]] {
            let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
            let written = process.identity_report(&arguments);
            assert!(
                written.is_none(),
                "case {form:?}: expected no report, got {written:?}"
            );
        }
    }

    /// RULE: `ulimit` answers the limit the definition sets for the resource the option
    /// letter names, in the unit the utility reports that resource in, with the soft limit
    /// the one it reports when neither `-S` nor `-H` is given.
    #[test]
    fn the_limit_report_answers_the_limit_the_definition_sets() {
        // (case, the limit set, the invocation, what the utility writes)
        let rows: &[(&str, Ulimit, &[&str], Option<&str>)] = &[
            ("nofile soft", nofile(1024, 4096), &["-n"], Some("1024")),
            ("nofile soft, asked for", nofile(1024, 4096), &["-Sn"], Some("1024")),
            ("nofile hard", nofile(1024, 4096), &["-Hn"], Some("4096")),
            ("nofile hard, split", nofile(1024, 4096), &["-H", "-n"], Some("4096")),
            (
                // "in 512-byte blocks": the modelled value is the byte count the runtime
                // sets, and the utility reports blocks of it.
                "fsize in blocks",
                Ulimit { name: "fsize".to_string(), soft_limit: 10_240, hard_limit: 10_240 },
                &["-f"],
                Some("20"),
            ),
            (
                // "in kibibytes".
                "stack in kibibytes",
                Ulimit { name: "stack".to_string(), soft_limit: 8_388_608, hard_limit: 8_388_608 },
                &["-s"],
                Some("8192"),
            ),
            (
                "a limit of -1 is unlimited",
                Ulimit { name: "nproc".to_string(), soft_limit: -1, hard_limit: -1 },
                &["-u"],
                Some("unlimited"),
            ),
            (
                // A resource the definition does not set, and which has no default of this
                // service's: the value is the operating system's.
                "a resource with no setting and no default",
                nofile(1024, 4096),
                &["-t"],
                None,
            ),
            ("a listing", nofile(1024, 4096), &["-a"], None),
            ("two resources", nofile(1024, 4096), &["-n", "-u"], None),
            ("a limit to set", nofile(1024, 4096), &["-n", "2048"], None),
        ];
        for (case, ulimit, form, expected) in rows {
            let process = ContainerProcess::of_settings(
                Some(&definition(None, vec![ulimit.clone()])),
                None,
                true,
            );
            let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
            let written = process.limit_report(&arguments);
            let expected = expected.map(|value| format!("{value}\n"));
            assert_eq!(
                written, expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// RULE: a `nofile` limit the definition does not set is the one Fargate overrides, whose
    /// value the modelled member documents — and both halves of it are that value — while a
    /// task on capacity registered to its cluster runs under the HOST's limits, which this
    /// emulator holds no value for.
    #[test]
    fn an_unset_nofile_limit_is_the_documented_fargate_default() {
        for form in [vec!["-n"], vec!["-Sn"], vec!["-Hn"]] {
            let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
            let on_fargate =
                ContainerProcess::of_settings(Some(&definition(None, Vec::new())), None, true)
                    .limit_report(&arguments);
            assert_eq!(
                on_fargate,
                Some(format!("{FARGATE_DEFAULT_NOFILE_LIMIT}\n")),
                "case {form:?} (Fargate): expected the documented default, got {on_fargate:?}"
            );
            let on_capacity =
                ContainerProcess::of_settings(Some(&definition(None, Vec::new())), None, false)
                    .limit_report(&arguments);
            assert!(
                on_capacity.is_none(),
                "case {form:?} (a registered container instance): the limit is the host's, got \
                 {on_capacity:?}"
            );
        }
    }

    /// RULE: every modelled `UlimitName` is a resource the utility can be asked about, so a
    /// model revision cannot leave a limit with no option letter.
    #[test]
    fn every_modelled_ulimit_name_has_an_option_letter() {
        for name in crate::ops::types::ULIMIT_NAME_VALUES {
            assert!(
                ULIMIT_RESOURCES
                    .iter()
                    .any(|(resource, _, _)| resource == name),
                "expected {name} to name a resource `ulimit` reports, got {ULIMIT_RESOURCES:?}"
            );
        }
    }

    /// What one row of the storage table asks for: the case, the storage the task holds, the
    /// invocation, and the size column the row of the report must carry.
    type StorageRow = (&'static str, Option<i32>, &'static [&'static str], Option<&'static str>);

    /// RULE: `df` reports the size of the task's ephemeral storage, in the unit the
    /// invocation asks for, and reports nothing for a task that holds no such size.
    #[test]
    fn the_storage_report_answers_the_tasks_ephemeral_storage() {
        let rows: &[StorageRow] = &[
            ("the Fargate default", Some(20), &[], Some("20971520")),
            ("an expanded size", Some(50), &["-k"], Some("52428800")),
            ("the POSIX form", Some(50), &["-P"], Some("52428800")),
            ("mebibytes", Some(50), &["-m"], Some("51200")),
            ("the largest unit it fits", Some(50), &["-h"], Some("50.0G")),
            ("the maximum", Some(200), &["-k"], Some("209715200")),
            ("a path on the layer", Some(21), &["-k", "/tmp"], Some("22020096")),
            ("a task holding no size", None, &["-k"], None),
            ("an option it does not read", Some(50), &["-i"], None),
        ];
        for (case, storage, form, expected) in rows {
            let process = ContainerProcess::of_settings(None, *storage, true);
            let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
            let written = process.storage_report(&arguments, 0, false);
            match expected {
                Some(size) => {
                    let report = written
                        .unwrap_or_else(|| panic!("case {case:?}: expected a report, got nothing"));
                    let row = report.lines().nth(1).unwrap_or_default();
                    let fields: Vec<&str> = row.split_whitespace().collect();
                    assert_eq!(
                        fields.first().copied(),
                        Some(CONTAINER_LAYER_DEVICE),
                        "case {case:?}: expected the layer's device, got {report:?}"
                    );
                    assert_eq!(
                        fields.get(1).copied(),
                        Some(*size),
                        "case {case:?}: expected a size of {size}, got {report:?}"
                    );
                    assert_eq!(
                        fields.last().copied(),
                        Some(CONTAINER_LAYER_MOUNT_POINT),
                        "case {case:?}: expected the layer's mount point, got {report:?}"
                    );
                }
                None => assert!(
                    written.is_none(),
                    "case {case:?}: expected no report, got {written:?}"
                ),
            }
        }
    }

    /// RULE: what the container's layer holds is what `df` reports as used, and a path
    /// inside a mounted volume is on another filesystem, which this reports nothing for.
    #[test]
    fn the_storage_report_answers_what_the_layer_holds() {
        let process = ContainerProcess::of_settings(None, Some(20), true);
        // (case, the bytes held, the used column, the capacity column)
        let rows: &[(&str, u64, &str, &str)] = &[
            ("an empty layer", 0, "0", "0%"),
            ("one byte", 1, "1", "1%"),
            ("a kibibyte", 1024, "1", "1%"),
            ("a mebibyte", 1024 * 1024, "1024", "1%"),
        ];
        for (case, held, used, capacity) in rows {
            let report = process
                .storage_report(&[], *held, false)
                .unwrap_or_else(|| panic!("case {case:?}: expected a report, got nothing"));
            let row = report.lines().nth(1).unwrap_or_default();
            let fields: Vec<&str> = row.split_whitespace().collect();
            assert_eq!(
                fields.get(2).copied(),
                Some(*used),
                "case {case:?}: expected {used} used, got {report:?}"
            );
            assert_eq!(
                fields.get(4).copied(),
                Some(*capacity),
                "case {case:?}: expected a capacity of {capacity}, got {report:?}"
            );
        }
        assert!(
            process.storage_report(&[], 0, true).is_none(),
            "a path inside a mounted volume is on a filesystem this reports nothing for"
        );
    }

    /// RULE: `pwd` writes the working directory the container's definition runs its commands
    /// in, and nothing for a form this session does not perform.
    #[test]
    fn the_working_directory_report_writes_the_directory() {
        for form in [vec![], vec!["-P"], vec!["-L"]] {
            let arguments: Vec<String> = form.iter().map(|word| word.to_string()).collect();
            let written = working_directory_report(&arguments, "/srv/app");
            assert_eq!(
                written.as_deref(),
                Some("/srv/app\n"),
                "case {form:?}: expected the working directory, got {written:?}"
            );
        }
        let declined = working_directory_report(&["/tmp".to_string()], "/srv/app");
        assert!(
            declined.is_none(),
            "an operand is a form this session does not perform, got {declined:?}"
        );
    }
}

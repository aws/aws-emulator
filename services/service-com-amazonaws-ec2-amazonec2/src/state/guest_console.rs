// SPDX-License-Identifier: CC0-1.0

//! What ONE boot of one instance's guest writes to that instance's CONSOLE.
//!
//! "For Linux instances, the instance console output displays the exact console output that
//! would normally be displayed on a physical monitor attached to a computer"
//! (`GetConsoleOutput`). So what a client reads there is what the GUEST wrote while it came
//! up: the kernel's own messages, each carrying the printk timestamp a kernel prefixes a
//! line with, then the init system's, then the output of the instance's own user data, which
//! the init system RUNS ([`super::guest_user_data`]).
//!
//! # What is real here and what is rendered
//!
//! The user-data program is PERFORMED: its commands run in the one bounded, in-process shell
//! session this workspace performs caller-supplied shell text with
//! (`service_common::posix_shell_session`), against this guest's own tree
//! (`service_common::posix_shell_volume`) — the files the machine keeps across its boots,
//! the identity the kernel publishes to it (`/proc/sys/kernel/random/boot_id`, this boot's
//! own) and its console device. A line the program writes to `/dev/console` therefore
//! appears on the console exactly as the program wrote it, a file it writes is still there
//! on the next boot, and a value it reads out of the guest is this emulator's own state
//! rather than anything echoed from a request.
//!
//! This emulator runs no kernel and no init system, so the lines AROUND that program are
//! RENDERED: the skeleton is the fixed sequence a Linux guest on this platform writes
//! ([`kernel_lines`], [`init_lines`]), and every VALUE in it is read off the stored instance
//! — its type, its architecture, its addresses, its image, its root volume, this boot's
//! identity and the instant it came up. Nothing is written for a fact the record does not
//! carry.
//!
//! # Written once, when the boot happened
//!
//! A console is a BUFFER: the guest wrote it when it booted. So these lines are computed
//! once, by the boot that wrote them ([`super::ComAmazonawsEc2Amazonec2State::record_instance_boot`]),
//! and kept on that boot's record — which is what makes reading a console a read (two reads
//! answer the same bytes, and the user-data program runs once per boot rather than once per
//! reader), and what makes the earlier boots' output still there, unchanged, after a reboot.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_common::posix_shell_session::{
    Bounds, OutputLimits, SessionConfig, ShellSession,
};
use ::service_common::posix_shell_volume::{ShellVolume, VolumeLimits};

use super::guest_user_data::GuestProgram;
use super::{iso8601_utc, ComAmazonawsEc2Amazonec2State, InstanceRecord};

/// The console device a guest writes a line to when it wants that line on the console.
pub const GUEST_CONSOLE_DEVICE: &str = "/dev/console";

/// The device that discards what is written to it.
pub const GUEST_NULL_DEVICE: &str = "/dev/null";

/// The path the kernel publishes this BOOT's own identity at, which is where a guest reads
/// "which boot am I" from.
pub const GUEST_BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

/// The path a guest reads its own machine identity from — stable across this instance's
/// boots, where the boot id is not.
pub const GUEST_MACHINE_ID_PATH: &str = "/etc/machine-id";

/// The path a guest reads its own host name from.
pub const GUEST_HOSTNAME_PATH: &str = "/etc/hostname";

/// The path the kernel publishes the command line it was booted with at.
pub const GUEST_CMDLINE_PATH: &str = "/proc/cmdline";

/// The path cloud-init publishes the instance's own id to its guest at.
pub const GUEST_INSTANCE_ID_PATH: &str = "/var/lib/cloud/data/instance-id";

/// The directories a guest of this platform already has, so a program that writes into one
/// of them does not have to make it first.
pub const GUEST_DIRECTORIES: &[&str] = &[
    "/bin",
    "/etc",
    "/home/ec2-user",
    "/opt",
    "/root",
    "/run",
    "/tmp",
    "/usr/bin",
    "/var/lib",
    "/var/lib/cloud/data",
    "/var/log",
    "/var/tmp",
];

/// The kernel release a guest of this emulator runs, where its image's own name does not
/// name one ([`image_kernel_release`]).
///
/// CHOSEN: no EC2 API publishes a guest's kernel release, and this emulator runs no kernel,
/// so the value is this emulator's own rather than a reproduction of an AMI's.
pub const DEFAULT_KERNEL_RELEASE: &str = "6.1.0";

/// The init system version the rendered init lines report. CHOSEN, for the same reason
/// [`DEFAULT_KERNEL_RELEASE`] is.
pub const INIT_SYSTEM_VERSION: &str = "252";

/// The cloud-init version the rendered init lines report. CHOSEN, for the same reason
/// [`DEFAULT_KERNEL_RELEASE`] is.
pub const CLOUD_INIT_VERSION: &str = "23.4";

/// The prefix every line cloud-init writes to a console carries, which is what a client
/// greps a console for the init system's own output by.
pub const CLOUD_INIT_PREFIX: &str = "cloud-init";

/// The prefix the lines carrying one instance FACT are written under, as cloud-init writes
/// the facts it discovered about the machine it is on.
pub const CI_INFO_PREFIX: &str = "ci-info:";

/// How many lines ONE boot keeps on the console, the user-data output included. A console is
/// a fixed-size buffer, so a boot that wrote more is kept up to this many.
pub const MAX_BOOT_CONSOLE_LINES: usize = 128;

/// How many bytes one of those lines may carry, as the device itself bounds them.
pub const MAX_BOOT_CONSOLE_LINE_BYTES: usize = 512;

/// How many files one guest keeps across its boots, and how many bytes they come to: the
/// machine's own storage, bounded because what writes it is caller-supplied text.
pub const MAX_GUEST_FILES: usize = 64;

/// See [`MAX_GUEST_FILES`].
pub const MAX_GUEST_FILE_BYTES: usize = 64 * 1024;

/// How many commands ONE boot's program performs before the session is given up on.
///
/// The bound on the work this run costs, and it matters where this runs: a boot is
/// materialised with the EC2 store's guard held ([`ComAmazonawsEc2Amazonec2State::record_instance_boot`]),
/// so what a caller-supplied program can make one boot cost is what every other caller of
/// that store waits for. It is well above any boot script a guest carries — the commands a
/// document can even name are already bounded by
/// [`super::guest_user_data::MAX_GUEST_COMMANDS`] — and far below what would cost this
/// process anything noticeable.
pub const MAX_BOOT_PROGRAM_COMMANDS: usize = 256;

/// What one instance's guest knows about the machine it runs on: the facts the console's
/// rendered lines report, each read off the stored instance.
#[derive(Debug, Clone)]
pub struct GuestMachine {
    /// The instance the guest is of.
    pub instance_id: String,
    /// Its instance type, which the firmware line reports.
    pub instance_type: String,
    /// The image it was launched from, for an instance whose record carries one.
    pub image_id: Option<String>,
    /// Its processor architecture, which the kernel reports.
    pub architecture: String,
    /// The Availability Zone it is in.
    pub availability_zone: String,
    /// Its private address, which the network driver's line reports.
    pub private_ip: String,
    /// Its private DNS name, which the init system sets the host name to.
    pub private_dns_name: String,
    /// Its public address, for an instance that has one.
    pub public_ip: Option<String>,
    /// When the instance was launched, which a reboot does not change.
    pub launch_time: SystemTime,
    /// The kernel release the guest runs ([`image_kernel_release`]).
    pub kernel_release: String,
    /// The device its root volume is attached at, and that volume's size in GiB.
    pub root_device: Option<(String, i32)>,
    /// The vCPUs and the memory (in MiB) the instance type gives the guest, where the
    /// published catalog holds facts for that type.
    pub processor: Option<(i32, i64)>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// What the guest of `instance` knows about the machine it runs on — every value read
    /// off the stored instance, the account's image catalog and the published instance-type
    /// facts.
    pub fn guest_machine(&self, instance: &InstanceRecord) -> GuestMachine {
        GuestMachine {
            instance_id: instance.instance_id.clone(),
            instance_type: instance.instance_type.clone(),
            image_id: instance.image_id.clone(),
            architecture: instance.architecture.clone(),
            availability_zone: instance.placement_az.clone(),
            private_ip: instance.private_ip.clone(),
            private_dns_name: instance.private_dns_name.clone(),
            public_ip: instance.public_ip.clone(),
            launch_time: instance.launch_time,
            kernel_release: self.image_kernel_release(instance.image_id.as_deref()),
            root_device: self
                .launched_root_device(instance.image_id.as_deref())
                .and_then(|(device, ebs)| ebs.volume_size.map(|size| (device, size))),
            processor: super::instance_type_record(&instance.instance_type)
                .map(|record| (record.default_vcpus, record.memory_mib)),
        }
    }

    /// The kernel release a guest launched from `image_id` runs.
    ///
    /// Read off the IMAGE where its own name states one — an image name carrying a
    /// `kernel-<release>` part names the kernel line that image boots, which is how the
    /// published Amazon Linux names are written — and [`DEFAULT_KERNEL_RELEASE`] for every
    /// other image, including one the account cannot see.
    pub fn image_kernel_release(&self, image_id: Option<&str>) -> String {
        let named = image_id
            .and_then(|image_id| self.images.get(image_id))
            .and_then(|image| image.name.as_deref())
            .and_then(kernel_release_in);
        named.unwrap_or_else(|| DEFAULT_KERNEL_RELEASE.to_string())
    }
}

/// The kernel release `name` names, for an image name carrying a `kernel-<release>` part.
fn kernel_release_in(name: &str) -> Option<String> {
    let (_, after) = name.split_once("kernel-")?;
    let release: String = after
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let (major, minor) = release.trim_end_matches('.').split_once('.')?;
    if major.is_empty() || minor.is_empty() {
        return None;
    }
    // An image names the kernel LINE (`6.1`); the release a guest reports is a release of
    // that line.
    Some(format!("{major}.{minor}.0"))
}

/// One boot's console output, and the guest files that boot left behind.
#[derive(Debug, Clone, Default)]
pub struct GuestBoot {
    /// What the guest wrote to the console while it came up, in the order it wrote it.
    pub lines: Vec<String>,
    /// The files on the machine's own storage once that boot's program had run — what the
    /// next boot of this guest reads.
    pub files: BTreeMap<String, String>,
}

/// What boot number `number` of `machine`'s guest writes to the console, and the files it
/// leaves behind.
///
/// `files` are the ones the machine's storage already held (what earlier boots wrote),
/// `program` is what the instance's user data gives the guest to run, and `boot_id` and
/// `booted_at` are this boot's own identity and instant.
pub fn boot_console(
    machine: &GuestMachine,
    number: u32,
    boot_id: &str,
    booted_at: SystemTime,
    program: &GuestProgram,
    files: &BTreeMap<String, String>,
) -> GuestBoot {
    let mut clock = PrintkClock::default();
    let mut lines: Vec<String> = Vec::new();
    for text in kernel_lines(machine, boot_id) {
        lines.push(clock.stamped(&text));
    }
    for text in init_lines(machine, number, boot_id, booted_at) {
        lines.push(clock.stamped(&text));
    }
    // The init stage is where the commands of EVERY boot are run, as cloud-init runs its
    // `bootcmd` module; the final stage is where a first-boot command runs.
    let (performed, files) = run_guest_program(machine, boot_id, booted_at, number, program, files);
    lines.extend(performed.into_iter().map(|written| match written {
        GuestOutput::Console(line) => line,
        GuestOutput::InitStage(line) => clock.stamped(&cloud_init_line(machine, number, &line)),
    }));
    lines.push(clock.stamped(&cloud_init_line(
        machine,
        number,
        &format!(
            "Cloud-init v. {CLOUD_INIT_VERSION} finished at {}. Datasource DataSourceEc2Local. \
             Up {} seconds",
            iso8601_utc(booted_at),
            clock.seconds()
        ),
    )));
    // A console is a FIXED-SIZE buffer, and what it holds is the most recent output: a boot
    // that wrote more lines than it holds keeps the last of them and drops the earliest, the
    // way a console device drops what no longer fits.
    if lines.len() > MAX_BOOT_CONSOLE_LINES {
        lines.drain(..lines.len() - MAX_BOOT_CONSOLE_LINES);
    }
    GuestBoot { lines, files }
}

/// A line the guest's own program wrote while the boot was coming up.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GuestOutput {
    /// Written to the console device, so it appears on the console exactly as written.
    Console(String),
    /// Written to the program's standard output, which the init system logs — so it appears
    /// on the console as a line of that init system's.
    InitStage(String),
}

/// The printk timestamps one boot's lines carry: seconds since this boot, in the
/// `[    6.096338]` form a kernel writes them, advancing by a fixed step per line so one
/// boot's log always renders the same.
#[derive(Debug, Default)]
struct PrintkClock {
    /// Microseconds since the boot.
    micros: u64,
    /// How many lines have been stamped, which is what varies the step.
    stamped: u64,
}

impl PrintkClock {
    /// The step between the line `index` and the one before it, in microseconds: a fixed
    /// sequence, so the same boot renders the same stamps rather than the host's clock
    /// leaking into a console.
    fn step(index: u64) -> u64 {
        4_231 + (index.wrapping_mul(1_117) % 97_000)
    }

    /// `text` with this line's printk timestamp before it.
    fn stamped(&mut self, text: &str) -> String {
        let stamp = format!(
            "[{:5}.{:06}]",
            self.micros / 1_000_000,
            self.micros % 1_000_000
        );
        self.micros = self.micros.saturating_add(Self::step(self.stamped));
        self.stamped = self.stamped.saturating_add(1);
        format!("{stamp} {text}")
    }

    /// How far into the boot the next line is, as the init system reports it ("Up 6.78
    /// seconds").
    fn seconds(&self) -> String {
        format!("{}.{:02}", self.micros / 1_000_000, (self.micros % 1_000_000) / 10_000)
    }
}

/// The KERNEL's own lines of one boot, without their printk timestamps: the fixed sequence a
/// Linux guest on this platform writes, with every value read off the machine.
fn kernel_lines(machine: &GuestMachine, boot_id: &str) -> Vec<String> {
    let release = &machine.kernel_release;
    let mut lines = vec![
        format!(
            "Linux version {release} (builder@emulator) #1 SMP PREEMPT_DYNAMIC {}",
            iso8601_utc(machine.launch_time)
        ),
        format!("Command line: {}", guest_command_line(machine)),
        format!("DMI: Amazon EC2 {}/, BIOS 1.0 10/16/2017", machine.instance_type),
        "Hypervisor detected: KVM".to_string(),
        "kvm-clock: cpu 0, msr 0:1ffe001, primary cpu clock".to_string(),
        "clocksource: kvm-clock: mask: 0xffffffffffffffff".to_string(),
        format!("Booting {} kernel on KVM", machine.architecture),
    ];
    if let Some((vcpus, memory_mib)) = machine.processor {
        let kilobytes = memory_mib.saturating_mul(1024);
        lines.push(format!(
            "Memory: {kilobytes}K/{kilobytes}K available ({memory_mib} MiB total)"
        ));
        lines.push(format!("smpboot: Allowing {vcpus} CPUs, 0 hotplug CPUs"));
        lines.push(format!("smp: Brought up 1 node, {vcpus} CPU(s)"));
    }
    lines.extend([
        "ACPI: Core revision 20220331".to_string(),
        "NET: Registered PF_INET protocol family".to_string(),
        "random: crng init done".to_string(),
        format!("Boot ID: {boot_id}"),
        "pci 0000:00:04.0: [1d0f:8061] type 00 class 0x010802".to_string(),
        "nvme nvme0: pci function 0000:00:04.0".to_string(),
    ]);
    if let Some((device, size)) = &machine.root_device {
        let node = device.rsplit('/').next().unwrap_or(device);
        lines.push(format!(
            "nvme0n1: {size}GiB root volume attached as {device} ({node})"
        ));
        lines.push(format!(
            "EXT4-fs (nvme0n1p1): mounted filesystem with ordered data mode as the root \
             filesystem of {device}"
        ));
    }
    lines.extend([
        "ena 0000:00:05.0: ENA device version: 0.10".to_string(),
        format!(
            "ena 0000:00:05.0 eth0: Local MAC address {}",
            guest_mac_address(machine)
        ),
        format!("IPv4: eth0: address {} link becomes ready", machine.private_ip),
        format!(
            "EXT4-fs: mounted root, zone {}",
            machine.availability_zone
        ),
        "Run /init as init process".to_string(),
    ]);
    lines
}

/// The INIT system's lines of one boot, without their printk timestamps — the init system's
/// own, then cloud-init's stage lines and the facts it discovered about this machine.
fn init_lines(
    machine: &GuestMachine,
    number: u32,
    boot_id: &str,
    booted_at: SystemTime,
) -> Vec<String> {
    let mut lines = vec![
        format!("systemd[1]: systemd {INIT_SYSTEM_VERSION} running in system mode"),
        "systemd[1]: Detected virtualization amazon.".to_string(),
        format!("systemd[1]: Detected architecture {}.", machine.architecture),
        format!(
            "systemd[1]: Hostname set to <{}>.",
            machine.private_dns_name
        ),
        "systemd[1]: Reached target Basic System.".to_string(),
        cloud_init_line(
            machine,
            number,
            &format!(
                "Cloud-init v. {CLOUD_INIT_VERSION} running 'init-local' at {}",
                iso8601_utc(booted_at)
            ),
        ),
        cloud_init_line(
            machine,
            number,
            &format!(
                "Cloud-init v. {CLOUD_INIT_VERSION} running 'init' at {}",
                iso8601_utc(booted_at)
            ),
        ),
    ];
    // The facts cloud-init discovered about the machine it is on, each read off the stored
    // instance: an instance the record carries no value for gets no line at all.
    let mut facts: Vec<(&str, String)> = vec![
        ("instance-id", machine.instance_id.clone()),
        ("instance-type", machine.instance_type.clone()),
    ];
    if let Some(image_id) = &machine.image_id {
        facts.push(("image-id", image_id.clone()));
    }
    facts.extend([
        ("availability-zone", machine.availability_zone.clone()),
        ("private-ip-address", machine.private_ip.clone()),
        ("private-dns-name", machine.private_dns_name.clone()),
    ]);
    if let Some(public_ip) = &machine.public_ip {
        facts.push(("public-ip-address", public_ip.clone()));
    }
    facts.extend([
        ("launch-time", iso8601_utc(machine.launch_time)),
        ("boot", number.to_string()),
        ("boot-id", boot_id.to_string()),
        ("boot-time", iso8601_utc(booted_at)),
    ]);
    for (name, value) in facts {
        lines.push(cloud_init_line(
            machine,
            number,
            &format!("{CI_INFO_PREFIX} {name}: {value}"),
        ));
    }
    lines.push(cloud_init_line(
        machine,
        number,
        &format!(
            "Cloud-init v. {CLOUD_INIT_VERSION} running 'modules:config' at {}",
            iso8601_utc(booted_at)
        ),
    ));
    lines
}

/// `text` as the line cloud-init writes it: its own name, the process it ran as in this
/// boot, and the text.
fn cloud_init_line(machine: &GuestMachine, number: u32, text: &str) -> String {
    format!("{CLOUD_INIT_PREFIX}[{}]: {text}", cloud_init_pid(machine, number))
}

/// The process id cloud-init ran as in boot `number` of this machine.
///
/// A guest's process ids are the guest's own and differ per boot, so this is DERIVED from
/// the machine and the boot rather than taken from a request: the same boot of the same
/// instance always reports the same one, and two boots report different ones.
fn cloud_init_pid(machine: &GuestMachine, number: u32) -> u32 {
    let mixed = ::service_common::stable_hex_id(&[
        machine.instance_id.as_str(),
        number.to_string().as_str(),
    ]);
    // Read out of the mixed hex without indexing it: the mixer's width is not this
    // function's to assume, so a shorter value answers the lowest pid rather than panicking.
    let value = mixed
        .get(..6)
        .and_then(|head| u32::from_str_radix(head, 16).ok())
        .unwrap_or(0);
    // The init system holds the low pids, so a guest's own services run above them.
    1_000 + value % 30_000
}

/// The command line the kernel of this guest reports being booted with.
fn guest_command_line(machine: &GuestMachine) -> String {
    let device = machine
        .root_device
        .as_ref()
        .map_or("/dev/xvda1".to_string(), |(device, _)| device.clone());
    format!(
        "BOOT_IMAGE=/boot/vmlinuz-{} root=UUID={} rootwait ro console=tty0 \
         console=ttyS0,115200n8 nvme_core.io_timeout=4294967295 root_device={device}",
        machine.kernel_release,
        stable_uuid(&["root-filesystem", &machine.instance_id])
    )
}

/// The MAC address of this machine's primary network interface, derived from its own private
/// address — a locally administered address, which is what a virtualised interface carries.
fn guest_mac_address(machine: &GuestMachine) -> String {
    let mixed = ::service_common::stable_hex_id(&["mac", &machine.private_ip]);
    // Five pairs read out of the mixed hex without indexing it, so a shorter value is
    // rendered short rather than panicking.
    let pairs: Vec<&str> = (0..5)
        .filter_map(|index| mixed.get(index * 2..index * 2 + 2))
        .collect();
    format!("02:{}", pairs.join(":"))
}

/// A UUID derived from `parts`: the form a filesystem or a machine identity carries, mixed
/// by the shared deterministic mixer and rendered by the shared UUID writer, so the same
/// machine always reports the same one.
fn stable_uuid(parts: &[&str]) -> String {
    let first = ::service_common::stable_hex_id(parts);
    let mut tail: Vec<&str> = parts.to_vec();
    tail.push("uuid");
    let second = ::service_common::stable_hex_id(&tail);
    let mut bytes = [0_u8; 16];
    let decoded = ::hex::decode(format!("{first}{second}")).unwrap_or_default();
    for (slot, byte) in bytes.iter_mut().zip(decoded) {
        *slot = byte;
    }
    ::service_common::uuid_from_random_bytes(bytes)
}

/// This machine's own identity, stable across its boots: 32 hexadecimal characters, which is
/// the form a guest reads at [`GUEST_MACHINE_ID_PATH`].
fn guest_machine_id(machine: &GuestMachine) -> String {
    let first = ::service_common::stable_hex_id(&["machine-id", &machine.instance_id]);
    let second = ::service_common::stable_hex_id(&["machine-id", &machine.instance_id, "tail"]);
    format!("{first}{second}")
}

/// Perform the guest's own program for boot number `number`, answering what it wrote and the
/// files it left on the machine's storage.
///
/// The program runs in the shared bounded shell session, in a tree holding exactly this
/// machine's storage, the values the kernel publishes to its guest and its two devices —
/// there is no host path, no process and no socket in it. A command the session does not
/// perform writes nothing and leaves the status behind it, the way it does on a machine
/// whose shell has no such program.
fn run_guest_program(
    machine: &GuestMachine,
    boot_id: &str,
    booted_at: SystemTime,
    number: u32,
    program: &GuestProgram,
    files: &BTreeMap<String, String>,
) -> (Vec<GuestOutput>, BTreeMap<String, String>) {
    let mut volume = ShellVolume::new(VolumeLimits {
        files: MAX_GUEST_FILES,
        bytes: MAX_GUEST_FILE_BYTES,
        device_lines: MAX_BOOT_CONSOLE_LINES,
        device_line_bytes: MAX_BOOT_CONSOLE_LINE_BYTES,
        ..VolumeLimits::default()
    });
    for directory in GUEST_DIRECTORIES {
        volume.make_directories(directory);
    }
    for (path, contents) in files {
        volume.put_file(path, contents);
    }
    // What the kernel and the init system publish to this guest: readable by its program,
    // never writable by it.
    volume.seal_file(GUEST_BOOT_ID_PATH, &format!("{boot_id}\n"));
    volume.seal_file(GUEST_MACHINE_ID_PATH, &format!("{}\n", guest_machine_id(machine)));
    volume.seal_file(GUEST_HOSTNAME_PATH, &format!("{}\n", machine.private_dns_name));
    volume.seal_file(GUEST_CMDLINE_PATH, &format!("{}\n", guest_command_line(machine)));
    volume.seal_file(GUEST_INSTANCE_ID_PATH, &format!("{}\n", machine.instance_id));
    volume.add_keeping_device(GUEST_CONSOLE_DEVICE);
    volume.add_discarding_device(GUEST_NULL_DEVICE);

    let environment = BTreeMap::from([
        (
            "PATH".to_string(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ),
        ("HOME".to_string(), "/root".to_string()),
        ("USER".to_string(), "root".to_string()),
        ("SHELL".to_string(), "/bin/sh".to_string()),
        ("HOSTNAME".to_string(), machine.private_dns_name.clone()),
        (
            "CLOUD_INIT_BOOT".to_string(),
            number.to_string(),
        ),
    ]);
    let mut session = ShellSession::in_volume(
        environment,
        SessionConfig {
            bounds: Bounds {
                // The bound on the work one boot costs the store's guard; every other bound
                // of a session is the shared one.
                commands: MAX_BOOT_PROGRAM_COMMANDS,
                ..Bounds::default()
            },
            output: Some(OutputLimits {
                lines: MAX_BOOT_CONSOLE_LINES,
                line_bytes: MAX_BOOT_CONSOLE_LINE_BYTES,
            }),
            ..SessionConfig::default()
        },
        volume,
    );
    // The instant the boot happened is the console's, not the program's: a guest's own clock
    // is not something a boot script reads here.
    let _ = booted_at;
    for command in program.commands_for_boot(number) {
        session.run(command);
        if session.exit_status().is_some() {
            break;
        }
    }
    let logged: Vec<String> = session.output().to_vec();
    let volume = session.into_volume().unwrap_or_default();
    let mut written: Vec<GuestOutput> = volume
        .device_lines(GUEST_CONSOLE_DEVICE)
        .iter()
        .map(|line| GuestOutput::Console(line.clone()))
        .collect();
    written.extend(logged.into_iter().map(GuestOutput::InitStage));
    let files = volume
        .stored_files()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect();
    (written, files)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime};

    use super::super::guest_user_data::GuestProgram;
    use super::{boot_console, kernel_release_in, GuestMachine, CLOUD_INIT_PREFIX};

    /// A machine of the shape a launch stores.
    fn machine() -> GuestMachine {
        GuestMachine {
            instance_id: "i-0123456789abcdef0".to_string(),
            instance_type: "t3.micro".to_string(),
            image_id: Some("ami-0123456789abcdef0".to_string()),
            architecture: "x86_64".to_string(),
            availability_zone: "us-west-2a".to_string(),
            private_ip: "10.0.1.23".to_string(),
            private_dns_name: "ip-10-0-1-23.us-west-2.compute.internal".to_string(),
            public_ip: Some("54.1.2.3".to_string()),
            launch_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            kernel_release: "6.1.0".to_string(),
            root_device: Some(("/dev/xvda".to_string(), 8)),
            processor: Some((2, 1024)),
        }
    }

    /// RULE: a boot's console carries the guest's own log — printk-timestamped kernel lines
    /// and init-system lines — whatever the instance's user data says.
    #[test]
    fn a_boot_writes_a_guest_s_own_log() {
        let booted = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060);
        let boot = boot_console(
            &machine(),
            1,
            "6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30",
            booted,
            &GuestProgram::default(),
            &BTreeMap::new(),
        );
        let stamped = boot
            .lines
            .iter()
            .filter(|line| printk_stamped(line))
            .count();
        assert!(
            stamped >= 20,
            "expected at least 20 printk-timestamped lines, got {stamped} of {:?}",
            boot.lines
        );
        assert!(
            boot.lines
                .iter()
                .any(|line| line.contains(&format!("{CLOUD_INIT_PREFIX}["))),
            "expected the init system's own lines, got {:?}",
            boot.lines
        );
        assert!(
            boot.lines
                .iter()
                .any(|line| line.contains("instance-id: i-0123456789abcdef0")),
            "expected the machine's own facts, got {:?}",
            boot.lines
        );
    }

    /// Whether `line` carries a printk timestamp, as a kernel writes one.
    fn printk_stamped(line: &str) -> bool {
        let Some(closed) = line.find(']') else {
            return false;
        };
        let Some(stamp) = line.get(1..closed) else {
            return false;
        };
        let Some((seconds, micros)) = stamp.trim().split_once('.') else {
            return false;
        };
        line.starts_with('[')
            && seconds.chars().all(|c| c.is_ascii_digit())
            && micros.len() == 6
            && micros.chars().all(|c| c.is_ascii_digit())
    }

    /// RULE: the guest's own program is PERFORMED — what it writes to the console device is
    /// on the console as written, what it reads of the guest is this boot's own state, and
    /// what it stores stays on the machine for the next boot.
    #[test]
    fn the_guest_s_program_is_performed() {
        let program = GuestProgram {
            every_boot: vec![
                "C=$(cat /var/lib/count)".to_string(),
                "C=$((C + 1))".to_string(),
                "echo $C > /var/lib/count".to_string(),
                "echo MARKER nonce-1 $(cat /proc/sys/kernel/random/boot_id) $C > /dev/console"
                    .to_string(),
            ],
            first_boot: vec!["echo FIRST-BOOT-ONLY > /dev/console".to_string()],
        };
        let booted = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060);
        let first = boot_console(
            &machine(),
            1,
            "6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30",
            booted,
            &program,
            &BTreeMap::new(),
        );
        assert!(
            first.lines.iter().any(|line| line
                == "MARKER nonce-1 6e3f1f2a-0f4d-4b7f-9a2e-7c1d5e8b4a30 1"),
            "expected the program's console line as written, got {:?}",
            first.lines
        );        assert!(
            first
                .lines
                .iter()
                .any(|line| line == "FIRST-BOOT-ONLY"),
            "expected the first boot's own command to have run, got {:?}",
            first.lines
        );
        let second = boot_console(
            &machine(),
            2,
            "9a1b2c3d-4e5f-4a6b-8c7d-0e1f2a3b4c5d",
            booted + Duration::from_secs(30),
            &program,
            &first.files,
        );
        assert!(
            second.lines.iter().any(|line| line
                == "MARKER nonce-1 9a1b2c3d-4e5f-4a6b-8c7d-0e1f2a3b4c5d 2"),
            "expected the count kept on the machine and this boot's own identity, got {:?}",
            second.lines
        );
        assert!(
            !second.lines.iter().any(|line| line == "FIRST-BOOT-ONLY"),
            "expected a first-boot command not to run again, got {:?}",
            second.lines
        );
    }

    /// RULE: the kernel release is read off the image's own name where that name states one.
    #[test]
    fn the_kernel_release_comes_from_the_image_name() {
        // (case, the image name, the release a guest of it reports)
        let rows = [
            (
                "an Amazon Linux 2023 name",
                "al2023-ami-2023.6.20241111.0-kernel-6.1-x86_64",
                Some("6.1.0"),
            ),
            (
                "an Amazon Linux 2 name",
                "amzn2-ami-kernel-5.10-hvm-2.0.20241113.1-x86_64-gp2",
                Some("5.10.0"),
            ),
            ("a name stating no kernel", "my-own-image", None),
            ("a name whose kernel part is not a release", "ami-kernel-x", None),
        ];
        for (case, name, expected) in rows {
            let release = kernel_release_in(name);
            assert_eq!(
                release.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {release:?}"
            );
        }
    }
}

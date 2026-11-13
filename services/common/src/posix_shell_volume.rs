// SPDX-License-Identifier: CC0-1.0

//! The in-memory TREE a bounded shell session reads and writes, for the emulated services
//! whose caller-supplied program is a program of a GUEST.
//!
//! # Why a session needs one
//!
//! A program that runs on a machine writes to that machine: a boot script keeps a counter
//! on the root volume, writes a line to the console device and reads an identity the kernel
//! publishes. A session with nowhere to read or write cannot perform one of those lines at
//! all, so the service that holds the machine (an EC2 instance and its volumes) hands the
//! session this tree, which is the WHOLE of what its commands can reach. There is no host
//! path behind it: every entry is one this process holds in memory, every one of them was
//! put here by the service that owns the machine, and nothing here opens, resolves or lists
//! a path of the host.
//!
//! # What it holds
//!
//! * FILES the program may read and write, which is what the machine's own storage is —
//!   the caller takes them back out ([`ShellVolume::stored_files`]) and keeps them beside
//!   the machine, so what one run wrote the next run reads;
//! * SEALED files, which the service seeded and a program may only READ: the values a
//!   kernel or an agent publishes to a guest rather than values the guest chose;
//! * DEVICES, which a program may only WRITE: a sink that keeps the lines written to it
//!   ([`ShellVolume::device_lines`], which is how a console gets what a boot script wrote)
//!   or one that discards them, the way `/dev/null` does;
//! * DIRECTORIES, so a write to a path whose directory the machine does not have fails the
//!   way it fails on a machine rather than silently inventing the tree.
//!
//! # Bounds
//!
//! Every write is bounded by [`VolumeLimits`], held here rather than in the program's
//! reach: how many files the tree holds and how many bytes they come to, how long one path
//! may be, and how many lines and bytes one device keeps. A write past a bound does not
//! happen and is reported as the failure it is, so a caller-supplied program cannot grow
//! this process without a stated ceiling.

use std::collections::{BTreeMap, BTreeSet};

/// How much one volume may hold. Every field is the EMULATOR's number: the programs are
/// caller-supplied text, so what one of them can store is bounded here.
#[derive(Debug, Clone, Copy)]
pub struct VolumeLimits {
    /// How many files the tree holds at once, sealed ones included.
    pub files: usize,
    /// How many bytes those files' paths and contents come to at once.
    pub bytes: usize,
    /// The longest path this tree accepts.
    pub path_bytes: usize,
    /// How many directories the tree holds at once.
    pub directories: usize,
    /// How many lines ONE device keeps. Past it a write is refused rather than kept.
    pub device_lines: usize,
    /// How many bytes ONE line written to a device may carry. A longer line is truncated on
    /// a character boundary, so what the program wrote is still reported.
    pub device_line_bytes: usize,
}

impl Default for VolumeLimits {
    /// Bounds far above any boot script a guest carries and far below what would cost this
    /// process anything noticeable.
    fn default() -> Self {
        Self {
            files: 256,
            bytes: 256 * 1024,
            path_bytes: 1024,
            directories: 256,
            device_lines: 512,
            device_line_bytes: 4 * 1024,
        }
    }
}

/// What one DEVICE of the tree does with the lines written to it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Device {
    /// Keep them, up to the volume's bounds — a console, whose whole purpose is that
    /// something reads back what was written to it.
    Keep(Vec<String>),
    /// Discard them, which is the whole of what the null device does.
    Discard,
}

/// The tree one session's commands read and write. See the module docs.
#[derive(Debug, Clone)]
pub struct ShellVolume {
    /// The files a program may read and write, by absolute path.
    files: BTreeMap<String, String>,
    /// The paths of [`Self::files`] entries a program may only READ.
    sealed: BTreeSet<String>,
    /// The directories the tree has, by absolute path, always including the root.
    directories: BTreeSet<String>,
    /// The devices the tree has, by absolute path.
    devices: BTreeMap<String, Device>,
    /// How many bytes the files come to, kept as they are written rather than recomputed.
    held_bytes: usize,
    limits: VolumeLimits,
}

impl Default for ShellVolume {
    fn default() -> Self {
        Self::new(VolumeLimits::default())
    }
}

/// The directory every tree has, which every absolute path is resolved against.
pub const ROOT: &str = "/";

impl ShellVolume {
    /// An empty tree — the root directory and nothing else — bounded by `limits`.
    #[must_use]
    pub fn new(limits: VolumeLimits) -> Self {
        Self {
            files: BTreeMap::new(),
            sealed: BTreeSet::new(),
            directories: BTreeSet::from([ROOT.to_string()]),
            devices: BTreeMap::new(),
            held_bytes: 0,
            limits,
        }
    }

    /// `path` as this tree resolves it: absolute, with the empty, `.` and `..` segments a
    /// path may carry resolved, or `None` for a path this tree does not accept — a relative
    /// one (a session has no working directory of its own), or one past
    /// [`VolumeLimits::path_bytes`].
    ///
    /// `..` above the root resolves to the root, as it does on a machine.
    #[must_use]
    pub fn resolve(&self, path: &str) -> Option<String> {
        if path.len() > self.limits.path_bytes || !path.starts_with('/') {
            return None;
        }
        let mut segments: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                named => segments.push(named),
            }
        }
        if segments.is_empty() {
            return Some(ROOT.to_string());
        }
        Some(format!("/{}", segments.join("/")))
    }

    /// The directory `path` lies in, or `None` for the root itself.
    fn parent_of(path: &str) -> Option<String> {
        let (parent, _) = path.rsplit_once('/')?;
        if parent.is_empty() {
            return Some(ROOT.to_string());
        }
        Some(parent.to_string())
    }

    /// Seed a file a program may read and WRITE, with the directories above it — what the
    /// machine's storage already held when this run started.
    ///
    /// `false` when the tree is at its bounds and the file was not seeded.
    pub fn put_file(&mut self, path: &str, contents: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        self.make_directories_of(&resolved);
        self.write_resolved(&resolved, contents, false)
    }

    /// Seed a file a program may only READ: a value the machine publishes to its guest
    /// rather than one the guest chose. A write to it fails the way a write to a read-only
    /// file fails.
    ///
    /// `false` when the tree is at its bounds and the file was not seeded.
    pub fn seal_file(&mut self, path: &str, contents: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        self.make_directories_of(&resolved);
        if !self.write_resolved(&resolved, contents, false) {
            return false;
        }
        self.sealed.insert(resolved);
        true
    }

    /// Add a device that KEEPS what is written to it, so the caller reads those lines back
    /// ([`Self::device_lines`]).
    pub fn add_keeping_device(&mut self, path: &str) {
        if let Some(resolved) = self.resolve(path) {
            self.make_directories_of(&resolved);
            self.devices.insert(resolved, Device::Keep(Vec::new()));
        }
    }

    /// Add a device that DISCARDS what is written to it, which is the whole of what the
    /// null device does. Reading one answers nothing, as reading it does on a machine.
    pub fn add_discarding_device(&mut self, path: &str) {
        if let Some(resolved) = self.resolve(path) {
            self.make_directories_of(&resolved);
            self.devices.insert(resolved, Device::Discard);
        }
    }

    /// Make `path` a directory, with every directory above it — `mkdir -p`'s whole effect.
    ///
    /// `false` when the tree is at [`VolumeLimits::directories`], or when a FILE already
    /// holds one of those paths, which is what makes the command fail rather than replace
    /// it.
    pub fn make_directories(&mut self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        let mut built = String::new();
        let mut made: Vec<String> = Vec::new();
        for segment in resolved.split('/').filter(|segment| !segment.is_empty()) {
            built.push('/');
            built.push_str(segment);
            if self.files.contains_key(&built) || self.devices.contains_key(&built) {
                return false;
            }
            if !self.directories.contains(&built) {
                made.push(built.clone());
            }
        }
        if self.directories.len().saturating_add(made.len()) > self.limits.directories {
            return false;
        }
        self.directories.extend(made);
        true
    }

    /// The directories above `path`, made as far as the bounds allow. Used where a seed the
    /// SERVICE performs implies them: a machine that has the file has the directories.
    fn make_directories_of(&mut self, path: &str) {
        if let Some(parent) = Self::parent_of(path) {
            let _ = self.make_directories(&parent);
        }
    }

    /// What reading `path` answers: a file's contents, a sealed file's contents, or the
    /// empty text a discarding device answers — and `None` for a path the tree does not
    /// hold, a directory, or a device whose lines are kept (a console is written, not read).
    #[must_use]
    pub fn read(&self, path: &str) -> Option<String> {
        let resolved = self.resolve(path)?;
        if let Some(Device::Discard) = self.devices.get(&resolved) {
            return Some(String::new());
        }
        self.files.get(&resolved).cloned()
    }

    /// Whether the tree holds anything at `path` at all — a file, a directory or a device.
    #[must_use]
    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|resolved| {
            self.files.contains_key(&resolved)
                || self.directories.contains(&resolved)
                || self.devices.contains_key(&resolved)
        })
    }

    /// Whether `path` is a file of the tree (sealed or not).
    #[must_use]
    pub fn is_file(&self, path: &str) -> bool {
        self.resolve(path)
            .is_some_and(|resolved| self.files.contains_key(&resolved))
    }

    /// Whether `path` is a directory of the tree.
    #[must_use]
    pub fn is_directory(&self, path: &str) -> bool {
        self.resolve(path)
            .is_some_and(|resolved| self.directories.contains(&resolved))
    }

    /// Write `contents` at `path`, replacing what is there or APPENDING to it.
    ///
    /// `false` is a write that did not happen, which is what the command performing it
    /// reports as its own failure: a path this tree does not accept, a sealed file, a
    /// directory, a path whose directory the tree does not have, or a write past
    /// [`VolumeLimits::files`] or [`VolumeLimits::bytes`]. A device takes the write as the
    /// lines it was given, up to [`VolumeLimits::device_lines`].
    pub fn write(&mut self, path: &str, contents: &str, append: bool) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.sealed.contains(&resolved) || self.directories.contains(&resolved) {
            return false;
        }
        if self.devices.contains_key(&resolved) {
            return self.write_device(&resolved, contents);
        }
        // A machine will not create a file in a directory it does not have.
        if Self::parent_of(&resolved).is_some_and(|parent| !self.directories.contains(&parent)) {
            return false;
        }
        self.write_resolved(&resolved, contents, append)
    }

    /// [`Self::write`] once the path is resolved and known to name a file.
    fn write_resolved(&mut self, resolved: &str, contents: &str, append: bool) -> bool {
        let held = self.files.get(resolved);
        if held.is_none() && self.files.len() >= self.limits.files {
            return false;
        }
        let kept = held.map_or(0, String::len);
        let addition = if append {
            contents.len()
        } else {
            contents.len().saturating_sub(kept)
        };
        let path_bytes = if held.is_none() { resolved.len() } else { 0 };
        let would_hold = if append {
            self.held_bytes.saturating_add(addition)
        } else {
            self.held_bytes
                .saturating_sub(kept)
                .saturating_add(contents.len())
        }
        .saturating_add(path_bytes);
        if would_hold > self.limits.bytes {
            return false;
        }
        self.held_bytes = would_hold;
        match self.files.get_mut(resolved) {
            Some(file) if append => file.push_str(contents),
            Some(file) => *file = contents.to_string(),
            None => {
                self.files.insert(resolved.to_string(), contents.to_string());
            }
        }
        true
    }

    /// Write to a DEVICE: one line per line of `contents`, kept or discarded as the device
    /// says. A line past [`VolumeLimits::device_line_bytes`] is truncated on a character
    /// boundary; a write past [`VolumeLimits::device_lines`] does not happen.
    fn write_device(&mut self, resolved: &str, contents: &str) -> bool {
        let limits = self.limits;
        let Some(device) = self.devices.get_mut(resolved) else {
            return false;
        };
        let Device::Keep(lines) = device else {
            return true;
        };
        // The device takes whole lines: a write with no newline of its own is still one
        // line, which is what a console shows for it.
        let written: Vec<&str> = contents.split_inclusive('\n').collect();
        if lines.len().saturating_add(written.len()) > limits.device_lines {
            return false;
        }
        for line in written {
            let mut line = line.trim_end_matches('\n').to_string();
            if line.len() > limits.device_line_bytes {
                let mut cut = limits.device_line_bytes;
                while cut > 0 && !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.truncate(cut);
            }
            lines.push(line);
        }
        true
    }

    /// Remove the file at `path`, answering whether there was one to remove. A sealed file,
    /// a directory and a device are not removed: a program does not take away what the
    /// machine published to it.
    pub fn remove_file(&mut self, path: &str) -> bool {
        let Some(resolved) = self.resolve(path) else {
            return false;
        };
        if self.sealed.contains(&resolved) {
            return false;
        }
        match self.files.remove(&resolved) {
            Some(contents) => {
                self.held_bytes = self
                    .held_bytes
                    .saturating_sub(contents.len().saturating_add(resolved.len()));
                true
            }
            None => false,
        }
    }

    /// The lines written to the device at `path`, in the order they were written — empty for
    /// a path that is not a keeping device.
    #[must_use]
    pub fn device_lines(&self, path: &str) -> &[String] {
        match self.resolve(path).and_then(|resolved| {
            self.devices.get(&resolved).map(|device| match device {
                Device::Keep(lines) => lines.as_slice(),
                Device::Discard => &[],
            })
        }) {
            Some(lines) => lines,
            None => &[],
        }
    }

    /// The files a program may write, as `(path, contents)` — what the caller keeps beside
    /// the machine so the next run of a program on it reads what this one wrote. The sealed
    /// files are left out: they are the service's own seed, not the machine's storage.
    pub fn stored_files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.files
            .iter()
            .filter(|(path, _)| !self.sealed.contains(*path))
            .map(|(path, contents)| (path.as_str(), contents.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::{ShellVolume, VolumeLimits};

    /// RULE: a path is resolved the way a machine resolves one — absolute, with empty, `.`
    /// and `..` segments resolved and `..` above the root staying at the root — and a
    /// relative path is one this tree does not accept at all.
    #[test]
    fn a_path_is_resolved_the_way_a_machine_resolves_one() {
        let volume = ShellVolume::default();
        // (case, the path written, what it resolves to)
        let rows = [
            ("already resolved", "/var/lib/counter", Some("/var/lib/counter")),
            ("a doubled separator", "/var//lib///counter", Some("/var/lib/counter")),
            ("a here segment", "/var/./lib/counter", Some("/var/lib/counter")),
            ("an up segment", "/var/lib/../counter", Some("/var/counter")),
            ("up above the root", "/../../counter", Some("/counter")),
            ("the root itself", "/", Some("/")),
            ("only up segments", "/..", Some("/")),
            ("a relative path", "var/lib/counter", None),
            ("an empty path", "", None),
        ];
        for (case, path, expected) in rows {
            let resolved = volume.resolve(path);
            assert_eq!(
                resolved.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: what a program may do to an entry is what the entry IS — a file it may read and
    /// write, a sealed file it may only read, a keeping device it may only write, a
    /// discarding device that answers nothing — so no write replaces what the service
    /// published to the guest.
    #[test]
    fn what_a_program_may_do_to_an_entry_is_what_the_entry_is() {
        let mut volume = ShellVolume::default();
        volume.put_file("/var/lib/counter", "1");
        volume.seal_file("/proc/sys/kernel/random/boot_id", "a-boot-id");
        volume.add_keeping_device("/dev/console");
        volume.add_discarding_device("/dev/null");

        // (case, path, may be read to, may be written)
        let rows = [
            ("a stored file", "/var/lib/counter", Some("1"), true),
            (
                "a sealed file",
                "/proc/sys/kernel/random/boot_id",
                Some("a-boot-id"),
                false,
            ),
            ("a keeping device", "/dev/console", None, true),
            ("a discarding device", "/dev/null", Some(""), true),
            ("a path the tree does not hold yet", "/var/lib/fresh", None, true),
            ("a directory", "/var/lib", None, false),
        ];
        for (case, path, readable, writable) in rows {
            let read = volume.read(path);
            assert_eq!(
                read.as_deref(),
                readable,
                "case {case:?}: expected a read of {readable:?}, got {read:?}"
            );
            let wrote = volume.write(path, "written\n", true);
            assert_eq!(
                wrote, writable,
                "case {case:?}: expected a write to answer {writable}, got {wrote}"
            );
        }
        assert_eq!(
            volume.read("/proc/sys/kernel/random/boot_id").as_deref(),
            Some("a-boot-id"),
            "the refused write left the sealed file as the service seeded it"
        );
        assert_eq!(
            volume.device_lines("/dev/console"),
            ["written"],
            "the device kept the line written to it, got {:?}",
            volume.device_lines("/dev/console")
        );
    }

    /// RULE: a write to a path whose directory the machine does not have fails, and `mkdir
    /// -p` is what makes it succeed.
    #[test]
    fn a_write_needs_the_directory_above_it() {
        let mut volume = ShellVolume::default();
        assert!(
            !volume.write("/var/lib/sim/counter", "1", false),
            "a write into a directory the tree does not have did not fail"
        );
        assert!(
            volume.make_directories("/var/lib/sim"),
            "mkdir -p did not make the directories"
        );
        assert!(
            volume.write("/var/lib/sim/counter", "1", false),
            "a write into the directory just made did not succeed"
        );
        assert_eq!(volume.read("/var/lib/sim/counter").as_deref(), Some("1"));
    }

    /// RULE: every bound is the volume's own, and a write past one does not happen.
    #[test]
    fn every_bound_refuses_the_write() {
        let limits = VolumeLimits {
            files: 2,
            bytes: 64,
            path_bytes: 32,
            directories: 4,
            device_lines: 2,
            device_line_bytes: 8,
        };
        let mut volume = ShellVolume::new(limits);
        assert!(volume.write("/one", "x", false), "the first file fits");
        assert!(volume.write("/two", "x", false), "the second file fits");
        assert!(
            !volume.write("/three", "x", false),
            "a file past the file count was still written"
        );
        assert!(
            !volume.write("/one", &"x".repeat(128), false),
            "a write past the byte bound was still performed"
        );
        assert!(
            !volume.write(&format!("/{}", "p".repeat(64)), "x", false),
            "a path past the path bound was still accepted"
        );
        volume.add_keeping_device("/dev/console");
        assert!(
            volume.write("/dev/console", "first\nsecond\n", true),
            "the device took the lines that fit"
        );
        assert!(
            !volume.write("/dev/console", "third\n", true),
            "a device write past the line bound was still kept"
        );
        assert_eq!(
            volume.device_lines("/dev/console").len(),
            2,
            "the device kept more lines than its bound allows"
        );
        let mut truncating = ShellVolume::new(limits);
        truncating.add_keeping_device("/dev/console");
        truncating.write("/dev/console", "0123456789abcdef\n", true);
        assert_eq!(
            truncating.device_lines("/dev/console"),
            ["01234567"],
            "a line past the device's line bound was not truncated, got {:?}",
            truncating.device_lines("/dev/console")
        );
    }

    /// RULE: the files a program wrote are what the caller takes back out, and the service's
    /// own sealed seed is not among them.
    #[test]
    fn the_files_a_program_wrote_come_back_out() {
        let mut volume = ShellVolume::default();
        volume.seal_file("/proc/sys/kernel/random/boot_id", "a-boot-id");
        volume.put_file("/var/lib/counter", "1");
        volume.write("/var/lib/counter", "2", false);
        volume.make_directories("/var/log");
        volume.write("/var/log/boot", "up\n", true);
        let stored: Vec<(&str, &str)> = volume.stored_files().collect();
        assert_eq!(
            stored,
            [("/var/lib/counter", "2"), ("/var/log/boot", "up\n")],
            "expected only the files the program may write, got {stored:?}"
        );
        assert!(
            volume.remove_file("/var/log/boot"),
            "a file the program wrote could not be removed"
        );
        assert!(
            !volume.remove_file("/proc/sys/kernel/random/boot_id"),
            "a sealed file was removed by the program"
        );
    }
}

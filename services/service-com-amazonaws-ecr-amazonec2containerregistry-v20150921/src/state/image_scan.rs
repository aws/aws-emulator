// SPDX-License-Identifier: CC0-1.0

use super::*;

/// What a basic image scan identifies in ONE layer's filesystem: the operating system the
/// layer's release file names, and the package manager whose installed-package database
/// it carries.
///
/// Those two are what basic scanning needs to report on an image — it scans the
/// operating-system packages of the image's filesystem — so they are what reading a
/// layer's bytes keeps, rather than the bytes. Both are absent for a layer that carries
/// neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerFilesystem {
    /// The operating system the layer's release file identifies, as that file names it
    /// (the `ID` of an `os-release` file, e.g. `alpine`), or `None` where the layer
    /// carries no release file.
    pub operating_system: Option<String>,
    /// The package manager whose installed-package database the layer carries (see
    /// [`PACKAGE_DATABASE_PATHS`]), or `None` where it carries none.
    pub package_manager: Option<String>,
}

impl LayerFilesystem {
    /// Whether this layer carries both of the things a basic scan needs, so that one
    /// layer alone makes an image scannable.
    pub(super) fn is_complete(&self) -> bool {
        self.operating_system.is_some() && self.package_manager.is_some()
    }

    /// Take over whatever `other` identified that this one has not: an image's
    /// filesystem is the union of its layers', so a release file in one layer and a
    /// package database in another identify the same image between them.
    pub(super) fn absorb(&mut self, other: &Self) {
        if self.operating_system.is_none() {
            self.operating_system = other.operating_system.clone();
        }
        if self.package_manager.is_none() {
            self.package_manager = other.package_manager.clone();
        }
    }
}

/// The paths an operating-system RELEASE FILE sits at inside an image's filesystem, each
/// with the operating system that file names when the file itself does not say.
///
/// The first two are the `os-release` file of the systemd/freedesktop os-release
/// specification, which every current distribution ships and which names the operating
/// system in its own `ID=` field (so there is nothing to supply for them); the rest are
/// the older per-distribution release files, whose NAME is the file name and whose
/// content is only a version.
pub(super) const OS_RELEASE_PATHS: &[(&str, Option<&str>)] = &[
    ("etc/os-release", None),
    ("usr/lib/os-release", None),
    ("etc/alpine-release", Some("alpine")),
    ("etc/debian_version", Some("debian")),
    ("etc/redhat-release", Some("rhel")),
    ("etc/centos-release", Some("centos")),
    ("etc/oracle-release", Some("ol")),
    ("etc/system-release", Some("amzn")),
    ("etc/SuSE-release", Some("suse")),
    ("etc/photon-release", Some("photon")),
];

/// Where a package manager keeps the database of the packages INSTALLED in an image's
/// filesystem, and the manager that keeps it there: apk (Alpine) keeps its installed set
/// in one file, dpkg (Debian, Ubuntu) in its status file, and rpm (Red Hat, Amazon Linux,
/// SUSE) keeps a database directory — at the traditional location, and at the one newer
/// distributions moved it to — so an entry ending in `/` matches everything under it.
///
/// A basic scan reports on the packages such a database lists, so an image whose
/// filesystem carries none is one it cannot report on.
pub(super) const PACKAGE_DATABASE_PATHS: &[(&str, &str)] = &[
    ("lib/apk/db/installed", "apk"),
    ("var/lib/dpkg/status", "dpkg"),
    ("var/lib/rpm/", "rpm"),
    ("usr/lib/sysimage/rpm/", "rpm"),
];

/// The most DECOMPRESSED bytes of one layer a scan reads before it stops looking.
///
/// A layer is caller-supplied and compressed, so its decompressed size is not bounded by
/// the bytes the client sent: this is the bound on the work one `CompleteLayerUpload`
/// can ask for. It is far above the filesystem of any ordinary base image, and a layer
/// that runs past it is read up to here rather than refused — what was identified in the
/// part that was read still stands.
pub(super) const MAX_SCANNED_LAYER_BYTES: u64 = 64 * 1024 * 1024;

/// The most archive ENTRIES of one layer a scan looks at, for the same reason as
/// [`MAX_SCANNED_LAYER_BYTES`]: a small archive can declare a very large number of
/// members.
pub(super) const MAX_SCANNED_LAYER_ENTRIES: usize = 100_000;

/// The most bytes of ONE release file a scan reads. A release file is a handful of
/// `KEY=value` lines; a member sitting at that path and carrying more than this states
/// its operating system in the part that is read, or in none of it.
pub(super) const MAX_RELEASE_FILE_BYTES: u64 = 64 * 1024;

/// Read the bytes of one registered layer for what a basic image scan identifies in it.
///
/// An image layer is a tar archive of the filesystem changes it contributes, gzip
/// compressed in both media types in common use (`...tar.gzip`, `...tar+gzip`) and plain
/// where the image declares an uncompressed layer, so the bytes are decompressed when
/// they carry the gzip magic and then walked as a tar: a release file in them names the
/// operating system, and a package manager's database in them is the installed-package
/// set the scan would report on. Bytes that are not a readable archive identify nothing,
/// which is the answer for anything that is not a filesystem layer at all.
///
/// Entry names are compared after their leading `./` or `/` is dropped, because a layer
/// built by one tool lists `etc/os-release` and one built by another lists
/// `./etc/os-release` for the same file.
pub fn scan_layer_filesystem(blob: &[u8]) -> LayerFilesystem {
    use ::std::io::Read;

    let mut found = LayerFilesystem::default();
    // The decompressed stream is read through a bound, so a layer whose compressed bytes
    // expand without limit costs this call a bounded amount of work.
    let decompressed: Box<dyn Read> = if blob.starts_with(&[0x1f, 0x8b]) {
        Box::new(::flate2::read::GzDecoder::new(blob))
    } else {
        Box::new(blob)
    };
    let mut archive = ::tar::Archive::new(decompressed.take(MAX_SCANNED_LAYER_BYTES));
    let Ok(entries) = archive.entries() else {
        return found;
    };
    for entry in entries.take(MAX_SCANNED_LAYER_ENTRIES) {
        // A member this archive cannot read is where the archive ends: what was read
        // before it still stands, and nothing past it is guessed at.
        let Ok(entry) = entry else {
            break;
        };
        let Ok(path) = entry.path() else {
            continue;
        };
        let path = path.to_string_lossy().into_owned();
        let path = path.trim_start_matches("./").trim_start_matches('/');
        if found.package_manager.is_none() {
            found.package_manager = package_manager_of_path(path).map(ToString::to_string);
        }
        if found.operating_system.is_none() {
            if let Some(named) = OS_RELEASE_PATHS
                .iter()
                .find(|(release, _)| *release == path)
                .map(|(_, named)| *named)
            {
                let mut content = String::new();
                // A release file that does not read as text names no operating system;
                // what it does carry is read through its own bound.
                let _ = entry
                    .take(MAX_RELEASE_FILE_BYTES)
                    .read_to_string(&mut content);
                found.operating_system =
                    os_release_id(&content).or_else(|| named.map(ToString::to_string));
            }
        }
        if found.is_complete() {
            break;
        }
    }
    found
}

/// The package manager whose installed-package database sits at `path`, or `None` where
/// nothing does. See [`PACKAGE_DATABASE_PATHS`]: an entry ending in `/` names a database
/// DIRECTORY, so everything under it is that database.
pub(super) fn package_manager_of_path(path: &str) -> Option<&'static str> {
    PACKAGE_DATABASE_PATHS
        .iter()
        .find(|(database, _)| {
            if let Some(directory) = database.strip_suffix('/') {
                path.starts_with(database) && path.len() > directory.len() + 1
            } else {
                path == *database
            }
        })
        .map(|(_, manager)| *manager)
}

/// The operating system an `os-release` file names: its `ID` field, or its `NAME` where
/// it carries no `ID`.
///
/// The format is the os-release specification's — newline-separated `KEY=value`, the
/// value optionally quoted — so a line that is not one of those two fields, a comment and
/// a blank line are all skipped, and a file that names neither field identifies no
/// operating system here.
pub(super) fn os_release_id(content: &str) -> Option<String> {
    let mut name: Option<String> = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "ID" => return Some(value),
            "NAME" => name = Some(value),
            _ => {}
        }
    }
    name
}

/// The one basic image scan an image carries: the run `StartImageScan` started on it.
///
/// A scan is the record of a RUN, not a projection of the image: it exists because a
/// start call created it, and reading it back computes nothing new. Its status follows
/// from the instants the clock owes it its run and its completion, so every reader
/// derives the same answer from the same record and no read has to write.
#[derive(Debug, Clone)]
pub struct ImageScan {
    /// When the call that admitted this scan was made. It is what the once-per-image
    /// [`BASIC_SCAN_INTERVAL`] is measured from, and what counts this scan towards the
    /// registry's [`MAX_BASIC_SCANS_PER_INTERVAL`].
    pub started_at: SystemTime,
    /// When the clock owes this scan its `PENDING` -> `IN_PROGRESS` transition:
    /// `started_at` + [`IMAGE_SCAN_QUEUE_DELAY`]. A start call ADMITS a scan — it does
    /// not run it — so until this instant the run is queued, and the transition out of
    /// the queue belongs to the clock rather than to any caller.
    pub running_due_at: SystemTime,
    /// When the clock owes this scan its `IN_PROGRESS` -> terminal transition:
    /// `running_due_at` + [`IMAGE_SCAN_DELAY`]. The transition is owed whether or not
    /// anybody calls again, so the instant is stored and never slept for, and a
    /// completed scan reports it as `imageScanCompletedAt` — the same instant on every
    /// read.
    pub complete_due_at: SystemTime,
    /// The status this run ends in once its completion is due, decided when the scan was
    /// started from the image it was started on ([`basic_scan_outcome`]) and stored, so
    /// the outcome is the run's own and every reader derives the same one. A scan is a
    /// record of what examining THAT image came to, which is why the terminal status is
    /// not re-derived — nor defaulted to success — on a read.
    pub terminal: ImageScanStatus,
    /// What the scan found, computed once when it was started (see
    /// [`basic_scan_findings`]) and stored, so paging through the findings reports the
    /// run rather than a fresh evaluation. The order is the run's own and never changes
    /// once stored, so a finding's INDEX in this list is the stable, unique and ascending
    /// key a page of findings is addressed by — see [`Self::finding_page_keys`]; a
    /// finding's `name` is neither unique nor ordered, so it is not that key.
    pub findings: Vec<ImageScanFinding>,
}

impl ImageScan {
    /// Whether the clock has already made this scan's completion due at `now`.
    pub fn is_complete(&self, now: SystemTime) -> bool {
        self.complete_due_at <= now
    }

    /// Whether the clock has already taken this scan out of the queue at `now`, i.e.
    /// whether the run has begun. A completed run has begun too.
    pub fn is_running(&self, now: SystemTime) -> bool {
        self.running_due_at <= now
    }

    /// When this scan completed, or `None` while it is still running. The instant is the
    /// one the clock owed, so it does not move between reads.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.is_complete(now).then_some(self.complete_due_at)
    }

    /// Whether a new scan of the same image may be started at `now`:
    /// [`BASIC_SCAN_INTERVAL`] must have passed since this one was started. A clock that
    /// went backwards leaves the scan inside its interval rather than reopening it.
    pub fn may_restart_at(&self, now: SystemTime) -> bool {
        matches!(now.duration_since(self.started_at), Ok(age) if age >= BASIC_SCAN_INTERVAL)
    }

    /// Whether this scan was started within `interval` of `now` — how a scan is counted
    /// towards the registry's scan budget.
    pub fn started_within(&self, interval: Duration, now: SystemTime) -> bool {
        match now.duration_since(self.started_at) {
            Ok(age) => age < interval,
            // Started "after" now: a clock that went backwards, so the scan is inside
            // any window ending at `now`.
            Err(_) => true,
        }
    }

    /// This scan's findings, each paired with the key a page is addressed by: its
    /// zero-padded INDEX in the run, which is unique and ascending as a string, so
    /// [`paginate`] resumes exactly where the previous page ended whatever the findings
    /// are named (two findings may share a `name`, or carry none at all).
    ///
    /// The width is fixed, so the keys sort the way the indices do; one run's findings
    /// are bounded by the image's own content, far inside it.
    pub fn finding_page_keys(&self) -> Vec<(String, ImageScanFinding)> {
        self.findings
            .iter()
            .enumerate()
            .map(|(index, finding)| (format!("{index:012}"), finding.clone()))
            .collect()
    }

    /// The `ImageScanStatus` every operation that reports this scan answers with — the
    /// start call included: how far the clock has carried this run through the lifecycle
    /// `ScanStatus` models for it. A run is `PENDING` while it is queued, `IN_PROGRESS`
    /// once the clock has made its run due, and its own terminal status once the clock
    /// has made its completion due.
    ///
    /// A queued run carries no description: the only wording ECR has for a scan is about
    /// a run that has begun or has ended, so nothing is authored for the state in which
    /// nothing has happened yet.
    pub fn to_status(&self, now: SystemTime) -> ImageScanStatus {
        if self.is_complete(now) {
            self.terminal.clone()
        } else if self.is_running(now) {
            ImageScanStatus {
                status: Some(SCAN_STATUS_IN_PROGRESS.to_string()),
                description: Some(SCAN_IN_PROGRESS_DESCRIPTION.to_string()),
            }
        } else {
            ImageScanStatus {
                status: Some(SCAN_STATUS_PENDING.to_string()),
                description: None,
            }
        }
    }

    /// Whether this run examined the image and finished — the one terminal status whose
    /// findings are a statement about the image.
    pub fn examined_the_image(&self) -> bool {
        self.terminal.status.as_deref() == Some(SCAN_STATUS_COMPLETE)
    }

    /// How many findings this scan reports per `FindingSeverity`, counted from the
    /// findings themselves. A scan with no findings counts none, which is the empty map
    /// ECR reports for an image nothing was found on.
    pub fn finding_severity_counts(&self) -> ::std::collections::HashMap<String, i32> {
        let mut counts: ::std::collections::HashMap<String, i32> =
            ::std::collections::HashMap::new();
        for finding in &self.findings {
            if let Some(severity) = finding.severity.as_deref() {
                *counts.entry(severity.to_string()).or_default() += 1;
            }
        }
        counts
    }

    /// The `imageScanFindings` a scan read reports, carrying `page` — the page of this
    /// scan's findings the request asked for — or `None` while the scan is still
    /// running: findings are readable once it has completed.
    ///
    /// The severity counts summarise the WHOLE scan rather than the page, so they do not
    /// change as a caller pages through the findings. `vulnerabilitySourceUpdatedAt` is
    /// omitted because no vulnerability source was consulted — see
    /// [`basic_scan_findings`].
    pub fn to_findings(
        &self,
        now: SystemTime,
        page: Vec<ImageScanFinding>,
    ) -> Option<ImageScanFindings> {
        if !self.examined_the_image() {
            // A run that could not examine the image found nothing to report: its status
            // is the whole answer, and a findings block would state a scan of content
            // nothing read.
            return None;
        }
        self.completed_at(now).map(|completed_at| ImageScanFindings {
            image_scan_completed_at: Some(super::reported_instant(completed_at)),
            vulnerability_source_updated_at: None,
            finding_severity_counts: Some(self.finding_severity_counts()),
            findings: Some(page),
            // `enhancedFindings` are Amazon Inspector's, which the ENHANCED scan type
            // produces; a basic scan reports its own findings in `findings`.
            enhanced_findings: None,
        })
    }
}

/// What a BASIC scan of `image` finds, given the repository's records of the blobs it is
/// built from.
///
/// A basic image scan is a VULNERABILITY scan of the operating-system packages in the
/// image's filesystem: a finding is one vulnerability a vulnerability source reports
/// against one installed package. The packages are identified here — the scan reads the
/// image's package database ([`scanned_filesystem`]) — but no vulnerability source is
/// reachable from this process, and a finding is never invented for a package nothing
/// looked up, so a run reports none and `findingSeverityCounts` counts what it reports
/// rather than stating a figure of its own. A scan that identified no filesystem at all
/// reports no findings either, and does not reach the status whose findings are readable
/// ([`basic_scan_outcome`]).
pub fn basic_scan_findings(
    image: &ImageRecord,
    layers: &BTreeMap<String, LayerBlob>,
) -> Vec<ImageScanFinding> {
    let _ = (image, layers);
    Vec::new()
}

/// Where a basic scan of `image` ENDS: the terminal `ImageScanStatus` the run reports
/// once the clock has made its completion due, given the repository's records of the
/// blobs the image is built from.
///
/// A scan is an examination of the image it was started on, so its outcome follows from
/// that image's CONTENT: a run reaches `COMPLETE`, the status whose findings are a
/// statement about the image, exactly where the scan identified in the image's filesystem
/// both the operating system and the package manager whose installed packages it reports
/// on ([`scanned_filesystem`]); an image missing either is one basic scanning does not
/// support, and the run ends `FAILED` with [`UNSUPPORTED_IMAGE_SCAN_DESCRIPTION`] — the
/// outcome that reports the image as one that is not supported. Neither outcome is the
/// default: each follows from what the layers carry.
pub fn basic_scan_outcome(
    image: &ImageRecord,
    layers: &BTreeMap<String, LayerBlob>,
) -> ImageScanStatus {
    if scanned_filesystem(image, layers).is_complete() {
        ImageScanStatus {
            status: Some(SCAN_STATUS_COMPLETE.to_string()),
            description: Some(SCAN_COMPLETE_DESCRIPTION.to_string()),
        }
    } else {
        ImageScanStatus {
            status: Some(SCAN_STATUS_FAILED.to_string()),
            description: Some(UNSUPPORTED_IMAGE_SCAN_DESCRIPTION.to_string()),
        }
    }
}

/// The filesystem a basic scan of `image` identifies: the operating system and the
/// package manager its layers carry between them.
///
/// An image's operating-system release file and its package database live in its
/// FILESYSTEM LAYERS, which is why a basic scan reads them: the layers are what the image
/// is made of. So the walk is over the layers the stored manifest declares, in the order
/// it declares them, resolved to this repository's own record of each blob — a manifest
/// descriptor states only a digest, a size and a media type, while the record carries what
/// reading the registered bytes identified ([`scan_layer_filesystem`]) — and an image
/// whose release file and package database sit in different layers is identified by both
/// of them together, which is how a multi-layer image carries them.
pub(super) fn scanned_filesystem(
    image: &ImageRecord,
    layers: &BTreeMap<String, LayerBlob>,
) -> LayerFilesystem {
    let mut identified = LayerFilesystem::default();
    for descriptor in manifest_layer_descriptors(&image.manifest) {
        // A digest the repository holds no blob for contributes no content: only a
        // registered layer has bytes this registry has read.
        if let Some(held) = layers.get(&descriptor.digest) {
            identified.absorb(&held.filesystem);
        }
        if identified.is_complete() {
            break;
        }
    }
    identified
}

/// Whether `manifest` describes something a basic scan can examine: a container image
/// with at least one filesystem layer.
///
/// A basic scan reads the operating-system packages in an image's filesystem, so the
/// manifest has to declare one. The Docker image manifest V2 schema 2 and the OCI image
/// manifest declare it as `layers`, the older V2 schema 1 as `fsLayers`; a manifest LIST
/// or OCI image index declares neither — it references other manifests, not layers — and
/// neither does a document that is not an image manifest at all. Those are what
/// `StartImageScan`'s `UnsupportedImageTypeException` ("the image is of a type that
/// cannot be scanned") reports.
pub fn is_scannable_image_manifest(manifest: &str) -> bool {
    !manifest_layer_descriptors(manifest).is_empty()
}

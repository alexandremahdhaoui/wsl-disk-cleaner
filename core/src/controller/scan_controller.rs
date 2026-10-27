use std::sync::Arc;
use std::thread;

use crate::port::UsageSource;
use crate::types::{
    DiskImage, ScanFailure, ScanResult, UsageNote, Volume, VolumeReport, VolumeUsage,
};
use crate::util::{error_chain, human_bytes};

pub trait Scanner: Send + Sync {
    fn scan(&self) -> ScanResult;
}

pub struct ScanController {
    sources: Vec<Arc<dyn UsageSource>>,
    min_note_bytes: u64,
}

impl ScanController {
    pub fn new(sources: Vec<Arc<dyn UsageSource>>, min_note_bytes: u64) -> Self {
        Self {
            sources,
            min_note_bytes,
        }
    }

    fn notes_for(&self, usage: &VolumeUsage, wsl_used: Option<u64>) -> Vec<UsageNote> {
        let mut notes = Vec::new();
        let unread = usage.used_bytes.saturating_sub(usage.walked_bytes);
        if unread >= self.min_note_bytes {
            notes.push(UsageNote {
                label: "Not readable by this scan".into(),
                bytes: unread,
                hint: "Folders this scan could not open. Examples are root owned folders, container storage and Windows system files.".into(),
            });
        }
        notes.extend(
            usage
                .disk_images
                .iter()
                .map(|image| disk_image_note(image, wsl_used)),
        );
        notes
    }
}

fn disk_image_note(image: &DiskImage, wsl_used: Option<u64>) -> UsageNote {
    let compact = format!(
        "wsl --shutdown, then Optimize-VHD -Path '{}' -Mode Full",
        image.path
    );
    let hint = match wsl_used {
        Some(used) => format!(
            "{} used inside WSL. Compact with: {compact}",
            human_bytes(used)
        ),
        None => format!("Compact with: {compact}"),
    };
    UsageNote {
        label: "WSL disk file".into(),
        bytes: image.bytes,
        hint,
    }
}

impl Scanner for ScanController {
    fn scan(&self) -> ScanResult {
        let results: Vec<(String, Result<VolumeUsage, String>)> = thread::scope(|scope| {
            let handles: Vec<_> = self
                .sources
                .iter()
                .map(|source| {
                    scope.spawn(move || {
                        source
                            .usage()
                            .map_err(|e| format!("scanning: {}", error_chain(&*e)))
                    })
                })
                .collect();
            self.sources
                .iter()
                .zip(handles)
                .map(|(source, handle)| {
                    let result = handle
                        .join()
                        .unwrap_or_else(|_| Err("scanning: the scan panicked".into()));
                    (source.name(), result)
                })
                .collect()
        });
        let mut usages = Vec::new();
        let mut failures = Vec::new();
        for (source, result) in results {
            match result {
                Ok(usage) => usages.push(usage),
                Err(reason) => failures.push(ScanFailure { source, reason }),
            }
        }
        let wsl_used = usages
            .iter()
            .find(|u| u.volume == Volume::Wsl)
            .map(|u| u.used_bytes);
        let reports = usages
            .into_iter()
            .map(|usage| VolumeReport {
                notes: self.notes_for(&usage, wsl_used),
                usage,
            })
            .collect();
        ScanResult { reports, failures }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::MockUsageSource;

    const GB: u64 = 1_000_000_000;

    fn usage(volume: Volume, used: u64, walked: u64, images: Vec<DiskImage>) -> VolumeUsage {
        VolumeUsage {
            volume,
            capacity_bytes: 1000 * GB,
            used_bytes: used,
            walked_bytes: walked,
            tree: vec![],
            disk_images: images,
        }
    }

    fn source(
        name: &'static str,
        result: Result<VolumeUsage, &'static str>,
    ) -> Arc<dyn UsageSource> {
        let mut source = MockUsageSource::new();
        source.expect_name().return_const(name.to_string());
        source
            .expect_usage()
            .return_once(move || result.map_err(|e| e.into()));
        Arc::new(source)
    }

    #[test]
    fn bytes_the_walk_could_not_read_become_a_note_above_the_threshold() {
        let result = ScanController::new(
            vec![source(
                "wsl",
                Ok(usage(Volume::Wsl, 193 * GB, 116 * GB, vec![])),
            )],
            GB,
        )
        .scan();
        let note = &result.reports[0].notes[0];
        assert_eq!(
            (note.label.as_str(), note.bytes),
            ("Not readable by this scan", 77 * GB)
        );
    }

    #[test]
    fn a_small_unread_gap_adds_no_note() {
        let result = ScanController::new(
            vec![source(
                "wsl",
                Ok(usage(Volume::Wsl, 10 * GB, 10 * GB - 5, vec![])),
            )],
            GB,
        )
        .scan();
        assert!(result.reports[0].notes.is_empty());
    }

    #[test]
    fn a_wsl_disk_file_note_shows_the_bytes_used_inside_wsl_and_the_compact_command() {
        let image = DiskImage {
            path: r"C:\wsl\ext4.vhdx".into(),
            bytes: 199 * GB,
        };
        let result = ScanController::new(
            vec![
                source("wsl", Ok(usage(Volume::Wsl, 30 * GB, 30 * GB, vec![]))),
                source(
                    "windows",
                    Ok(usage(Volume::Windows, 847 * GB, 847 * GB, vec![image])),
                ),
            ],
            GB,
        )
        .scan();
        let note = &result.reports[1].notes[0];
        assert_eq!(note.bytes, 199 * GB);
        assert_eq!(
            note.hint,
            r"30.0 GB used inside WSL. Compact with: wsl --shutdown, then Optimize-VHD -Path 'C:\wsl\ext4.vhdx' -Mode Full"
        );
    }

    #[test]
    fn a_failing_source_lands_in_failures_and_the_other_volume_still_reports() {
        let result = ScanController::new(
            vec![
                source("windows", Err("powershell.exe is not on PATH")),
                source("wsl", Ok(usage(Volume::Wsl, GB, GB, vec![]))),
            ],
            GB,
        )
        .scan();
        assert_eq!(result.reports.len(), 1);
        assert_eq!(
            result.failures,
            vec![ScanFailure {
                source: "windows".into(),
                reason: "scanning: powershell.exe is not on PATH".into()
            }]
        );
    }
}

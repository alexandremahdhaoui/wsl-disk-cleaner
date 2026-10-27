#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Volume {
    Wsl,
    Windows,
}

impl Volume {
    pub fn label(self) -> &'static str {
        match self {
            Volume::Wsl => "WSL",
            Volume::Windows => "Windows",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeNode {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub children: Vec<SizeNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskImage {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeUsage {
    pub volume: Volume,
    pub capacity_bytes: u64,
    pub used_bytes: u64,
    pub walked_bytes: u64,
    pub tree: Vec<SizeNode>,
    pub disk_images: Vec<DiskImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageNote {
    pub label: String,
    pub bytes: u64,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeReport {
    pub usage: VolumeUsage,
    pub notes: Vec<UsageNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFailure {
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanResult {
    pub reports: Vec<VolumeReport>,
    pub failures: Vec<ScanFailure>,
}

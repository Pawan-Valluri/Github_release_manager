use serde::{Deserialize, Serialize};
use std::sync::mpsc::Sender;

#[derive(Debug, Default, PartialEq)]
pub enum AppTab {
    #[default]
    Home,
    Tasks,    // Renamed from Downloads
    Settings, // NEW: Settings tab
}

// NEW: A struct to hold the downloadable files
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub download_url: String,
    pub size: u64, // in bytes
}

#[derive(Debug, Default, PartialEq)]
pub enum HomeState {
    #[default]
    Overview,
    Fetching {
        repo_name: String,
    },
    FetchingLatest {
        repo_name: String,
        allow_prerelease: bool,
    },
    // UPDATE: Now carries a Vec of the full ReleaseMetadata structs
    Selection {
        repo_name: String,
        available_releases: Vec<ReleaseMetadata>,
    },
    // UPDATE: Carries the single selected ReleaseMetadata
    FetchingAssets {
        repo_name: String,
        release: ReleaseMetadata,
        auto_update: bool, // NEW
    },
    AssetSelection {
        repo_name: String,
        release: ReleaseMetadata,
        assets: Vec<Asset>,
    },
    // NEW: The popup state
    ConfirmRegex {
        repo_name: String,
        release: ReleaseMetadata,
        asset: Asset,
        regex_string: String,
    },
    Error {
        message: String,
    },
}

pub struct Project {
    pub repo_name: String,
    pub version: String,
    pub owner: String,
    pub last_updated: String,
    pub readme: String,
    pub is_expanded: bool,
    pub release_info: Option<ReleaseMetadata>, // NEW: The raw GitHub data
    pub allow_prerelease: bool,
    pub asset_regex: Option<String>, // NEW
}

pub enum AsyncMessage {
    // UPDATE: Passing the rich data
    FetchComplete {
        repo_name: String,
        releases: Vec<ReleaseMetadata>,
    },
    FetchAssetsComplete {
        repo_name: String,
        release: ReleaseMetadata,
        assets: Vec<Asset>,
        auto_update: bool, // NEW
    },
    FetchError(String),

    DownloadStarted(String),
    DownloadProgress {
        file_name: String,
        progress: f32,
    },
    // UPDATE: When a download finishes, tell the UI what project to update!
    DownloadComplete {
        file_name: String,
        repo_name: String,
        release: ReleaseMetadata,
    },
    DownloadError {
        file_name: String,
        error: String,
    },
}

// NEW: Maps directly to the GitHub Release JSON
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseMetadata {
    pub tag_name: String,
    pub target_commitish: String,
    pub name: Option<String>, // Name can sometimes be null on GitHub
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub immutable: bool, // Added based on your requirements
    #[serde(default)]
    pub prerelease: bool,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: String,
}

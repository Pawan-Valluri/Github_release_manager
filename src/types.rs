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
    Selection {
        repo_name: String,
        available_versions: Vec<String>,
    },
    FetchingAssets {
        repo_name: String,
        version: String,
    },
    AssetSelection {
        repo_name: String,
        version: String,
        assets: Vec<Asset>,
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
}

pub enum AsyncMessage {
    FetchComplete {
        repo_name: String,
        versions: Vec<String>,
    },
    FetchAssetsComplete {
        repo_name: String,
        version: String,
        assets: Vec<Asset>,
    },
    FetchError(String),

    // NEW: Messages for streaming file downloads
    DownloadStarted(String),
    DownloadProgress {
        file_name: String,
        progress: f32,
    },
    DownloadComplete(String),
    DownloadError {
        file_name: String,
        error: String,
    },
}

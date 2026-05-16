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
    Fetching { repo_name: String },
    FetchingLatest { repo_name: String, allow_prerelease: bool },
    Selection { repo_name: String, available_releases: Vec<ReleaseMetadata> },
    
    // NEW: Question 1 - Keep the same asset?
    PromptAssetReuse { repo_name: String, release: ReleaseMetadata }, 
    
    FetchingAssets { repo_name: String, release: ReleaseMetadata, auto_update: bool },
    AssetSelection { repo_name: String, release: ReleaseMetadata, assets: Vec<Asset> },
    
    // RENAMED & EXPANDED: Question 2 - Confirm download & check pipeline
    ConfirmDownload { repo_name: String, release: ReleaseMetadata, asset: Asset, regex_string: String }, 
    
    ProjectConfig { repo_name: String },
    
    // NEW: Dedicated Pipeline Editor Page
    PipelineEditor { 
        repo_name: String, 
        // If they came here from the download flow, we hold the file info so we can start it after saving!
        pending_download: Option<(String, String, u64, ReleaseMetadata)> 
    },
    
    Error { message: String },
}

// The tasks that can be executed in the pipeline
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipelineTask {
    pub task_id: String,
}

pub struct Project {
    pub repo_name: String,
    pub version: String,
    pub owner: String,
    pub last_updated: String,
    pub readme: String,
    pub is_expanded: bool,
    pub release_info: Option<ReleaseMetadata>, // The raw GitHub data
    pub allow_prerelease: bool,
    pub asset_regex: Option<String>,
    pub current_asset_name: Option<String>,
    pub pipeline: Vec<PipelineTask>, // NEW: Execution pipeline
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

    DownloadStarted {
        repo_name: String,
        file_name: String,
    },
    DownloadProgress {
        repo_name: String,
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
    
    // NEW: Pipeline Execution Messages
    PipelineTaskStarted { repo_name: String, task_name: String },
    PipelineTaskLog { repo_name: String, log_line: String },
    PipelineTaskComplete { repo_name: String, success: bool },
    
    // NEW: Triggered when the loop of tasks is entirely done
    PipelineSequenceComplete { repo_name: String }, 
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

// --- Nushell Task Manifest Schema ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskManifest {
    pub id: String,         // NEW: e.g., "core.extract" or "bob.auto_patcher"
    pub entrypoint: String, // NEW: e.g., "main.nu"
    pub metadata: TaskMetadata,
    pub execution: ExecutionProfile,
    pub dependencies: TaskDependencies,
    pub io: IODefinitions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskMetadata {
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    pub manifest_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EngineRequirement {
    InternalPreferred,
    SystemRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionProfile {
    pub target_platforms: Vec<String>, // e.g., ["windows", "linux"]
    pub min_nu_version: String,
    pub engine_requirement: EngineRequirement,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDependencies {
    pub system_packages: Vec<String>, // e.g., ["unzip", "tar"]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IODefinitions {
    pub expected_env_vars: Vec<String>, // e.g., ["GRM_ASSET_PATH"]
    pub timeout_seconds: u64,
}

// --- Job Tracking Structures ---

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone)]
pub enum JobStage {
    Download { 
        file_name: String, 
        progress: f32, 
        status: JobStatus 
    },
    Script { 
        task_name: String, 
        logs: Vec<String>, 
        status: JobStatus 
    },
}

impl JobStage {
    pub fn status(&self) -> &JobStatus {
        match self {
            JobStage::Download { status, .. } => status,
            JobStage::Script { status, .. } => status,
        }
    }
}

pub struct ActiveJob {
    pub repo_name: String,
    pub target_version: String,
    pub stages: Vec<JobStage>,
}

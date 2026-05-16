use crate::types::{PipelineTask, Project, ReleaseMetadata};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestProject {
    pub repo_name: String,
    pub version: String,
    pub last_local_update: String,             // "when we are updating"
    pub release_info: Option<ReleaseMetadata>, // The detailed GitHub data
    #[serde(default)]
    pub allow_prerelease: bool,
    #[serde(default)]
    pub asset_regex: Option<String>,
    #[serde(default)]
    pub current_asset_name: Option<String>,
    #[serde(default)]
    pub pipeline: Vec<PipelineTask>, // NEW
}

#[derive(Serialize, Deserialize, Default)]
pub struct AppManifest {
    pub projects: Vec<ManifestProject>,
}

pub fn load_manifest(install_folder: &str) -> Vec<Project> {
    let folder_path = Path::new(install_folder);
    if !folder_path.exists() {
        let _ = fs::create_dir_all(folder_path);
    }

    let manifest_path = folder_path.join("manifest.json");

    // Create default manifest with our rich data structure
    if !manifest_path.exists() {
        let default_manifest = AppManifest {
            projects: vec![ManifestProject {
                repo_name: "neovim/neovim".to_string(),
                version: "v0.9.5".to_string(),
                last_local_update: Utc::now().to_rfc3339(),
                release_info: None,
                allow_prerelease: false,
                asset_regex: None,
                current_asset_name: None,
                pipeline: vec![],
            }],
        };
        if let Ok(json) = serde_json::to_string_pretty(&default_manifest) {
            let _ = fs::write(&manifest_path, json);
        }
    }

    let mut ui_projects = Vec::new();

    if let Ok(data) = fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<AppManifest>(&data) {
            for mp in manifest.projects {
                let parts: Vec<&str> = mp.repo_name.split('/').collect();
                let owner = if parts.len() > 1 {
                    parts[0].to_string()
                } else {
                    "Unknown".to_string()
                };

                // Format the timestamp nicely for the UI (e.g., extracting just the date)
                let display_date = mp
                    .last_local_update
                    .split('T')
                    .next()
                    .unwrap_or("Unknown")
                    .to_string();

                ui_projects.push(Project {
                    repo_name: mp.repo_name.clone(),
                    version: mp.version.clone(),
                    owner,
                    last_updated: format!("Updated: {}", display_date),
                    readme: format!("# {}\n\n**Current local version:** {}\n\n*Select a version to update.*", mp.repo_name, mp.version),
                    is_expanded: false,
                    release_info: mp.release_info.clone(),
                    allow_prerelease: mp.allow_prerelease,
                    asset_regex: mp.asset_regex.clone(),
                    current_asset_name: mp.current_asset_name.clone(),
                    pipeline: mp.pipeline.clone(), // NEW
                });
            }
        }
    }

    ui_projects
}

// NEW: We will call this in the next steps after a download finishes
pub fn save_manifest(install_folder: &str, projects: &[Project]) {
    let manifest_path = Path::new(install_folder).join("manifest.json");

    let manifest_projects: Vec<ManifestProject> = projects
        .iter()
        .map(|p| {
            ManifestProject {
                repo_name: p.repo_name.clone(),
                version: p.version.clone(),
                last_local_update: Utc::now().to_rfc3339(), // Stamp it with right now!
                release_info: p.release_info.clone(),
                allow_prerelease: p.allow_prerelease,
                asset_regex: p.asset_regex.clone(),
                current_asset_name: p.current_asset_name.clone(),
                pipeline: p.pipeline.clone(), // NEW
            }
        })
        .collect();

    let manifest = AppManifest {
        projects: manifest_projects,
    };

    if let Ok(json) = serde_json::to_string_pretty(&manifest) {
        let _ = fs::write(&manifest_path, json);
    }
}

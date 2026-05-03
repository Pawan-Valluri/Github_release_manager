use crate::types::Project;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// The struct that represents the JSON on disk
#[derive(Serialize, Deserialize)]
pub struct ManifestProject {
    pub repo_name: String,
    pub version: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct AppManifest {
    pub projects: Vec<ManifestProject>,
}

pub fn load_manifest(install_folder: &str) -> Vec<Project> {
    let folder_path = Path::new(install_folder);

    // 1. Check and create the install folder if it doesn't exist
    if !folder_path.exists() {
        let _ = fs::create_dir_all(folder_path);
    }

    let manifest_path = folder_path.join("manifest.json");

    // 2. Create a default manifest if it doesn't exist
    if !manifest_path.exists() {
        let default_manifest = AppManifest {
            projects: vec![
                // Adding one default so the UI isn't completely empty on first boot
                ManifestProject {
                    repo_name: "neovim/neovim".to_string(),
                    version: "v0.9.5".to_string(),
                },
            ],
        };
        if let Ok(json) = serde_json::to_string_pretty(&default_manifest) {
            let _ = fs::write(&manifest_path, json);
        }
    }

    let mut ui_projects = Vec::new();

    // 3. Read and parse the manifest
    if let Ok(data) = fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<AppManifest>(&data) {
            for mp in manifest.projects {
                // Extract owner from "owner/repo"
                let parts: Vec<&str> = mp.repo_name.split('/').collect();
                let owner = if parts.len() > 1 {
                    parts[0].to_string()
                } else {
                    "Unknown".to_string()
                };

                ui_projects.push(Project {
                    repo_name: mp.repo_name.clone(),
                    version: mp.version.clone(),
                    owner,
                    last_updated: "Local".to_string(),
                    // Generate a dynamic placeholder readme based on the local data
                    readme: format!("# {}\n\n**Current local version:** {}\n\n*Click 'Fetch Versions / Update' to query GitHub for the latest info.*", mp.repo_name, mp.version),
                    is_expanded: false,
                });
            }
        }
    }

    ui_projects
}

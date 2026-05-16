use std::process::Stdio;
use std::sync::mpsc::Sender;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use walkdir::WalkDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use crate::types::{AsyncMessage, TaskManifest};

// A wrapper to hold the parsed manifest AND its physical location on disk
#[derive(Debug, Clone)]
pub struct RegisteredTask {
    pub manifest: TaskManifest,
    pub directory: PathBuf,
}

// NEW: The Foolproof Task Scanner
pub fn scan_for_tasks(install_folder: &str) -> HashMap<String, RegisteredTask> {
    let tasks_dir = std::path::Path::new(install_folder).join("tasks");
    let mut registry = HashMap::new();

    // If the directory doesn't exist, create it and return empty registry
    if !tasks_dir.exists() {
        let _ = std::fs::create_dir_all(&tasks_dir);
        return registry;
    }

    // Recursively walk the tasks directory looking for 'task.toml'
    for entry in WalkDir::new(tasks_dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() && entry.file_name() == "task.toml" {
            let toml_path = entry.path();
            let parent_dir = toml_path.parent().unwrap().to_path_buf();

            if let Ok(content) = std::fs::read_to_string(toml_path) {
                if let Ok(manifest) = toml::from_str::<TaskManifest>(&content) {
                    // Make sure the entrypoint script actually exists!
                    let script_path = parent_dir.join(&manifest.entrypoint);
                    if script_path.exists() {
                        registry.insert(manifest.id.clone(), RegisteredTask {
                            manifest,
                            directory: parent_dir,
                        });
                    } else {
                        println!("Warning: Task '{}' is missing its entrypoint script '{}'", manifest.id, manifest.entrypoint);
                    }
                }
            }
        }
    }
    registry
}

// NEW: Helper to ensure our test script exists
pub async fn ensure_dummy_extract_script(install_folder: &str) -> Result<(), String> {
    let core_task_dir = std::path::Path::new(install_folder).join("tasks").join("core_extract");
    
    if !core_task_dir.exists() {
        tokio::fs::create_dir_all(&core_task_dir).await.map_err(|e| e.to_string())?;
        
        let script_path = core_task_dir.join("main.nu");
        let toml_path = core_task_dir.join("task.toml"); // Must be named task.toml for the scanner!

        let nu_code = r#"
def main [file_path: string, target_dir: string] {
    print $"Starting extraction sequence for: ($file_path)..."
    sleep 1sec
    print $"Extracting contents to: ($target_dir)..."
    sleep 1sec
    print $"SUCCESS: Extraction complete!"
}
"#;
        tokio::fs::write(&script_path, nu_code).await.map_err(|e| e.to_string())?;

        let toml_code = r#"
id = "core.extract"
entrypoint = "main.nu"

[metadata]
name = "Standard Extractor"
description = "Extracts standard archives using Nushell"
manifest_version = "1.0"

[execution]
target_platforms = ["windows", "linux", "macos"]
min_nu_version = "0.112.2"
engine_requirement = "internal_preferred"

[dependencies]
system_packages = []

[io]
expected_env_vars = ["GRM_ASSET_PATH", "GRM_INSTALL_DIR"]
timeout_seconds = 60
"#;
        tokio::fs::write(&toml_path, toml_code).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

// NEW: Validates the TOML file before allowing execution
pub async fn validate_task_manifest(toml_path: &str) -> Result<TaskManifest, String> {
    let content = tokio::fs::read_to_string(toml_path).await.map_err(|e| format!("Failed to read TOML: {}", e))?;
    let manifest: TaskManifest = toml::from_str(&content).map_err(|e| format!("Invalid TOML schema: {}", e))?;
    
    // Here you would add OS checks:
    // let current_os = std::env::consts::OS;
    // if !manifest.execution.target_platforms.contains(&current_os.to_string()) { ... }

    Ok(manifest)
}

// UPDATED: Now accepts environment variables for dynamic script execution
pub async fn run_nu_script(
    repo_name: String,
    task_name: String,
    script_path: String,
    asset_path: String,     // NEW
    install_dir: String,    // NEW
    tx: Sender<AsyncMessage>,
    ctx: eframe::egui::Context,
) -> Result<bool, String> {
    
    // Announce start
    let _ = tx.send(AsyncMessage::PipelineTaskStarted { repo_name: repo_name.clone(), task_name: task_name.clone() });
    ctx.request_repaint();

    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;

    // Spawn Subprocess and inject data via Environment Variables!
    let mut child = Command::new(current_exe)
        .arg("--nu-worker")
        .arg(&script_path)
        .env("GRM_ASSET_PATH", &asset_path)   // Passed to script
        .env("GRM_INSTALL_DIR", &install_dir) // Passed to script
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn worker: {}", e))?;

    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let stderr = child.stderr.take().expect("Failed to capture stderr");

    let tx_out = tx.clone();
    let repo_out = repo_name.clone();
    let ctx_out = ctx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_out.send(AsyncMessage::PipelineTaskLog { repo_name: repo_out.clone(), log_line: line });
            ctx_out.request_repaint();
        }
    });

    let tx_err = tx.clone();
    let repo_err = repo_name.clone();
    let ctx_err = ctx.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_err.send(AsyncMessage::PipelineTaskLog { repo_name: repo_err.clone(), log_line: format!("ERROR: {}", line) });
            ctx_err.request_repaint();
        }
    });

    let status = child.wait().await.map_err(|e| e.to_string())?;
    
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let success = status.success();
    let _ = tx.send(AsyncMessage::PipelineTaskComplete { repo_name, success });
    ctx.request_repaint();

    Ok(success)
}

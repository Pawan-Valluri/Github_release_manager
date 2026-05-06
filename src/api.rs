use crate::types::{Asset, AsyncMessage, ReleaseMetadata}; // Make sure AsyncMessage is imported
use serde::Deserialize;
use std::sync::mpsc::Sender;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use eframe::egui::Context;

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

pub async fn fetch_repo_releases(repo_name: &str) -> Result<Vec<ReleaseMetadata>, String> {
    let url = format!("https://api.github.com/repos/{}/releases", repo_name);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "GRM-Rust-App")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("API Error: {}", response.status()));
    }

    // Now we deserialize directly into our robust ReleaseMetadata struct!
    let releases: Vec<ReleaseMetadata> = response
        .json()
        .await
        .map_err(|e| format!("Parse error: {}", e))?;

    Ok(releases)
}

// Internal structs for deserializing the Asset JSON
#[derive(serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(serde::Deserialize)]
struct GitHubReleaseDetails {
    assets: Vec<GitHubAsset>,
}

// NEW: Fetch the actual files attached to a specific version
pub async fn fetch_release_assets(repo_name: &str, tag: &str) -> Result<Vec<Asset>, String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/tags/{}",
        repo_name, tag
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "GRM-Rust-App")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("API Error: {}", response.status()));
    }

    let release: GitHubReleaseDetails = response
        .json()
        .await
        .map_err(|e| format!("Parse error: {}", e))?;

    // Convert the GitHub JSON structs into our clean internal UI structs
    let assets = release
        .assets
        .into_iter()
        .map(|a| Asset {
            name: a.name,
            download_url: a.browser_download_url,
            size: a.size,
        })
        .collect();

    Ok(assets)
}

pub async fn download_asset(
    url: String,
    file_name: String,
    total_size: u64,
    target_dir: String,
    repo_name: String,
    release: ReleaseMetadata,
    tx: Sender<AsyncMessage>,
    ctx: Context,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut response = client
        .get(&url)
        .header("User-Agent", "GRM-Rust-App")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download failed: {}", response.status()));
    }

    // UPDATE: Use the target_dir from settings instead of current_dir
    let path = std::path::Path::new(&target_dir).join(&file_name);
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| format!("File creation error: {}", e))?;

    let mut downloaded: u64 = 0;
    let _ = tx.send(AsyncMessage::DownloadStarted(file_name.clone()));
    ctx.request_repaint(); // WAKE UP THE UI THREAD

    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;

        downloaded += chunk.len() as u64;
        let progress = if total_size > 0 {
            (downloaded as f32 / total_size as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let _ = tx.send(AsyncMessage::DownloadProgress {
            file_name: file_name.clone(),
            progress,
        });

        ctx.request_repaint(); // WAKE UP THE UI THREAD TO ANIMATE THE BAR
    }

    let _ = tx.send(AsyncMessage::DownloadComplete {
        file_name,
        repo_name,
        release,
    });
    ctx.request_repaint(); // WAKE UP THE UI THREAD FOR THE FINAL GREEN CHECKMARK
    Ok(())
}

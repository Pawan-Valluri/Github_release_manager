use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use tokio::runtime::Runtime; // NEW IMPORT
use crate::task_runner::{RegisteredTask, scan_for_tasks};

use crate::api::{fetch_release_assets, fetch_repo_releases};
use crate::config::{AppConfig, load_config, save_config};
use crate::types::{AppTab, AsyncMessage, HomeState, Project, ReleaseMetadata, ActiveJob, JobStage, JobStatus};

pub struct GrmApp {
    active_tab: AppTab,
    home_state: HomeState,
    sidebar_open: bool,
    projects: Vec<Project>,
    markdown_cache: CommonMarkCache,
    config: AppConfig,
    
    // REMOVE: active_tasks and task_logs
    // NEW: Unified active jobs tracker
    active_jobs: HashMap<String, ActiveJob>, 
    
    task_registry: HashMap<String, RegisteredTask>, // NEW
    
    rt: Runtime,
    tx: Sender<AsyncMessage>,
    rx: Receiver<AsyncMessage>,
}

impl GrmApp {
    pub fn new(rt: Runtime, tx: Sender<AsyncMessage>, rx: Receiver<AsyncMessage>) -> Self {
        let config = crate::config::load_config(); // Load settings from disk

        let install_folder = config.install_folder.clone();
        rt.block_on(async move {
            let _ = crate::task_runner::ensure_dummy_extract_script(&install_folder).await;
        });

        let mut app = Self {
            active_tab: AppTab::default(),
            home_state: HomeState::default(),
            sidebar_open: true,
            markdown_cache: CommonMarkCache::default(),
            task_registry: scan_for_tasks(&config.install_folder),
            config,
            active_jobs: HashMap::new(), // NEW
            rt,
            tx,
            rx,
            projects: Vec::new(), // Start empty
        };

        // Load the manifest immediately on boot
        app.reload_manifest();

        app
    }

    fn reload_manifest(&mut self) {
        self.projects = crate::manifest::load_manifest(&self.config.install_folder);
    }

    fn trigger_fetch(&mut self, repo_name: String, ctx: egui::Context) {
        self.home_state = HomeState::Fetching {
            repo_name: repo_name.clone(),
        };
        let tx = self.tx.clone();

        self.rt.spawn(async move {
            match fetch_repo_releases(&repo_name).await {
                Ok(releases) => {
                    let _ = tx.send(AsyncMessage::FetchComplete {
                        repo_name,
                        releases,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncMessage::FetchError(e));
                }
            }
            ctx.request_repaint();
        });
    }

    fn trigger_fetch_latest(&mut self, repo_name: String, allow_prerelease: bool, ctx: egui::Context) {
        self.home_state = HomeState::FetchingLatest { repo_name: repo_name.clone(), allow_prerelease };
        let tx = self.tx.clone();
        
        self.rt.spawn(async move {
            match fetch_repo_releases(&repo_name).await {
                Ok(releases) => { let _ = tx.send(AsyncMessage::FetchComplete { repo_name, releases }); }
                Err(e) => { let _ = tx.send(AsyncMessage::FetchError(e)); }
            }
            ctx.request_repaint();
        });
    }

    fn trigger_asset_fetch(
        &mut self,
        repo_name: String,
        release: ReleaseMetadata,
        auto_update: bool, // NEW PARAMETER
        ctx: egui::Context,
    ) {
        self.home_state = HomeState::FetchingAssets {
            repo_name: repo_name.clone(),
            release: release.clone(),
            auto_update, // SAVE IT IN STATE
        };
        let tx = self.tx.clone();

        self.rt.spawn(async move {
            match crate::api::fetch_release_assets(&repo_name, &release.tag_name).await {
                Ok(assets) => {
                    let _ = tx.send(AsyncMessage::FetchAssetsComplete {
                        repo_name,
                        release,
                        assets,
                        auto_update, // PASS IT TO THE MESSAGE
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncMessage::FetchError(e));
                }
            }
            ctx.request_repaint();
        });
    }
    fn trigger_download(
        &mut self,
        url: String,
        file_name: String,
        size: u64,
        repo_name: String,
        release: ReleaseMetadata,
        ctx: egui::Context,
    ) {
        let tx = self.tx.clone();
        let background_ctx = ctx.clone();
        let target_dir = self.config.install_folder.clone();

        self.rt.spawn(async move {
            // Pass the extra context into the api call
            if let Err(e) = crate::api::download_asset(
                url,
                file_name.clone(),
                size,
                target_dir,
                repo_name,
                release,
                tx.clone(),
                background_ctx.clone(),
            )
            .await
            {
                let _ = tx.send(AsyncMessage::DownloadError {
                    file_name,
                    error: e,
                });
                background_ctx.request_repaint();
            }
        });

        self.active_tab = AppTab::Tasks;
        self.home_state = HomeState::Overview; // Reset the Home view so it's clean when we return!
    }

    // NEW: Spawns a background task that waits for each pipeline step sequentially
    fn trigger_pipeline_sequence(&self, repo_name: String, pipeline: Vec<crate::types::PipelineTask>, file_name: String, ctx: egui::Context) {
        let tx = self.tx.clone();
        let background_ctx = ctx.clone();
        let install_folder = self.config.install_folder.clone();
        
        let asset_path = std::path::Path::new(&install_folder).join(&file_name).to_string_lossy().to_string();

        let registry = self.task_registry.clone();

        self.rt.spawn(async move {
            for task in pipeline {
                if let Some(registered) = registry.get(&task.task_id) {
                    let script_path = registered.directory.join(&registered.manifest.entrypoint);
                    let toml_path = registered.directory.join("task.toml");
                    
                    match crate::task_runner::validate_task_manifest(&toml_path.to_string_lossy().to_string()).await {
                        Ok(manifest) => {
                            let _ = crate::task_runner::run_nu_script(
                                repo_name.clone(), 
                                manifest.metadata.name, 
                                script_path.to_string_lossy().to_string(), 
                                asset_path.clone(), 
                                install_folder.clone(), 
                                tx.clone(), 
                                background_ctx.clone()
                            ).await;
                        }
                        Err(e) => {
                            let _ = tx.send(AsyncMessage::PipelineTaskLog { 
                                repo_name: repo_name.clone(), 
                                log_line: format!("ERROR: Manifest Validation Failed: {}", e) 
                            });
                            break;
                        }
                    }
                } else {
                    let _ = tx.send(AsyncMessage::PipelineTaskLog { 
                        repo_name: repo_name.clone(), 
                        log_line: format!("ERROR: Task '{}' not found in registry.", task.task_id) 
                    });
                    break;
                }
            }
            
            // Announce that all tasks are finished
            let _ = tx.send(AsyncMessage::PipelineSequenceComplete { repo_name });
            background_ctx.request_repaint();
        });
    }
}

impl eframe::App for GrmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_zoom_factor(self.config.ui_zoom_factor);

        // NEW: We use this to trigger a download safely outside the message loop
        let mut auto_download_trigger = None;

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AsyncMessage::FetchComplete {
                    repo_name,
                    releases,
                } => {
                    let mut auto_update = None;
                    if let HomeState::FetchingLatest { allow_prerelease, .. } = &self.home_state {
                        auto_update = Some(*allow_prerelease);
                    }

                    if let Some(allow_prerelease) = auto_update {
                        let latest = releases.into_iter().find(|r| {
                            if allow_prerelease { !r.draft } else { !r.draft && !r.prerelease }
                        });

                        if let Some(release) = latest {
                            // Pass 'true' for auto_update
                            self.trigger_asset_fetch(repo_name, release, true, ctx.clone());
                        } else {
                            self.home_state = HomeState::Error { message: "No suitable releases found on GitHub.".to_string() };
                        }
                    } else {
                        self.home_state = HomeState::Selection { repo_name, available_releases: releases };
                    }
                }
                AsyncMessage::FetchAssetsComplete {
                    repo_name,
                    release,
                    assets,
                    auto_update,
                } => {
                    let mut found_match = false;

                    // IF AUTO UPDATING: Check Regex!
                    if auto_update {
                        if let Some(project) = self.projects.iter().find(|p| p.repo_name == repo_name) {
                            if let Some(pattern) = &project.asset_regex {
                                if let Ok(re) = regex::Regex::new(pattern) {
                                    if let Some(asset) = assets.iter().find(|a| re.is_match(&a.name)) {
                                        auto_download_trigger = Some((
                                            asset.download_url.clone(), asset.name.clone(), asset.size,
                                            repo_name.clone(), release.clone(),
                                        ));
                                        found_match = true;
                                    }
                                }
                            }
                        }
                    }

                    // If not auto-updating, or regex failed, go to selection screen
                    if !found_match {
                        self.home_state = HomeState::AssetSelection {
                            repo_name,
                            release,
                            assets,
                        };
                    }
                }
                AsyncMessage::FetchError(err) => {
                    self.home_state = HomeState::Error { message: err };
                }
                // NEW: Handle Download states
                AsyncMessage::DownloadStarted { repo_name, .. } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        if let Some(JobStage::Download { status, .. }) = job.stages.get_mut(0) {
                            *status = JobStatus::Running;
                        }
                    }
                }
                AsyncMessage::DownloadProgress { repo_name, progress, .. } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        if let Some(JobStage::Download { progress: p, .. }) = job.stages.get_mut(0) {
                            *p = progress;
                        }
                    }
                    ctx.request_repaint();
                }
                AsyncMessage::DownloadComplete {
                    file_name,
                    repo_name,
                    release,
                } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        if let Some(JobStage::Download { progress, status, .. }) = job.stages.get_mut(0) {
                            *progress = 1.0;
                            *status = JobStatus::Success;
                        }
                    }

                    // --- THE MANIFEST MAGIC ---
                    // 1. Check if the project already exists in our tracked list
                    let mut found = false;
                    for project in &mut self.projects {
                        if project.repo_name == repo_name {
                            project.version = release.tag_name.clone();
                            project.release_info = Some(release.clone());
                            project.current_asset_name = Some(file_name.clone()); // NEW
                            found = true;
                            break;
                        }
                    }

                    // 2. If it's a new project, add it!
                    if !found {
                        let parts: Vec<&str> = repo_name.split('/').collect();
                        let owner = if parts.len() > 1 {
                            parts[0].to_string()
                        } else {
                            "Unknown".to_string()
                        };

                        self.projects.push(Project {
                            repo_name: repo_name.clone(),
                            version: release.tag_name.clone(),
                            owner,
                            last_updated: "Just now".to_string(),
                            readme: format!(
                                "# {}\n\nDownloaded version: {}",
                                repo_name, release.tag_name
                            ),
                            is_expanded: false,
                            release_info: Some(release.clone()),
                            allow_prerelease: false,
                            asset_regex: None,
                            current_asset_name: Some(file_name.clone()),
                            pipeline: vec![], // NEW
                        });
                    }

                    // 3. Save it permanently to disk!
                    crate::manifest::save_manifest(&self.config.install_folder, &self.projects);

                    // --- NEW: TRIGGER THE PIPELINE ---
                    // Find the project we just updated, grab its pipeline, and fire the executor!
                    if let Some(project) = self.projects.iter().find(|p| p.repo_name == repo_name) {
                        if !project.pipeline.is_empty() {
                            self.trigger_pipeline_sequence(
                                repo_name.clone(), 
                                project.pipeline.clone(), 
                                file_name.clone(), 
                                ctx.clone()
                            );
                        }
                    }
                }
                AsyncMessage::DownloadError { file_name, error } => {
                    println!("Error downloading {}: {}", file_name, error);
                }
                // --- PIPELINE MESSAGES ---
                AsyncMessage::PipelineTaskStarted { repo_name, task_name } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        for stage in &mut job.stages {
                            if let JobStage::Script { task_name: name, status, logs } = stage {
                                if name == &task_name && *status == JobStatus::Pending {
                                    *status = JobStatus::Running;
                                    logs.push(format!("--- Starting Task: {} ---", task_name));
                                    break;
                                }
                            }
                        }
                    }
                }
                AsyncMessage::PipelineTaskLog { repo_name, log_line } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        for stage in &mut job.stages {
                            if let JobStage::Script { status: JobStatus::Running, logs, .. } = stage {
                                logs.push(log_line.clone());
                                if logs.len() > 500 { logs.remove(0); } // Keep terminal from blowing up RAM
                                break;
                            }
                        }
                    }
                }
                AsyncMessage::PipelineTaskComplete { repo_name, success } => {
                    if let Some(job) = self.active_jobs.get_mut(&repo_name) {
                        for stage in &mut job.stages {
                            if let JobStage::Script { status, logs, .. } = stage {
                                if *status == JobStatus::Running {
                                    *status = if success { JobStatus::Success } else { JobStatus::Failed };
                                    logs.push(if success { "--- Success ---".to_string() } else { "--- FAILED ---".to_string() });
                                    break;
                                }
                            }
                        }
                    }
                }
                AsyncMessage::PipelineSequenceComplete { repo_name } => {
                    // All done! (We don't need to do much here visually anymore because the stages track themselves)
                }
            }
        }

        if self.sidebar_open {
            egui::SidePanel::left("sidebar")
                .resizable(true)
                .min_width(150.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("GRM");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("⏴").on_hover_text("Close Sidebar").clicked() {
                                self.sidebar_open = false;
                            }
                        });
                    });
                    ui.separator();

                    let mut trigger_manifest_reload = false;

                    if ui
                        .selectable_label(self.active_tab == AppTab::Home, "🏠  Home")
                        .clicked()
                    {
                        if self.active_tab != AppTab::Home {
                            trigger_manifest_reload = true; // They navigated to Home, trigger a read
                        }
                        self.active_tab = AppTab::Home;
                    }
                    if ui
                        .selectable_label(self.active_tab == AppTab::Tasks, "📋  Tasks")
                        .clicked()
                    {
                        self.active_tab = AppTab::Tasks;
                    }
                    if ui
                        .selectable_label(self.active_tab == AppTab::Settings, "⚙  Settings")
                        .clicked()
                    {
                        self.active_tab = AppTab::Settings;
                    }

                    // If the flag was set, reload from disk!
                    if trigger_manifest_reload {
                        self.reload_manifest();
                    }

                    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                        ui.separator();
                        ui.label("v0.1.0");
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if !self.sidebar_open {
                    if ui.button("☰").on_hover_text("Open Sidebar").clicked() {
                        self.sidebar_open = true;
                    }
                }
                ui.heading(match self.active_tab {
                    AppTab::Home => "GitHub Projects",
                    AppTab::Tasks => "Active Tasks",
                    AppTab::Settings => "Settings",
                });
            });
            ui.separator();

            match self.active_tab {
                AppTab::Home => {
                    // This variable stores state changes so we don't mutate `self.home_state` while it's borrowed
                    let mut next_home_state = None;
                    let mut pending_auto_update: Option<(String, bool)> = None;
                    let mut pending_custom_fetch: Option<String> = None;
                    
                    // UPDATE: Added a boolean to control if we use the Regex or go to AssetSelection
                    let mut pending_asset_fetch: Option<(String, ReleaseMetadata, bool)> = None; 
                    
                    let mut pending_download: Option<(String, String, u64, String, ReleaseMetadata, Option<String>)> = None;
                    let mut updated_regex_string: Option<String> = None;
                    let save_manifest_after_regex = false;
                    let mut reload_manifest_on_cancel = false;
                    
                    // NEW: Trigger to jump to Pipeline Editor with a pending download
                    let mut jump_to_pipeline: Option<(String, String, String, u64, ReleaseMetadata)> = None;

                    match &self.home_state {
                        HomeState::Fetching { repo_name } | HomeState::FetchingLatest { repo_name, .. } => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("Fetching releases for {}...", repo_name));
                            });
                        }
                        HomeState::Selection {
                            repo_name,
                            available_releases,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(format!("Releases for {}:", repo_name));
                                if ui.button("⬅ Back").clicked() {
                                    next_home_state = Some(HomeState::Overview);
                                }
                            });
                            ui.separator();

                            egui::ScrollArea::vertical().show(ui, |ui| {
                                egui::Frame::none()
                                    .inner_margin(egui::Margin {
                                        left: 0.0,
                                        right: 16.0,
                                        top: 0.0,
                                        bottom: 0.0,
                                    })
                                    .show(ui, |ui| {
                                        for release in available_releases {
                                            // Display the tag name, and append [DRAFT] or [PRERELEASE] if applicable
                                            let mut label = release.tag_name.clone();
                                            if release.draft {
                                                label.push_str(" [DRAFT]");
                                            }
                                            if release.prerelease {
                                                label.push_str(" [PRERELEASE]");
                                            }

                                            if ui
                                                .add_sized(
                                                    [ui.available_width(), 0.0],
                                                    egui::Button::new(label),
                                                )
                                                .clicked()
                                            {
                                                next_home_state = Some(HomeState::PromptAssetReuse {
                                                    repo_name: repo_name.clone(),
                                                    release: release.clone(),
                                                });
                                            }
                                        }
                                    });
                            });
                        }
                        
                        // NEW: PromptAssetReuse (Question 1)
                        HomeState::PromptAssetReuse { repo_name, release } => {
                            ui.heading("Asset Selection Strategy");
                            ui.separator();
                            ui.label(format!("You selected version {}. Do you want to use your existing Asset Regex pattern to find the file automatically, or select a new file manually?", release.tag_name));
                            ui.add_space(16.0);
                            
                            ui.horizontal(|ui| {
                                let auto_btn = ui.add(egui::Button::new("🤖 Auto-Match Existing Pattern").fill(egui::Color32::from_rgb(30, 100, 40)));
                                if auto_btn.clicked() {
                                    // true = Auto match regex
                                    pending_asset_fetch = Some((repo_name.clone(), release.clone(), true)); 
                                }
                                
                                let manual_btn = ui.add(egui::Button::new("✋ Select New File Manually").fill(egui::Color32::from_rgb(40, 80, 140)));
                                if manual_btn.clicked() {
                                    // false = Go to AssetSelection screen
                                    pending_asset_fetch = Some((repo_name.clone(), release.clone(), false)); 
                                }
                            });
                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() { next_home_state = Some(HomeState::Overview); }
                        }
                        HomeState::FetchingAssets { repo_name, release, .. } => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!(
                                    "Finding assets for {} ({}) ...",
                                    repo_name, release.tag_name
                                ));
                            });
                        }
                        HomeState::AssetSelection {
                            repo_name,
                            release,
                            assets,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Assets for {} ({}):",
                                    repo_name, release.tag_name
                                ));
                                if ui.button("⬅ Back to Versions").clicked() {
                                    next_home_state = Some(HomeState::Overview);
                                }
                            });
                            ui.separator();

                            if assets.is_empty() {
                                ui.label("No downloadable assets found for this release.");
                            } else {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for asset in assets {
                                        ui.horizontal(|ui| {
                                            let mb = asset.size as f64 / 1_048_576.0;
                                            ui.label(format!("{:.2} MB", mb));

                                            if ui.button(format!("⬇ Download {}", asset.name)).clicked() {
                                                let proposed_regex = asset.name.replace(&release.tag_name, "(.*?)");
                                                next_home_state = Some(HomeState::ConfirmDownload {
                                                    repo_name: repo_name.clone(),
                                                    release: release.clone(),
                                                    asset: asset.clone(),
                                                    regex_string: format!("^{}$", proposed_regex),
                                                });
                                            }
                                        });
                                        ui.add_space(4.0);
                                    }
                                });
                            }
                        }

                        // RENAMED & EXPANDED: Confirm Download Options
                        HomeState::ConfirmDownload { repo_name, release, asset, regex_string } => {
                            ui.heading("Review Asset & Post-Download Tasks");
                            ui.separator();
                            ui.label("Review the regex pattern to ensure future auto-updates can find this file.");
                            
                            ui.add_space(8.0);
                            ui.strong(format!("Selected File: {}", asset.name));
                            
                            ui.horizontal(|ui| {
                                ui.label("Asset Regex:");
                                let mut temp_regex = regex_string.clone();
                                if ui.add_sized([ui.available_width(), 0.0], egui::TextEdit::singleline(&mut temp_regex)).changed() {
                                    updated_regex_string = Some(temp_regex);
                                }
                            });

                            ui.add_space(16.0);
                            ui.heading("Execution Pipeline");
                            ui.label("Would you like to review/edit the post-download tasks before continuing?");
                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                // 1. Direct Download
                                let direct_btn = ui.add(egui::Button::new("✔ Confirm & Download (Use Existing Tasks)").fill(egui::Color32::from_rgb(30, 100, 40)));
                                if direct_btn.clicked() {
                                    pending_download = Some((
                                        asset.download_url.clone(), asset.name.clone(), asset.size,
                                        repo_name.clone(), release.clone(), Some(regex_string.clone()),
                                    ));
                                }
                                
                                // 2. Go to Pipeline
                                let edit_btn = ui.add(egui::Button::new("⚙ Edit Tasks First").fill(egui::Color32::from_rgb(140, 80, 40)));
                                if edit_btn.clicked() {
                                    jump_to_pipeline = Some((
                                        repo_name.clone(), asset.download_url.clone(), asset.name.clone(), asset.size, release.clone()
                                    ));
                                }
                            });
                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() { next_home_state = Some(HomeState::Overview); }
                        }
                        
                        // NEW: Dedicated Pipeline Editor
                        HomeState::PipelineEditor { repo_name, pending_download: editor_pending_download } => {
                            ui.horizontal(|ui| {
                                let cancel_btn = ui.add(egui::Button::new("Cancel").fill(egui::Color32::from_rgb(100, 100, 100)));
                                if cancel_btn.clicked() {
                                    next_home_state = Some(HomeState::Overview);
                                    reload_manifest_on_cancel = true;
                                }
                                ui.heading(format!("📋 Pipeline Editor: {}", repo_name));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Save Button (Green)
                                    let save_btn = ui.add(egui::Button::new("💾 Save & Continue").fill(egui::Color32::from_rgb(30, 100, 40)));
                                    if save_btn.clicked() {
                                        crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                                        
                                        // If they came here from the download flow, trigger it!
                                        if let Some((url, name, size, release)) = editor_pending_download {
                                            pending_download = Some((
                                                url.clone(), name.clone(), *size, repo_name.clone(), release.clone(), None
                                            ));
                                        } else {
                                            next_home_state = Some(HomeState::Overview);
                                        }
                                    }
                                });
                            });
                            ui.separator();
                            
                            if let Some(project) = self.projects.iter_mut().find(|p| p.repo_name == *repo_name) {
                                ui.label("Sequence of actions performed automatically after a successful download.");
                                ui.add_space(8.0);
                                
                                let mut task_to_delete = None;
                                let mut task_to_add = None;

                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for (i, task) in project.pipeline.iter().enumerate() {
                                        egui::Frame::window(&ui.style()).inner_margin(8.0).show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                let _ = ui.button("🔄 Change");
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                                    if ui.button("❌ Delete").clicked() { task_to_delete = Some(i); }
                                                });
                                            });
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                if let Some(registered) = self.task_registry.get(&task.task_id) {
                                                    ui.strong(&registered.manifest.metadata.name);
                                                } else {
                                                    ui.colored_label(egui::Color32::RED, format!("Missing Task: {}", task.task_id));
                                                }
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                                                    let _ = ui.button("⚙ Config");
                                                });
                                            });
                                        });
                                        ui.vertical_centered(|ui| { ui.label("⬇"); });
                                    }

                                    let mut dropzone_frame = egui::Frame::window(&ui.style());
                                    dropzone_frame.fill = egui::Color32::from_black_alpha(40);
                                    dropzone_frame.inner_margin(12.0).show(ui, |ui| {
                                        ui.centered_and_justified(|ui| {
                                            // Instead of a single button, use a Menu Button for dynamic tasks
                                            ui.menu_button("+ Add Task", |ui| {
                                                if self.task_registry.is_empty() {
                                                    ui.label("No tasks installed.");
                                                } else {
                                                    for (task_id, registered_task) in &self.task_registry {
                                                        if ui.button(&registered_task.manifest.metadata.name).on_hover_text(&registered_task.manifest.metadata.description).clicked() {
                                                            task_to_add = Some(crate::types::PipelineTask { 
                                                                task_id: task_id.clone() 
                                                            });
                                                            ui.close_menu();
                                                        }
                                                    }
                                                }
                                            });
                                        });
                                    });
                                });

                                if let Some(idx) = task_to_delete { project.pipeline.remove(idx); }
                                if let Some(new_task) = task_to_add { project.pipeline.push(new_task); }
                            } else {
                                ui.colored_label(egui::Color32::RED, "Project not found!");
                            }
                        }
                        HomeState::ProjectConfig { repo_name } => {
                            ui.horizontal(|ui| {
                                ui.heading(format!("⚙ Configuration: {}", repo_name));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("⬅ Back").clicked() {
                                        next_home_state = Some(HomeState::Overview);
                                    }
                                });
                            });
                            ui.separator();

                            if let Some(project) = self.projects.iter_mut().find(|p| p.repo_name == *repo_name) {
                                // We use a flag so we only write to disk once per frame if a setting changes
                                let mut trigger_save = false;

                                ui.add_space(8.0);

                                ui.group(|ui| {
                                    ui.heading("Version & Asset Control");
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        if ui.button("🔄 Change Version").on_hover_text("Fetch and select a different release version").clicked() {
                                            pending_custom_fetch = Some(repo_name.clone());
                                        }
                                        if ui.button("📦 Change Asset").on_hover_text("Select a different file from the current version").clicked() {
                                            if let Some(release) = &project.release_info {
                                                pending_asset_fetch = Some((repo_name.clone(), release.clone(), false));
                                            } else {
                                                pending_custom_fetch = Some(repo_name.clone());
                                            }
                                        }
                                    });
                                });

                                ui.add_space(8.0);

                                ui.group(|ui| {
                                    ui.heading("Update Rules");
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Asset Regex:");
                                        let mut temp_regex = project.asset_regex.clone().unwrap_or_default();
                                        if ui.add_sized([ui.available_width(), 0.0], egui::TextEdit::singleline(&mut temp_regex)).changed() {
                                            project.asset_regex = Some(temp_regex);
                                            trigger_save = true;
                                        }
                                    });
                                    ui.small("This Regex is used to automatically find the correct file when updating.");
                                    ui.add_space(4.0);

                                    // MOVED HERE: The prerelease checkbox
                                    if ui.checkbox(&mut project.allow_prerelease, "Allow Pre-releases").changed() {
                                        trigger_save = true;
                                    }
                                });

                                    // Pipeline has been moved to PipelineEditor!

                                // Save to disk if anything changed this frame
                                if trigger_save {
                                    crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                                }

                            } else {
                                ui.colored_label(egui::Color32::RED, "Project not found!");
                            }
                        }
                        HomeState::Error { message } => {
                            ui.colored_label(egui::Color32::RED, format!("Error: {}", message));
                            if ui.button("⬅ Back").clicked() {
                                next_home_state = Some(HomeState::Overview);
                            }
                        }
                        HomeState::Overview => {
                            let target_expanded_height = ui.available_height() * 0.40;

                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {

                                    for project in &mut self.projects {
                                        // RESTORED UI: The Card Frame
                                        egui::Frame::window(&ui.style()).inner_margin(12.0).show(
                                            ui,
                                            |ui| {
                                                // Force full width to prevent layout jitter
                                                ui.set_min_width(ui.available_width());

                                                // RESTORED UI: The explicitly formatted Top & Bottom Rows
                                                let header_response = ui
                                                    .group(|ui| {
                                                        ui.horizontal(|ui| {
                                                            // --- OWNER/REPO VISUAL POLISH ---
                                                            ui.horizontal(|ui| {
                                                                ui.spacing_mut().item_spacing.x = 0.0;
                                                                ui.label(format!("{}/", project.owner));
                                                                let repo_only = project.repo_name.split('/').last().unwrap_or(&project.repo_name);
                                                                ui.strong(repo_only);
                                                            }).response.on_hover_text("Repository Name");
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.label(&project.version)
                                                                        .on_hover_text(
                                                                            "Current Version",
                                                                        );
                                                                },
                                                            );
                                                        });
                                                        ui.horizontal(|ui| {
                                                            ui.small(&project.last_updated)
                                                                .on_hover_text(
                                                                    "Last updated locally",
                                                                );
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    // Show Asset name instead of Owner
                                                                    let asset_display = project.current_asset_name.as_deref().unwrap_or("No asset downloaded");
                                                                    ui.small(asset_display)
                                                                        .on_hover_text(
                                                                            "Tracked Asset",
                                                                        );
                                                                },
                                                            );
                                                        });
                                                    })
                                                    .response
                                                    .interact(egui::Sense::click());

                                                if header_response.clicked() {
                                                    project.is_expanded = !project.is_expanded;
                                                }

                                                if project.is_expanded {
                                                    ui.add_space(8.0);
                                                    
                                                    // --- THE NEW BUTTON LAYOUT ---
                                                    ui.horizontal(|ui| {
                                                        // LEFT SIDE: Auto Update & Pipeline
                                                        let update_btn = ui.add(
                                                            egui::Button::new("⬆ Update")
                                                                .fill(egui::Color32::from_rgb(30, 100, 40))
                                                        );
                                                        if update_btn.on_hover_text("Automatically fetches the latest version and takes you to the download").clicked() {
                                                            pending_auto_update = Some((project.repo_name.clone(), project.allow_prerelease));
                                                        }
                                                        
                                                        // NEW: Pipeline Access Button
                                                        let pipe_btn = ui.add(
                                                            egui::Button::new("📋 Pipeline")
                                                                .fill(egui::Color32::from_rgb(100, 40, 100)) // Deep Purple
                                                        );
                                                        if pipe_btn.on_hover_text("Edit Post-Download Tasks").clicked() {
                                                            next_home_state = Some(HomeState::PipelineEditor { 
                                                                repo_name: project.repo_name.clone(),
                                                                pending_download: None, // No download yet, just editing
                                                            });
                                                        }

                                                        // RIGHT SIDE: Manage Project
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            let config_btn = ui.add(
                                                                egui::Button::new("⚙ Manage Project")
                                                                    .fill(egui::Color32::from_rgb(40, 80, 140))
                                                            );
                                                            if config_btn.on_hover_text("Configure versions, assets, and update rules").clicked() {
                                                                next_home_state = Some(HomeState::ProjectConfig { repo_name: project.repo_name.clone() });
                                                            }
                                                        });
                                                    });
                                                    ui.separator();

                                                    egui::ScrollArea::vertical()
                                                        .id_source(&project.repo_name)
                                                        .min_scrolled_height(target_expanded_height)
                                                        .max_height(target_expanded_height)
                                                        .show(ui, |ui| {
                                                            CommonMarkViewer::new(format!(
                                                                "md_{}",
                                                                project.repo_name
                                                            ))
                                                            .show(
                                                                ui,
                                                                &mut self.markdown_cache,
                                                                &project.readme,
                                                            );
                                                        });
                                                }
                                            },
                                        );
                                        ui.add_space(8.0);
                                    }

                                });
                        }
                    }

                    if let Some(new_state) = next_home_state {
                        self.home_state = new_state;
                    }
                    if let Some(new_regex) = updated_regex_string {
                        if let HomeState::ConfirmDownload { regex_string, .. } = &mut self.home_state {
                            *regex_string = new_regex;
                        }
                    }

                    if let Some((repo, allow_pre)) = pending_auto_update {
                        self.trigger_fetch_latest(repo, allow_pre, ctx.clone());
                    }
                    if let Some(repo) = pending_custom_fetch {
                        self.trigger_fetch(repo, ctx.clone());
                    }
                    // UPDATE: Pass the auto_update boolean!
                    if let Some((repo, release, auto_match)) = pending_asset_fetch {
                        self.trigger_asset_fetch(repo, release, auto_match, ctx.clone());
                    }
                    
                    // NEW: Jump to pipeline & save regex
                    if let Some((repo, url, name, size, release)) = jump_to_pipeline {
                        // We must save the regex if they typed one in before clicking "Edit Pipeline"
                        if let HomeState::ConfirmDownload { regex_string, .. } = &self.home_state {
                            if let Some(proj) = self.projects.iter_mut().find(|p| p.repo_name == repo) {
                                proj.asset_regex = Some(regex_string.clone());
                            }
                        }
                        self.home_state = HomeState::PipelineEditor {
                            repo_name: repo.clone(),
                            pending_download: Some((url, name, size, release))
                        };
                    }
                    if let Some((url, name, size, repo, release, regex_opt)) = pending_download {
                        // If they confirmed a regex, save it to the project immediately!
                        if let Some(regex_str) = regex_opt {
                            if let Some(proj) = self.projects.iter_mut().find(|p| p.repo_name == repo) {
                                proj.asset_regex = Some(regex_str);
                                crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                            }
                        }

                        // --- NEW: BUILD THE JOB CARD ---
                        if let Some(project) = self.projects.iter().find(|p| p.repo_name == repo) {
                            let mut stages = vec![JobStage::Download {
                                file_name: name.clone(),
                                progress: 0.0,
                                status: JobStatus::Pending,
                            }];
                            
                            // Append future pipeline scripts as pending stages
                            for task in &project.pipeline {
                                if let Some(registered) = self.task_registry.get(&task.task_id) {
                                    stages.push(JobStage::Script {
                                        task_name: registered.manifest.metadata.name.clone(),
                                        logs: Vec::new(),
                                        status: JobStatus::Pending,
                                    });
                                }
                            }
                            
                            self.active_jobs.insert(repo.clone(), ActiveJob {
                                repo_name: repo.clone(),
                                target_version: release.tag_name.clone(),
                                stages,
                            });
                        }

                        self.trigger_download(url, name, size, repo, release, ctx.clone());
                    }

                    // TRIGGER THE AUTO-DOWNLOAD IF REGEX MATCHED
                    if let Some((url, name, size, repo, release)) = auto_download_trigger {
                        if let Some(project) = self.projects.iter().find(|p| p.repo_name == repo) {
                            let mut stages = vec![JobStage::Download {
                                file_name: name.clone(),
                                progress: 0.0,
                                status: JobStatus::Pending,
                            }];
                            
                            for task in &project.pipeline {
                                if let Some(registered) = self.task_registry.get(&task.task_id) {
                                    stages.push(JobStage::Script {
                                        task_name: registered.manifest.metadata.name.clone(),
                                        logs: Vec::new(),
                                        status: JobStatus::Pending,
                                    });
                                }
                            }
                            
                            self.active_jobs.insert(repo.clone(), ActiveJob {
                                repo_name: repo.clone(),
                                target_version: release.tag_name.clone(),
                                stages,
                            });
                        }
                        self.trigger_download(url, name, size, repo, release, ctx.clone());
                    }
                    // Deferred manifest save from ProjectConfig regex edit
                    if save_manifest_after_regex {
                        crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                    }
                    if reload_manifest_on_cancel {
                        self.reload_manifest();
                    }
                }
                AppTab::Tasks => {
                    ui.heading("Active & Recent Jobs");
                    ui.separator();

                    if self.active_jobs.is_empty() {
                        ui.label("No active jobs.");
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (repo_name, job) in &self.active_jobs {
                                
                                // DRAW THE JOB CARD
                                egui::Frame::window(&ui.style()).inner_margin(12.0).show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    
                                    // 1. Header (Project Name & Global Progress)
                                    ui.horizontal(|ui| {
                                        ui.heading(&job.repo_name);
                                        ui.label(format!("→ Updating to {}", job.target_version));
                                    });
                                    
                                    let total_stages = job.stages.len() as f32;
                                    let completed_stages = job.stages.iter().filter(|s| *s.status() == JobStatus::Success).count() as f32;
                                    let overall_progress = if total_stages > 0.0 { completed_stages / total_stages } else { 0.0 };
                                    
                                    ui.add(egui::ProgressBar::new(overall_progress).animate(overall_progress < 1.0));
                                    ui.add_space(8.0);
                                    
                                    // 2. The Stepper (Accordions)
                                    for (i, stage) in job.stages.iter().enumerate() {
                                        let stage_num = i + 1;
                                        
                                        match stage {
                                            JobStage::Download { file_name, progress, status } => {
                                                let icon = match status {
                                                    JobStatus::Pending => "⏳",
                                                    JobStatus::Running => "🔄",
                                                    JobStatus::Success => "✔",
                                                    JobStatus::Failed => "❌",
                                                };
                                                
                                                let title = format!("{} Stage {}: Download ({})", icon, stage_num, file_name);
                                                
                                                // Expand automatically if it is currently running
                                                egui::CollapsingHeader::new(title).default_open(*status == JobStatus::Running).show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label("Streaming binary from GitHub...");
                                                        if *status == JobStatus::Success { ui.colored_label(egui::Color32::GREEN, "Done"); }
                                                    });
                                                    ui.add(egui::ProgressBar::new(*progress).show_percentage());
                                                });
                                            }
                                            JobStage::Script { task_name, logs, status } => {
                                                let icon = match status {
                                                    JobStatus::Pending => "⏳",
                                                    JobStatus::Running => "🔄",
                                                    JobStatus::Success => "✔",
                                                    JobStatus::Failed => "❌",
                                                };
                                                
                                                let title = format!("{} Stage {}: Execute ({})", icon, stage_num, task_name);
                                                
                                                egui::CollapsingHeader::new(title).default_open(*status == JobStatus::Running).show(ui, |ui| {
                                                    if logs.is_empty() {
                                                        ui.label("Waiting to start...");
                                                    } else {
                                                        // The Embedded Black Terminal
                                                        let mut terminal_frame = egui::Frame::dark_canvas(&ui.style());
                                                        terminal_frame.fill = egui::Color32::from_rgb(10, 10, 10);
                                                        terminal_frame = terminal_frame.inner_margin(8.0);
                                                        
                                                        terminal_frame.show(ui, |ui| {
                                                            egui::ScrollArea::vertical()
                                                                .id_source(format!("log_{}_{}", repo_name, i))
                                                                .max_height(200.0)
                                                                .stick_to_bottom(true)
                                                                .show(ui, |ui| {
                                                                    ui.set_min_width(ui.available_width());
                                                                    for line in logs {
                                                                        let color = if line.starts_with("ERROR:") || line.contains("FAILED") {
                                                                            egui::Color32::RED
                                                                        } else if line.contains("Success") {
                                                                            egui::Color32::GREEN
                                                                        } else {
                                                                            egui::Color32::LIGHT_GRAY
                                                                        };
                                                                        ui.colored_label(color, egui::RichText::new(line).monospace().size(12.0));
                                                                    }
                                                                });
                                                        });
                                                    }
                                                });
                                            }
                                        }
                                    }
                                });
                                ui.add_space(8.0);
                            }
                        });
                    }
                }
                AppTab::Settings => {
                    ui.heading("General Settings");
                    ui.separator();

                    // We use this boolean to avoid writing to disk 60 times a second
                    let mut config_changed = false;

                    ui.horizontal(|ui| {
                        ui.label("Install Folder:");
                        let folder_edit = ui.add_sized(
                            [ui.available_width(), 0.0],
                            egui::TextEdit::singleline(&mut self.config.install_folder),
                        );
                        // Only save if the user edited the text
                        if folder_edit.changed() {
                            config_changed = true;
                        }
                    });
                    ui.small("Downloaded assets will be saved to this directory.");

                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        ui.label("Global Font & UI Scale:");
                        // Add the slider, clamping it between 0.8 (80%) and 2.0 (200%)
                        let slider = ui.add(
                            egui::Slider::new(&mut self.config.ui_zoom_factor, 0.8..=2.0)
                                .text("Multiplier"),
                        );
                        // Save to disk if the slider is moved
                        if slider.changed() {
                            config_changed = true;
                        }
                    });
                    ui.small("Scales the entire application. Saves automatically.");

                    // If any setting was touched this frame, write the new JSON to disk
                    if config_changed {
                        save_config(&self.config);
                    }
                }
            }
        });
    }
}

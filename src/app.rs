use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use tokio::runtime::Runtime; // NEW IMPORT

use crate::api::{fetch_release_assets, fetch_repo_releases};
use crate::config::{AppConfig, load_config, save_config};
use crate::types::{AppTab, AsyncMessage, HomeState, Project, ReleaseMetadata};

pub struct GrmApp {
    active_tab: AppTab,
    home_state: HomeState,
    sidebar_open: bool,
    projects: Vec<Project>,
    markdown_cache: CommonMarkCache,
    config: AppConfig,
    active_tasks: HashMap<String, f32>,
    rt: Runtime,
    tx: Sender<AsyncMessage>,
    rx: Receiver<AsyncMessage>,
}

impl GrmApp {
    pub fn new(rt: Runtime, tx: Sender<AsyncMessage>, rx: Receiver<AsyncMessage>) -> Self {
        let config = crate::config::load_config(); // Load settings from disk

        let mut app = Self {
            active_tab: AppTab::default(),
            home_state: HomeState::default(),
            sidebar_open: true,
            markdown_cache: CommonMarkCache::default(),
            config,
            active_tasks: HashMap::new(),
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
                AsyncMessage::DownloadStarted(name) => {
                    self.active_tasks.insert(name, 0.0);
                }
                AsyncMessage::DownloadProgress {
                    file_name,
                    progress,
                } => {
                    if let Some(p) = self.active_tasks.get_mut(&file_name) {
                        *p = progress;
                    }
                    ctx.request_repaint(); // Tell the UI to redraw immediately for smooth animation
                }
                AsyncMessage::DownloadComplete {
                    file_name,
                    repo_name,
                    release,
                } => {
                    if let Some(p) = self.active_tasks.get_mut(&file_name) {
                        *p = 1.0;
                    }

                    // --- THE MANIFEST MAGIC ---
                    // 1. Check if the project already exists in our tracked list
                    let mut found = false;
                    for project in &mut self.projects {
                        if project.repo_name == repo_name {
                            project.version = release.tag_name.clone();
                            project.release_info = Some(release.clone());
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
                        });
                    }

                    // 3. Save it permanently to disk!
                    crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                }
                AsyncMessage::DownloadError { file_name, error } => {
                    println!("Error downloading {}: {}", file_name, error);
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
                    let mut pending_asset_fetch: Option<(String, ReleaseMetadata)> = None;
                    // UPDATE: Added Option<String> to hold the confirmed regex
                    let mut pending_download: Option<(String, String, u64, String, ReleaseMetadata, Option<String>)> = None;
                    let mut updated_regex_string: Option<String> = None;
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
                                                pending_asset_fetch =
                                                    Some((repo_name.clone(), release.clone()));
                                            }
                                        }
                                    });
                            });
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
                                                // INSTEAD OF DOWNLOADING, PROPOSE A REGEX AND GO TO POPUP
                                                let proposed_regex = asset.name.replace(&release.tag_name, "(.*?)");
                                                next_home_state = Some(HomeState::ConfirmRegex {
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

                        // NEW: The Regex Confirmation Popup
                        HomeState::ConfirmRegex { repo_name, release, asset, regex_string } => {
                            ui.heading("Confirm Download & Asset Pattern");
                            ui.separator();
                            ui.label("To auto-update this project in the future, GRM needs a Regex pattern to identify the correct file.");
                            ui.add_space(8.0);

                            ui.strong(format!("Selected File: {}", asset.name));

                            ui.horizontal(|ui| {
                                ui.label("Regex Pattern:");
                                let mut temp_regex = regex_string.clone();
                                if ui.add_sized([ui.available_width(), 0.0], egui::TextEdit::singleline(&mut temp_regex)).changed() {
                                    updated_regex_string = Some(temp_regex);
                                }
                            });

                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    next_home_state = Some(HomeState::Overview);
                                }
                                if ui.button("Confirm Download & Save Pattern").clicked() {
                                    pending_download = Some((
                                        asset.download_url.clone(),
                                        asset.name.clone(),
                                        asset.size,
                                        repo_name.clone(),
                                        release.clone(),
                                        Some(regex_string.clone()),
                                    ));
                                }
                            });
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
                                                                    ui.small(&project.owner)
                                                                        .on_hover_text(
                                                                            "Repository Owner",
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
                                                        // LEFT SIDE: Auto Update & Checkbox
                                                        let update_btn = ui.add(
                                                            egui::Button::new("⬆ Update")
                                                                .fill(egui::Color32::from_rgb(30, 100, 40)) // Dark Green
                                                        );
                                                        if update_btn.on_hover_text("Automatically fetches the latest version and takes you to the download").clicked() {
                                                            pending_auto_update = Some((project.repo_name.clone(), project.allow_prerelease));
                                                        }
                                                        
                                                        ui.checkbox(&mut project.allow_prerelease, "Pre-releases")
                                                            .on_hover_text("Include Beta/Alpha versions when finding the latest update");

                                                        // RIGHT SIDE: Custom Version
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            let custom_btn = ui.add(
                                                                egui::Button::new("⚙ Custom Version")
                                                                    .fill(egui::Color32::from_rgb(40, 80, 140)) // Dark Blue
                                                            );
                                                            if custom_btn.on_hover_text("View all available versions manually").clicked() {
                                                                pending_custom_fetch = Some(project.repo_name.clone());
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
                        if let HomeState::ConfirmRegex { regex_string, .. } = &mut self.home_state {
                            *regex_string = new_regex;
                        }
                    }

                    if let Some((repo, allow_pre)) = pending_auto_update {
                        self.trigger_fetch_latest(repo, allow_pre, ctx.clone());
                    }
                    if let Some(repo) = pending_custom_fetch {
                        self.trigger_fetch(repo, ctx.clone());
                    }
                    if let Some((repo, release)) = pending_asset_fetch {
                        // Pass 'false' for auto_update since they clicked Custom Version
                        self.trigger_asset_fetch(repo, release, false, ctx.clone());
                    }
                    if let Some((url, name, size, repo, release, regex_opt)) = pending_download {
                        // If they confirmed a regex, save it to the project immediately!
                        if let Some(regex_str) = regex_opt {
                            if let Some(proj) = self.projects.iter_mut().find(|p| p.repo_name == repo) {
                                proj.asset_regex = Some(regex_str);
                                crate::manifest::save_manifest(&self.config.install_folder, &self.projects);
                            }
                        }
                        self.trigger_download(url, name, size, repo, release, ctx.clone());
                    }

                    // TRIGGER THE AUTO-DOWNLOAD IF REGEX MATCHED
                    if let Some((url, name, size, repo, release)) = auto_download_trigger {
                        self.trigger_download(url, name, size, repo, release, ctx.clone());
                    }
                }
                AppTab::Tasks => {
                    if self.active_tasks.is_empty() {
                        ui.label("No active tasks.");
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let mut tasks: Vec<_> = self.active_tasks.iter().collect();
                            tasks.sort_by(|a, b| a.0.cmp(b.0));

                            for (name, progress) in tasks {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(name);
                                        if *progress == 1.0 {
                                            ui.colored_label(egui::Color32::GREEN, "✔ Complete");
                                        }
                                    });
                                    let bar = egui::ProgressBar::new(*progress)
                                        .show_percentage()
                                        .animate(*progress < 1.0);
                                    ui.add(bar);
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

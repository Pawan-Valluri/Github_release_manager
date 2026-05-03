use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use tokio::runtime::Runtime; // NEW IMPORT

use crate::api::{fetch_release_assets, fetch_repo_releases};
use crate::config::{AppConfig, load_config, save_config};
use crate::types::{AppTab, AsyncMessage, HomeState, Project};

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
                Ok(versions) => {
                    let _ = tx.send(AsyncMessage::FetchComplete {
                        repo_name,
                        versions,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncMessage::FetchError(e));
                }
            }
            ctx.request_repaint();
        });
    }

    fn trigger_asset_fetch(&mut self, repo_name: String, version: String, ctx: egui::Context) {
        self.home_state = HomeState::FetchingAssets {
            repo_name: repo_name.clone(),
            version: version.clone(),
        };
        let tx = self.tx.clone();

        self.rt.spawn(async move {
            match fetch_release_assets(&repo_name, &version).await {
                Ok(assets) => {
                    let _ = tx.send(AsyncMessage::FetchAssetsComplete {
                        repo_name,
                        version,
                        assets,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncMessage::FetchError(e));
                }
            }
            ctx.request_repaint();
        });
    }

    fn trigger_download(&mut self, url: String, file_name: String, size: u64, ctx: egui::Context) {
        let tx = self.tx.clone();
        let background_ctx = ctx.clone();
        let target_dir = self.config.install_folder.clone();

        self.rt.spawn(async move {
            if let Err(e) = crate::api::download_asset(
                url,
                file_name.clone(),
                size,
                target_dir,
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

        self.active_tab = AppTab::Tasks; // Go to Tasks tab
    }
}

impl eframe::App for GrmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_zoom_factor(self.config.ui_zoom_factor);

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AsyncMessage::FetchComplete {
                    repo_name,
                    versions,
                } => {
                    self.home_state = HomeState::Selection {
                        repo_name,
                        available_versions: versions,
                    };
                }
                AsyncMessage::FetchAssetsComplete {
                    repo_name,
                    version,
                    assets,
                } => {
                    self.home_state = HomeState::AssetSelection {
                        repo_name,
                        version,
                        assets,
                    };
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
                AsyncMessage::DownloadComplete(name) => {
                    // For now, let's just push it to 100% and keep it in the list so the user sees it finished
                    if let Some(p) = self.active_tasks.get_mut(&name) {
                        *p = 1.0;
                    }
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
                    let mut pending_asset_fetch: Option<(String, String)> = None;
                    let mut pending_download: Option<(String, String, u64)> = None;

                    match &self.home_state {
                        HomeState::Fetching { repo_name } => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("Fetching releases for {}...", repo_name));
                            });
                        }
                        HomeState::Selection {
                            repo_name,
                            available_versions,
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
                                        for version in available_versions {
                                            if ui
                                                .add_sized(
                                                    [ui.available_width(), 0.0],
                                                    egui::Button::new(version),
                                                )
                                                .clicked()
                                            {
                                                // UPDATE: Defer the fetch to avoid borrow checker errors
                                                pending_asset_fetch =
                                                    Some((repo_name.clone(), version.clone()));
                                            }
                                        }
                                    });
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
                                    let mut fetch_target = None;

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
                                                            ui.strong(&project.repo_name)
                                                                .on_hover_text("Repository Name");
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
                                                    ui.horizontal(|ui| {
                                                        if ui
                                                            .button("Fetch Versions / Update")
                                                            .clicked()
                                                        {
                                                            fetch_target =
                                                                Some(project.repo_name.clone());
                                                        }
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

                                    if let Some(repo) = fetch_target {
                                        self.trigger_fetch(repo, ctx.clone());
                                    }
                                });
                        }
                        // NEW: Spinner for fetching assets
                        HomeState::FetchingAssets { repo_name, version } => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!(
                                    "Finding assets for {} ({}) ...",
                                    repo_name, version
                                ));
                            });
                        }
                        // NEW: The UI showing the actual files!
                        HomeState::AssetSelection {
                            repo_name,
                            version,
                            assets,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(format!("Assets for {} ({}):", repo_name, version));
                                if ui.button("⬅ Back to Versions").clicked() {
                                    // Let's cheat a bit and re-fetch versions if they hit back,
                                    // or you can cache them. For now, let's just go to overview.
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
                                            // Convert bytes to Megabytes
                                            let mb = asset.size as f64 / 1_048_576.0;
                                            ui.label(format!("{:.2} MB", mb));

                                            if ui
                                                .button(format!("⬇ Download {}", asset.name))
                                                .clicked()
                                            {
                                                pending_download = Some((
                                                    asset.download_url.clone(),
                                                    asset.name.clone(),
                                                    asset.size,
                                                ));
                                            }
                                        });
                                        ui.add_space(4.0);
                                    }
                                });
                            }
                        }
                    }

                    // Apply the state transition if one occurred
                    if let Some(new_state) = next_home_state {
                        self.home_state = new_state;
                    }

                    // NEW: Trigger the asset fetch now that the immutable borrow is dropped
                    if let Some((repo, version)) = pending_asset_fetch {
                        self.trigger_asset_fetch(repo, version, ctx.clone());
                    }

                    if let Some((url, name, size)) = pending_download {
                        self.trigger_download(url, name, size, ctx.clone());
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

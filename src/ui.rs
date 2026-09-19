use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use eframe::egui::{self, Color32, RichText, Stroke};

use crate::{
    config::Settings,
    input::enumerate_input,
    media::{self, CancellationToken, GpuCapabilities},
    model::{EpisodeStage, QueueItem, VIDEO_EXTENSIONS},
    paths::AppPaths,
    pipeline::{self, PipelineEvent},
    update::{self, AvailableUpdate},
    voicestudio::{VoiceChoice, VoiceStudioManager},
};

const BG: Color32 = Color32::from_rgb(8, 14, 24);
const PANEL: Color32 = Color32::from_rgb(14, 23, 38);
const TEXT: Color32 = Color32::from_rgb(236, 243, 250);
const MUTED: Color32 = Color32::from_rgb(151, 166, 184);
const BORDER: Color32 = Color32::from_rgb(42, 58, 78);
const ACCENT: Color32 = Color32::from_rgb(60, 181, 255);
const SUCCESS: Color32 = Color32::from_rgb(67, 207, 152);
const WARNING: Color32 = Color32::from_rgb(255, 187, 74);
const DANGER: Color32 = Color32::from_rgb(244, 103, 111);

#[derive(Debug)]
enum UiEvent {
    Voices(Result<Vec<VoiceChoice>, String>),
    Gpu(GpuCapabilities),
    Preview(Result<(), String>),
    Update(Result<Option<AvailableUpdate>, String>),
    UpdateProgress(f32, String),
    UpdateLaunched(Result<(), String>),
}

#[derive(Clone)]
struct Notice {
    title: String,
    body: String,
    color: Color32,
}

pub struct RecapApp {
    paths: AppPaths,
    settings: Settings,
    voicestudio: VoiceStudioManager,
    queue: Vec<QueueItem>,
    selected_input: Option<PathBuf>,
    running: bool,
    cancel: CancellationToken,
    pipeline_rx: Option<Receiver<PipelineEvent>>,
    pipeline_thread: Option<thread::JoinHandle<()>>,
    ui_tx: Sender<UiEvent>,
    ui_rx: Receiver<UiEvent>,
    voices: Vec<VoiceChoice>,
    gpu: Option<GpuCapabilities>,
    voices_loading: bool,
    gpu_checking: bool,
    preview_loading: bool,
    settings_open: bool,
    status: String,
    notice: Option<Notice>,
    update_checking: bool,
    update_available: Option<AvailableUpdate>,
    update_downloading: bool,
    update_progress: f32,
    update_status: String,
}

impl RecapApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let paths = AppPaths::discover();
        let _ = paths.ensure_dirs();
        let settings = Settings::load(&paths.settings);
        let voicestudio = VoiceStudioManager::new(paths.clone());
        let (ui_tx, ui_rx) = mpsc::channel();
        let mut app = Self {
            paths,
            settings,
            voicestudio,
            queue: Vec::new(),
            selected_input: None,
            running: false,
            cancel: CancellationToken::default(),
            pipeline_rx: None,
            pipeline_thread: None,
            ui_tx,
            ui_rx,
            voices: Vec::new(),
            gpu: None,
            voices_loading: false,
            gpu_checking: false,
            preview_loading: false,
            settings_open: false,
            status: "Select one video or a folder to begin.".into(),
            notice: None,
            update_checking: false,
            update_available: None,
            update_downloading: false,
            update_progress: 0.0,
            update_status: String::new(),
        };
        app.refresh_gpu();
        app.refresh_voices();
        if app.settings.check_updates_on_start { app.check_updates(); }
        app
    }

    fn refresh_gpu(&mut self) {
        if self.gpu_checking { return; }
        self.gpu_checking = true;
        let tx = self.ui_tx.clone();
        let paths = self.paths.clone();
        thread::spawn(move || {
            let _ = tx.send(UiEvent::Gpu(media::detect_nvidia(&paths, &CancellationToken::default())));
        });
    }

    fn refresh_voices(&mut self) {
        if self.voices_loading { return; }
        self.voices_loading = true;
        let tx = self.ui_tx.clone();
        let manager = self.voicestudio.clone();
        let settings = self.settings.clone();
        thread::spawn(move || {
            let result = manager.voices(&settings, &CancellationToken::default()).map_err(|e| format!("{e:#}"));
            let _ = tx.send(UiEvent::Voices(result));
        });
    }

    fn preview_voice(&mut self) {
        if self.preview_loading { return; }
        self.preview_loading = true;
        let tx = self.ui_tx.clone();
        let manager = self.voicestudio.clone();
        let settings = self.settings.clone();
        let paths = self.paths.clone();
        thread::spawn(move || {
            let output = paths.data.join("cache").join("voice-preview.wav");
            let cancel = CancellationToken::default();
            let result = manager.preview(
                &settings,
                "Welcome to Tool Recap. This VoiceStudio voice is ready for automated video narration.",
                &output,
                &cancel,
                |p, text| { let _ = tx.send(UiEvent::UpdateProgress(p, text.to_owned())); },
            ).and_then(|_| media::play_audio(&paths, &output)).map_err(|e| format!("{e:#}"));
            let _ = tx.send(UiEvent::Preview(result));
        });
    }

    fn select_input(&mut self, path: PathBuf) {
        if self.running { return; }
        match enumerate_input(&path) {
            Ok(videos) => {
                self.queue = videos.into_iter().map(QueueItem::new).collect();
                self.selected_input = Some(path.clone());
                let parent = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
                self.settings.last_input_directory = parent.to_string_lossy().to_string();
                let _ = self.settings.save(&self.paths.settings);
                self.status = format!("{} video(s) detected and ready.", self.queue.len());
                self.notice = None;
            }
            Err(error) => self.error("Input not accepted", &error.to_string()),
        }
    }

    fn start(&mut self) {
        if self.running || self.queue.is_empty() { return; }
        if !self.gpu.as_ref().map(|g| g.available).unwrap_or(false) {
            let body = self.gpu.as_ref().map(|g| g.reason.clone()).unwrap_or_else(|| "GPU check is still running.".into());
            self.error("NVIDIA RTX render unavailable", &body);
            return;
        }
        if self.settings.api_endpoint.trim().is_empty() || self.settings.api_model.trim().is_empty() {
            self.settings_open = true;
            self.warn("Analysis API required", "Configure the OpenAI-compatible analysis endpoint and model.");
            return;
        }
        if let Err(error) = self.settings.save(&self.paths.settings) {
            self.error("Could not save settings", &error.to_string());
            return;
        }
        for item in &mut self.queue {
            item.stage = EpisodeStage::Waiting;
            item.progress = 0.0;
            item.status = "Queued".into();
            item.output = None;
        }
        self.cancel.reset();
        let (tx, rx) = mpsc::channel();
        self.pipeline_rx = Some(rx);
        self.pipeline_thread = Some(pipeline::spawn_batch(
            self.queue.clone(), self.settings.clone(), self.paths.clone(),
            self.voicestudio.clone(), self.cancel.clone(), tx,
        ));
        self.running = true;
        self.status = "Processing sequentially — analyze → prepare → render.".into();
        self.notice = None;
    }

    fn stop(&mut self) {
        if self.running {
            self.cancel.cancel();
            self.status = "Stopping the active episode and its child processes…".into();
        }
    }

    fn check_updates(&mut self) {
        if self.update_checking || self.update_downloading { return; }
        self.update_checking = true;
        let tx = self.ui_tx.clone();
        thread::spawn(move || {
            let _ = tx.send(UiEvent::Update(update::check_for_update().map_err(|e| format!("{e:#}"))));
        });
    }

    fn apply_update(&mut self) {
        let Some(update) = self.update_available.clone() else { return; };
        if self.running || self.update_downloading { return; }
        self.update_downloading = true;
        let tx = self.ui_tx.clone();
        let paths = self.paths.clone();
        thread::spawn(move || {
            let cancel = CancellationToken::default();
            let result = update::download_and_launch_update(&paths, &update, &cancel, |p, text| {
                let _ = tx.send(UiEvent::UpdateProgress(p, text.to_owned()));
            }).map_err(|e| format!("{e:#}"));
            let _ = tx.send(UiEvent::UpdateLaunched(result));
        });
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.ui_rx.try_recv() {
            match event {
                UiEvent::Voices(result) => {
                    self.voices_loading = false;
                    match result {
                        Ok(voices) => self.voices = voices,
                        Err(error) => self.warn("VoiceStudio unavailable", &error),
                    }
                }
                UiEvent::Gpu(gpu) => { self.gpu_checking = false; self.gpu = Some(gpu); }
                UiEvent::Preview(result) => {
                    self.preview_loading = false;
                    match result {
                        Ok(()) => self.success("Voice preview", "VoiceStudio preview is playing."),
                        Err(error) => self.error("Voice preview failed", &error),
                    }
                }
                UiEvent::Update(result) => {
                    self.update_checking = false;
                    match result {
                        Ok(Some(update)) => self.update_available = Some(update),
                        Ok(None) => {}
                        Err(error) => self.warn("Update check failed", &error),
                    }
                }
                UiEvent::UpdateProgress(p, text) => {
                    self.update_progress = p;
                    self.update_status = text;
                }
                UiEvent::UpdateLaunched(result) => {
                    self.update_downloading = false;
                    match result {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(error) => self.error("Update failed safely", &format!("The current installation was left usable. {error}")),
                    }
                }
            }
        }

        let mut finished = None;
        if let Some(rx) = &self.pipeline_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    PipelineEvent::Episode { index, stage, progress, status, output } => {
                        if let Some(item) = self.queue.get_mut(index) {
                            item.stage = stage;
                            item.progress = progress;
                            item.status = status;
                            if output.is_some() { item.output = output; }
                        }
                    }
                    PipelineEvent::SystemStatus(text) => self.status = text,
                    PipelineEvent::BatchFinished { completed, failed, stopped } => finished = Some((completed, failed, stopped)),
                }
            }
        }
        if let Some((completed, failed, stopped)) = finished {
            self.running = false;
            self.pipeline_rx = None;
            if let Some(handle) = self.pipeline_thread.take() { let _ = handle.join(); }
            if stopped {
                self.status = "Stopped. Start is available again.".into();
                self.notice = Some(Notice { title: "Processing stopped".into(), body: format!("{completed} episode(s) completed before Stop."), color: WARNING });
            } else if failed > 0 {
                self.status = "Processing failed. Start is available again.".into();
                self.notice = Some(Notice { title: "Recap production failed".into(), body: format!("{completed} completed, {failed} failed."), color: DANGER });
            } else {
                self.status = "All episodes completed.".into();
                self.notice = Some(Notice { title: "Recap production complete".into(), body: format!("Rendered {completed} episode(s) successfully."), color: SUCCESS });
                if self.settings.notify_complete { notify_native("Tool Recap complete", &format!("Rendered {completed} episode(s).")); }
            }
        }
    }

    fn info_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).fill(PANEL).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("INPUT & RUNTIME").strong().color(ACCENT).size(11.0));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.running, egui::Button::new("Select video")).clicked() {
                    let mut dialog = rfd::FileDialog::new().add_filter("Supported video", VIDEO_EXTENSIONS);
                    if !self.settings.last_input_directory.is_empty() { dialog = dialog.set_directory(&self.settings.last_input_directory); }
                    if let Some(path) = dialog.pick_file() { self.select_input(path); }
                }
                if ui.add_enabled(!self.running, egui::Button::new("Select folder")).clicked() {
                    let mut dialog = rfd::FileDialog::new();
                    if !self.settings.last_input_directory.is_empty() { dialog = dialog.set_directory(&self.settings.last_input_directory); }
                    if let Some(path) = dialog.pick_folder() { self.select_input(path); }
                }
                if let Some(path) = &self.selected_input {
                    ui.label(RichText::new(path.display().to_string()).color(MUTED).size(12.0));
                }
            });
            ui.label(RichText::new("Folder mode scans direct children only; subfolders and non-video files are ignored.").color(MUTED).size(11.0));
            ui.separator();
            let gpu_text = if self.gpu_checking { "Checking…".into() } else if let Some(gpu) = &self.gpu {
                if gpu.available { format!("{} • H.264 NVENC ready", gpu.gpu_name) } else { gpu.reason.clone() }
            } else { "Not checked".into() };
            status_row(ui, "NVIDIA render", &gpu_text, self.gpu.as_ref().map(|g| g.available).unwrap_or(false));
            status_row(ui, "VoiceStudio", if self.voices_loading { "Starting / checking…" } else if self.voices.is_empty() { "Unavailable" } else { "Ready" }, !self.voices.is_empty());
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.running, egui::Button::new("Recheck GPU")).clicked() { self.refresh_gpu(); }
                if ui.add_enabled(!self.running, egui::Button::new("Reload voices")).clicked() { self.refresh_voices(); }
            });
        });
    }

    fn voice_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).fill(PANEL).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
            ui.label(RichText::new("VOICESTUDIO NARRATION").strong().color(ACCENT).size(11.0));
            egui::ComboBox::from_id_salt("voice_select")
                .selected_text(self.voices.iter().find(|v| v.voice_id == self.settings.voice_id).map(VoiceChoice::display_name).unwrap_or_else(|| self.settings.voice_id.clone()))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for voice in &self.voices {
                        ui.selectable_value(&mut self.settings.voice_id, voice.voice_id.clone(), voice.display_name());
                    }
                });
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.running && !self.preview_loading, egui::Button::new(if self.preview_loading { "Preparing preview…" } else { "Preview voice" })).clicked() { self.preview_voice(); }
                if ui.add_enabled(!self.running, egui::Button::new("Voice & API settings")).clicked() { self.settings_open = true; }
            });
            ui.label(RichText::new("Large models download on first use into the portable data folder and are reused.").color(MUTED).size(11.0));
        });
    }

    fn queue_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).fill(PANEL).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("PROCESSING QUEUE").strong().color(ACCENT).size(11.0));
                ui.label(RichText::new(format!("{} episode(s) • sequential only", self.queue.len())).color(MUTED).size(11.0));
            });
            if self.queue.is_empty() {
                ui.add_space(20.0);
                ui.label(RichText::new("No videos queued. Select a source above.").color(MUTED));
                ui.add_space(20.0);
            } else {
                egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                    egui::Grid::new("queue").num_columns(4).spacing([12.0, 8.0]).striped(true).show(ui, |ui| {
                        for heading in ["VIDEO", "STAGE", "PROGRESS", "STATUS"] {
                            ui.label(RichText::new(heading).strong().color(MUTED).size(10.0));
                        }
                        ui.end_row();
                        for item in &self.queue {
                            ui.label(RichText::new(item.source.file_name().and_then(|s| s.to_str()).unwrap_or(&item.name)).color(TEXT));
                            ui.label(RichText::new(item.stage.label()).color(stage_color(item.stage)));
                            ui.add(egui::ProgressBar::new(item.progress).desired_width(150.0).show_percentage());
                            ui.label(RichText::new(&item.status).color(MUTED).size(11.0));
                            ui.end_row();
                        }
                    });
                });
            }
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open { return; }
        let mut open = true;
        egui::Window::new("Tool Recap settings").open(&mut open).default_width(620.0).show(ctx, |ui| {
            ui.heading("Analysis API");
            field(ui, "Endpoint", &mut self.settings.api_endpoint);
            field(ui, "Model", &mut self.settings.api_model);
            ui.horizontal(|ui| {
                ui.label("API key");
                ui.add(egui::TextEdit::singleline(&mut self.settings.api_key).password(true));
            });
            ui.separator();
            ui.heading("VoiceStudio");
            field(ui, "Local URL", &mut self.settings.voicestudio_url);
            field(ui, "TTS engine", &mut self.settings.voice_model);
            field(ui, "TTS model repo", &mut self.settings.tts_model_repo);
            field(ui, "ASR model repo", &mut self.settings.asr_model_repo);
            field(ui, "Language", &mut self.settings.voice_language);
            field(ui, "Narration style", &mut self.settings.voice_style);
            ui.separator();
            field(ui, "Output folder", &mut self.settings.output_subdirectory);
            ui.checkbox(&mut self.settings.burn_subtitles, "Burn narration subtitles into rendered video");
            ui.checkbox(&mut self.settings.notify_complete, "Show completion notification");
            ui.checkbox(&mut self.settings.check_updates_on_start, "Check GitHub Releases on startup");
            if ui.button("Save settings").clicked() {
                match self.settings.save(&self.paths.settings) {
                    Ok(()) => self.success("Settings saved", "Preferences were saved in the portable data folder."),
                    Err(error) => self.error("Settings save failed", &error.to_string()),
                }
            }
        });
        self.settings_open = open;
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        let Some(update) = self.update_available.clone() else { return; };
        let mut open = true;
        egui::Window::new(format!("Update available — {}", update.tag)).open(&mut open).default_width(520.0).show(ctx, |ui| {
            ui.label(RichText::new(format!("Tool Recap {} is available.", update.version)).strong().size(16.0));
            if !update.notes.trim().is_empty() {
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| { ui.label(&update.notes); });
            }
            if self.update_downloading {
                ui.add(egui::ProgressBar::new(self.update_progress).show_percentage());
                ui.label(RichText::new(&self.update_status).color(MUTED));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.running && !self.update_downloading, egui::Button::new("Update and restart")).clicked() { self.apply_update(); }
                if ui.add_enabled(!self.update_downloading, egui::Button::new("Later")).clicked() { self.update_available = None; }
            });
            ui.label(RichText::new("Update packages are SHA-256 verified and applied through a rollback-capable helper. data/ is preserved.").color(MUTED).size(11.0));
        });
        if !open && !self.update_downloading { self.update_available = None; }
    }

    fn success(&mut self, title: &str, body: &str) { self.notice = Some(Notice { title: title.into(), body: body.into(), color: SUCCESS }); }
    fn warn(&mut self, title: &str, body: &str) { self.notice = Some(Notice { title: title.into(), body: body.into(), color: WARNING }); }
    fn error(&mut self, title: &str, body: &str) { self.notice = Some(Notice { title: title.into(), body: body.into(), color: DANGER }); }
}

impl eframe::App for RecapApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_events(ctx);
        if self.running || self.voices_loading || self.gpu_checking || self.update_checking || self.update_downloading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::new().fill(BG).inner_margin(16).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("TOOL RECAP").strong().size(22.0).color(TEXT));
                ui.label(RichText::new("RUST").strong().size(11.0).color(ACCENT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).color(MUTED));
                });
            });
            ui.label(RichText::new("Automated RTX video recap production with managed VoiceStudio narration").color(MUTED));
            ui.add_space(12.0);

            if let Some(notice) = self.notice.clone() {
                egui::Frame::group(ui.style()).stroke(Stroke::new(1.0, notice.color)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(notice.title).strong().color(notice.color));
                            ui.label(RichText::new(notice.body).color(TEXT));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.button(RichText::new("×").size(18.0)).on_hover_text("Dismiss notification").clicked() { self.notice = None; }
                        });
                    });
                });
                ui.add_space(8.0);
            }

            self.info_card(ui);
            ui.add_space(8.0);
            self.voice_card(ui);
            ui.add_space(8.0);
            self.queue_card(ui);
            ui.add_space(8.0);
            egui::Frame::group(ui.style()).fill(PANEL).stroke(Stroke::new(1.0, BORDER)).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.running && !self.queue.is_empty(), egui::Button::new(RichText::new("Start recap production").strong())).clicked() { self.start(); }
                    if ui.add_enabled(self.running, egui::Button::new(RichText::new("Stop").strong().color(DANGER))).clicked() { self.stop(); }
                    if ui.add_enabled(!self.running, egui::Button::new("Settings")).clicked() { self.settings_open = true; }
                    if ui.add_enabled(!self.running && !self.update_checking, egui::Button::new(if self.update_checking { "Checking…" } else { "Check updates" })).clicked() { self.check_updates(); }
                    ui.separator();
                    ui.label(RichText::new(&self.status).color(MUTED));
                });
            });
        });
        self.settings_window(ui.ctx());
        self.update_window(ui.ctx());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.settings.save(&self.paths.settings);
        self.cancel.cancel();
        self.voicestudio.shutdown();
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.selection.bg_fill = Color32::from_rgb(24, 78, 112);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 34.0;
    });
}

fn status_row(ui: &mut egui::Ui, label: &str, value: &str, good: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(if good { SUCCESS } else { WARNING }));
        ui.label(RichText::new(label).strong().color(TEXT));
        ui.label(RichText::new(value).color(MUTED).size(11.0));
    });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.text_edit_singleline(value);
    });
}

fn stage_color(stage: EpisodeStage) -> Color32 {
    match stage {
        EpisodeStage::Completed => SUCCESS,
        EpisodeStage::Failed => DANGER,
        EpisodeStage::Stopped => WARNING,
        EpisodeStage::Analyzing | EpisodeStage::Preparing | EpisodeStage::Rendering => ACCENT,
        EpisodeStage::Waiting => MUTED,
    }
}

#[cfg(windows)]
fn notify_native(title: &str, body: &str) {
    use tauri_winrt_notification::{Duration as ToastDuration, Toast};
    let _ = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .duration(ToastDuration::Short)
        .show();
}

#[cfg(not(windows))]
fn notify_native(_title: &str, _body: &str) {}

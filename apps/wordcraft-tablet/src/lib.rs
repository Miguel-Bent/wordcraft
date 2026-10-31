#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// `android_main` is the one export the Android glue looks up by name (`#[unsafe(no_mangle)]`),
// and `set_var` is `unsafe` in this edition; both run once, before the UI starts.
#![allow(unsafe_code)]
// Android only: the crate is empty on other hosts, so `cargo clippy --workspace` can check the rest.
#![cfg(target_os = "android")]
//! WordCraft on Android tablets.
//!
//! Runs the same [`wordcraft_ui_egui::WordApp`] as the desktop app, through eframe's Android
//! runner. The platform pieces are in `platform`: files and saves through the system picker,
//! the clipboard, and the settings folder in the app's private storage.

mod platform;

/// Diagnostics for a frozen UI. The frame marks the phase it is in, and a watchdog logs when one
/// phase outlasts [`STALL`]: the input dispatcher gives up on a window that doesn't answer a touch
/// within 10 seconds, so a stall that long is a freeze the user sees. The log names the phase.
mod stall {
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    const STALL: Duration = Duration::from_secs(3);

    static PHASE: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

    /// Marks `phase` as the frame's current phase until the guard drops.
    pub fn enter(phase: &'static str) -> Phase {
        *PHASE.lock().unwrap_or_else(PoisonError::into_inner) = Some((phase, Instant::now()));
        Phase
    }

    pub struct Phase;

    impl Drop for Phase {
        fn drop(&mut self) {
            *PHASE.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    /// Starts the watchdog thread, which logs a stall once and its end once.
    pub fn watch() {
        std::thread::spawn(|| {
            // The start of the phase whose stall was logged.
            let mut reported: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let now = Instant::now();
                let current = *PHASE.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(stalled) = reported
                    && current.map(|(_, since)| since) != Some(stalled)
                {
                    log::error!("UI resumed after {:.1}s", now.duration_since(stalled).as_secs_f32());
                    reported = None;
                }
                if let Some((phase, since)) = current
                    && reported.is_none()
                    && now.duration_since(since) >= STALL
                {
                    log::error!("UI stalled in {phase} for {:.1}s", now.duration_since(since).as_secs_f32());
                    reported = Some(since);
                }
            }
        });
    }
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use platform::Platform;
use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::{Session, sample};
use wordcraft_ui_egui::{Inbox, UiState, WordApp};

struct TabletApp {
    app: WordApp,
    platform: Platform,
    /// The settings folder (preferences, recovery copies, dictionary, fonts), if there is one.
    settings: Option<PathBuf>,
    /// The preferences last written, so they are only written when they change.
    saved_prefs: Option<Vec<u8>>,
}

impl eframe::App for TabletApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _phase = stall::enter("logic");
        // Files and save results the activity reported since the last frame.
        self.platform.apply_events();
        self.app.logic(ctx);
        self.save_prefs_if_changed();
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let _phase = stall::enter("raw input");
        // With clipboard shortcuts off (see `android_main`), Ctrl+C, Ctrl+X and Ctrl+V arrive as
        // keys. A text field (egui's own) needs the copy, cut and paste events the desktop sends;
        // the document doesn't: its shortcuts already run those commands from the key itself, and
        // a second event would copy, cut or paste twice.
        // The document's keys are commands already; only another text field needs the events.
        if self.app.editing_document(ctx) || !ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut events = Vec::new();
        for event in &raw.events {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
            if !(modifiers.command || modifiers.ctrl) {
                continue;
            }
            match key {
                egui::Key::C => events.push(egui::Event::Copy),
                egui::Key::X => events.push(egui::Event::Cut),
                egui::Key::V => {
                    if let Some(text) = platform::clipboard_text().filter(|text| !text.is_empty()) {
                        events.push(egui::Event::Paste(text));
                    }
                }
                _ => {}
            }
        }
        raw.events.extend(events);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _phase = stall::enter("ui");
        self.app.ui(ui);
        // egui has no clipboard on Android: its copies go to the system clipboard here.
        // Copies are output commands; taking them here keeps egui from handling them too.
        let copies: Vec<String> = ui.ctx().output_mut(|output| {
            let (copies, rest): (Vec<_>, Vec<_>) =
                std::mem::take(&mut output.commands).into_iter().partition(|command| matches!(command, egui::OutputCommand::CopyText(_)));
            output.commands = rest;
            copies
                .into_iter()
                .filter_map(|command| match command {
                    egui::OutputCommand::CopyText(text) => Some(text),
                    _ => None,
                })
                .collect()
        });
        for text in copies {
            platform::copy_text(&text);
        }
    }
}

impl TabletApp {
    fn save_prefs_if_changed(&mut self) {
        let Some(dir) = &self.settings else { return };
        let Ok(bytes) = serde_json::to_vec_pretty(&self.app.prefs()) else { return };
        if self.saved_prefs.as_deref() == Some(bytes.as_slice()) {
            return;
        }
        match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(dir.join("ui.json"), &bytes)) {
            Ok(()) => self.saved_prefs = Some(bytes),
            Err(error) => log::warn!("preferences: cannot save: {error}"),
        }
    }
}

/// Points the engine's settings folder at the app's private storage, where it keeps the
/// preferences, the user dictionary, the user's fonts and the recovery copies.
fn use_private_settings(app: &AndroidApp) -> Option<PathBuf> {
    let dir = app.internal_data_path()?;
    // SAFETY: set once at start-up, before any other thread of this process reads the environment.
    unsafe { std::env::set_var("WORDCRAFT_PROFILE_DIR", &dir) };
    Some(dir)
}

/// Loads what the desktop loads at start-up from the settings folder: the user's fonts, the
/// preferences, the user dictionary, and it turns on the recovery copies.
fn load_settings(app: &mut WordApp, ctx: &egui::Context, dir: &Path) {
    let fonts = dir.join("fonts");
    let (loaded, skipped) = wordcraft_fonts::FontDb::global().load_user_fonts(&fonts);
    if !loaded.is_empty() || !skipped.is_empty() {
        log::info!("user fonts: {} loaded, {} skipped", loaded.len(), skipped.len());
    }

    if let Ok(bytes) = std::fs::read(dir.join("ui.json"))
        && let Some(ui) = UiState::from_json(&bytes)
    {
        app.apply_prefs(ui);
    }

    let dictionary = dir.join(Session::USER_DICT_FILE);
    wordcraft_proof::load_user_dictionary(&dictionary);
    app.session.user_dict_path = Some(dictionary);

    let repaint = ctx.clone();
    if let Err(error) = app.session.enable_recovery(dir.join("recovery"), Arc::new(move || repaint.request_repaint())) {
        log::warn!("local recovery unavailable: {error}");
    }
}

/// The Android entry point, called by the activity once the window exists.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Debug).with_tag("wordcraft"));
    stall::watch();
    let settings = use_private_settings(&app);
    platform::set_activity(app.clone());
    let mut options = eframe::NativeOptions {
        android_app: Some(app),
        // egui has no clipboard on Android: Ctrl+C/X/V then arrive as keys, and the app handles
        // them below with the system clipboard.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    // Debug builds ask wgpu for Vulkan debug utilities, which phone and tablet drivers often lack;
    // the loader then panics. Release builds don't ask, so turn that off everywhere on Android.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.instance_descriptor.flags = eframe::wgpu::InstanceFlags::empty();
    }
    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(move |creation| {
            let inbox = Inbox::default();
            let platform = Platform::new(&creation.egui_ctx, inbox);
            let session = Session::new(sample::sample_document());
            let mut app = WordApp::new(session, platform.services());
            if let Some(dir) = &settings {
                load_settings(&mut app, &creation.egui_ctx, dir);
            }
            Ok(Box::new(TabletApp { app, platform, settings, saved_prefs: None }))
        }),
    );
    if let Err(error) = result {
        log::error!("WordCraft stopped: {error}");
    }
}

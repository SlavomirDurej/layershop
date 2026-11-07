//! Window › Timeline: a floating panel that creates and drives the document timeline. All actions
//! run the engine `timeline.*` commands, so the control channel drives it the same way.

use egui::{RichText, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// Timeline panel view state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineUi {
    pub open: bool,
    /// Playing back (auto-advancing the playhead at the frame rate).
    #[serde(default)]
    pub playing: bool,
    /// egui time (seconds) of the last auto-advance.
    #[serde(default, skip)]
    pub last_time: f64,
}

/// Menu checkmark for Window › Timeline.
pub fn checked(app: &PhotocraftApp, id: &str) -> Option<bool> {
    (id == "window.panel.timeline").then_some(app.ui.timeline.open)
}

/// Ids this module owns (for parity / enablement).
pub fn handles(id: &str) -> bool {
    id == "window.panel.timeline"
}

/// Toggle the Timeline panel.
pub fn menu(app: &mut PhotocraftApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    if id == "window.panel.timeline" {
        app.ui.timeline.open = !app.ui.timeline.open;
        return Some(Ok(json!({ "open": app.ui.timeline.open })));
    }
    None
}

/// A frame rate as the panel shows it: whole rates as they are, others to one decimal (3.3).
fn fps_label(fps: f32) -> String {
    if (fps - fps.round()).abs() < 0.05 { format!("{:.0}", fps) } else { format!("{:.1}", fps) }
}

/// Export GIF…: ask where, then render the timeline there as one looping GIF
/// (`file.export.renderVideo` with `format: gif`).
pub fn save_gif(app: &mut PhotocraftApp) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Err("Export GIF to a local file is unavailable on the web".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let st = app.session.active().ok_or("no document")?;
        let doc = st.doc.id;
        let source = st.path.clone().unwrap_or_else(|| st.doc.name.clone());
        let suggested = std::path::Path::new(&source).with_extension("gif").to_string_lossy().into_owned();
        app.pick_save(&suggested, move |app, path| {
            let file = std::path::Path::new(&path);
            let dir = file.parent().map(|d| d.to_string_lossy().into_owned()).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".into());
            let name = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            app.with_document(doc, |app| app.run("file.export.renderVideo", json!({"dir": dir, "name": name, "format": "gif"})))
        })
    }
}

/// Render the Timeline panel.
pub fn windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    // Playback started elsewhere (the AI Gen panel's Play) runs without the panel.
    if !app.ui.timeline.open {
        playback(app, ctx);
        return;
    }
    let tl = app.session.active().and_then(|d| d.doc.timeline.clone());
    let have_doc = app.session.active().is_some();
    // Video layers left without a timeline (it was deleted): a new one fits them.
    let videos = app.session.active().map_or(0, |d| d.doc.walk().iter().filter(|(_, _, l)| l.video.is_some()).count());
    let mut act: Option<(&str, Value)> = None;
    let mut close = false;
    let mut toggle_play = false;
    let mut stop_play = false;
    let mut export_gif = false;
    let playing = app.ui.timeline.playing;
    let t = Tokens::get(ctx);

    crate::analysis_ui::panel_window(app, ctx, "timeline", tl!("Timeline"), vec2(0.0, 520.0), 660.0, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Timeline")).strong().color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::icons::button(ui, "x", 20.0, false, tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
        ui.separator();

        if !have_doc {
            ui.weak(tl!("Open a document to use the timeline."));
            return;
        }
        match &tl {
            None => {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if crate::widgets::primary_button(ui, tl!("Create Video Timeline"), 200.0).clicked() {
                        // Sized for the video layers there are (timeline.create's defaults), else 30 @ 30.
                        act = Some(("timeline.create", if videos > 0 { json!({}) } else { json!({ "duration": 30, "fps": 30.0 }) }));
                    }
                    ui.add_space(4.0);
                    if videos > 0 {
                        let n = videos.to_string();
                        ui.label(
                            RichText::new(crate::i18n::fmt(
                                &crate::i18n::trn(
                                    crate::i18n::current(),
                                    videos as u64,
                                    "{n} video layer doesn't play without a timeline: create one for it",
                                    "{n} video layers don't play without a timeline: create one for them",
                                ),
                                &[("n", &n)],
                            ))
                            .color(t.warning),
                        );
                    } else {
                        ui.weak(tl!("30 frames @ 30 fps"));
                    }
                });
            }
            Some(tl) => {
                ui.horizontal(|ui| {
                    let play_icon = if playing { "pause" } else { "play" };
                    if crate::icons::button(ui, play_icon, 22.0, playing, if playing { tl!("Pause") } else { tl!("Play") }).clicked() {
                        toggle_play = true;
                    }
                    if crate::icons::button(ui, "square", 22.0, false, tl!("Stop")).clicked() {
                        if !playing {
                            act = Some(("timeline.setFrame", json!({ "frame": 0 })));
                        }
                        stop_play = true;
                    }
                    if crate::icons::button(ui, "chevron-left", 22.0, false, tl!("Previous frame")).clicked() {
                        stop_play = true;
                        act = Some(("timeline.previousFrame", json!({})));
                    }
                    ui.label(
                        RichText::new(crate::i18n::fmt(
                            crate::i18n::t("Frame {current} / {total}"),
                            &[("current", &(tl.current + 1).to_string()), ("total", &tl.duration.to_string())],
                        ))
                        .color(t.text)
                        .monospace(),
                    );
                    if crate::icons::button(ui, "chevron-right", 22.0, false, tl!("Next frame")).clicked() {
                        stop_play = true;
                        act = Some(("timeline.nextFrame", json!({})));
                    }
                    ui.separator();
                    ui.label(RichText::new(format!("{:.2}s", tl.time())).color(t.text_dim).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tip = if videos > 0 {
                            tl!("Delete Timeline: the video layers stay, but don't play until a timeline is made again")
                        } else {
                            tl!("Delete Timeline")
                        };
                        if crate::icons::button(ui, "trash", 20.0, false, tip).clicked() {
                            stop_play = true;
                            act = Some(("timeline.delete", json!({})));
                        }
                        ui.label(RichText::new(format!("{} fps", fps_label(tl.fps))).color(t.text_dim));
                        if ui.button(tl!("Export GIF…")).on_hover_text(tl!("Save the timeline as a looping animated GIF")).clicked() {
                            stop_play = true;
                            export_gif = true;
                        }
                    });
                });
                ui.add_space(6.0);
                // Scrubber: frame playhead across the duration.
                if tl.duration > 1 {
                    let mut frame = tl.current as f64;
                    let resp = ui.add(egui::Slider::new(&mut frame, 0.0..=(tl.duration - 1) as f64).integer().show_value(false).text("playhead"));
                    if resp.changed() {
                        stop_play = true;
                        act = Some(("timeline.setFrame", json!({ "frame": frame as u64 })));
                    }
                }
            }
        }
    });

    if close {
        app.ui.timeline.open = false;
        app.ui.timeline.playing = false;
    }
    if stop_play {
        app.ui.timeline.playing = false;
    } else if toggle_play {
        app.ui.timeline.playing = !app.ui.timeline.playing;
        app.ui.timeline.last_time = ctx.input(|i| i.time);
    }
    if let Some((cmd, p)) = act {
        let _ = app.run(cmd, p);
    }
    if export_gif && let Err(e) = save_gif(app) {
        crate::notices::error(app, format!("{}: {e}", tl!("Export GIF")));
    }
    playback(app, ctx);
}

/// Playback: auto-advance the playhead at the frame rate (loops).
fn playback(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.ui.timeline.playing {
        if let Some(t) = app.session.active().and_then(|d| d.doc.timeline.clone()) {
            let now = ctx.input(|i| i.time);
            let period = 1.0 / f64::from(t.fps.max(1.0));
            if now - app.ui.timeline.last_time >= period {
                app.ui.timeline.last_time = now;
                let next = (t.current + 1) % t.duration.max(1);
                let _ = app.run("timeline.setFrame", json!({ "frame": next as u64 }));
            }
            ctx.request_repaint();
        } else {
            app.ui.timeline.playing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn stop_step_and_scrub_pause_playback_and_second_stop_rewinds() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 100, "height": 80})).unwrap();
        app.run("timeline.create", json!({"duration": 10, "fps": 30.0})).unwrap();
        app.ui.timeline.open = true;
        app.last_canvas_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 700.0));

        let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).with_step_dt(0.001).build_ui_state(|ui, app| windows(app, ui.ctx()), app);
        h.run_steps(2);

        // Start playback via Play button.
        h.get_by_label("Play").click();
        h.run_steps(2);
        assert!(h.state().ui.timeline.playing);

        // Move to frame 5 while playing; first Stop pauses at the current frame without rewinding.
        h.state_mut().run("timeline.setFrame", json!({"frame": 5})).unwrap();
        h.get_by_label("Stop").click();
        h.run_steps(2);
        assert!(!h.state().ui.timeline.playing, "first Stop pauses playback");
        assert_eq!(h.state().session.active().unwrap().doc.timeline.as_ref().unwrap().current, 5);

        // Second Stop while already paused rewinds to frame 0.
        h.get_by_label("Stop").click();
        h.run_steps(2);
        assert!(!h.state().ui.timeline.playing);
        assert_eq!(h.state().session.active().unwrap().doc.timeline.as_ref().unwrap().current, 0);

        // Next frame and Previous frame pause playback.
        h.state_mut().ui.timeline.playing = true;
        h.run_steps(1);
        h.get_by_label("Next frame").click();
        h.run_steps(2);
        assert!(!h.state().ui.timeline.playing, "Next frame pauses playback");
        assert_eq!(h.state().session.active().unwrap().doc.timeline.as_ref().unwrap().current, 1);

        h.state_mut().ui.timeline.playing = true;
        h.run_steps(1);
        h.get_by_label("Previous frame").click();
        h.run_steps(2);
        assert!(!h.state().ui.timeline.playing, "Previous frame pauses playback");
        assert_eq!(h.state().session.active().unwrap().doc.timeline.as_ref().unwrap().current, 0);

        // Scrubbing the playhead slider pauses playback and updates the current frame.
        h.state_mut().ui.timeline.playing = true;
        h.run_steps(1);
        let slider = h.get_by_role(egui::accesskit::Role::Slider).rect();
        let target = egui::pos2(slider.right() - 8.0, slider.center().y);
        h.event(egui::Event::PointerMoved(target));
        h.event(egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        h.event(egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
        assert!(!h.state().ui.timeline.playing, "scrubbing pauses playback");
        assert!(h.state().session.active().unwrap().doc.timeline.as_ref().unwrap().current > 0);
    }
}

#[cfg(test)]
mod export_gif_tests {
    use super::*;
    use crate::file_dialog::{FileDialogAnswer, FileDialogRequest};
    use egui_kittest::{Harness, kittest::Queryable};

    /// Export GIF… asks for a .gif beside the document and writes the timeline there as one GIF.
    #[test]
    fn export_gif_asks_where_and_writes_one_gif() {
        let dir = std::env::temp_dir().join(format!("photocraft-export-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("fine.gif").to_string_lossy().replace('\\', "/");
        let (show, asked) = crate::file_dialog::fake(vec![Some(FileDialogAnswer::SaveTo(target.clone()))]);
        let mut h = Harness::builder().with_size(vec2(1440.0, 900.0)).with_max_steps(64).build_eframe(move |cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
            PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services { file_dialog: Some(show), ..Default::default() })
        });
        h.state_mut().run("file.new", json!({"width": 16, "height": 16})).unwrap();
        h.state_mut().run("timeline.create", json!({"duration": 3, "fps": 3.3333})).unwrap();
        h.state_mut().ui.timeline.open = true;
        h.state_mut().sync_views();
        h.run_steps(4);
        assert!(h.query_by_label("3.3 fps").is_some(), "a rate between whole numbers keeps its decimal");
        h.get_by_label("Export GIF…").click();
        h.run_steps(6);
        assert!(matches!(asked.borrow().as_slice(), [FileDialogRequest::Save { .. }]), "{:?}", asked.borrow());
        let gif = std::fs::read(&target).unwrap();
        assert_eq!(&gif[..6], b"GIF89a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rates_read_as_numbers() {
        assert_eq!((fps_label(30.0), fps_label(3.3333), fps_label(24.97)), ("30".into(), "3.3".into(), "25".into()));
    }
}

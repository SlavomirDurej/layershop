use super::{GENERATE, PhotocraftApp, tr, widgets};
use photocraft_doc::{LayerContent, LayerId};
use photocraft_engine::{
    ai_upscale_cmds::{output_plan, source_size},
    jobs::{JobEvent, JobId, JobOutcome, Started},
};
use serde_json::{Value, json};

const COMMAND: &str = "ai.upscale";
type Models = Vec<(String, Vec<u32>)>;
type Source = Result<(LayerId, (u32, u32)), String>;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct UpscaleUi {
    model: String,
    scale: u32,
    inventory: Option<Result<Models, String>>,
    server: Option<(String, u64)>,
    scan: Option<JobId>,
    job: Option<JobId>,
    outcome: Option<(bool, String)>,
    // Computing exact bounds can read tiles. Only repeat it when the document or target changes.
    source: Option<((u64, u64, u64), Source)>,
}

impl Default for UpscaleUi {
    fn default() -> Self {
        Self { model: String::new(), scale: 2, inventory: None, server: None, scan: None, job: None, outcome: None, source: None }
    }
}

impl UpscaleUi {
    fn models(&self) -> &[(String, Vec<u32>)] {
        self.inventory.as_ref().and_then(|r| r.as_ref().ok()).map_or(&[], Vec::as_slice)
    }

    fn scales(&self) -> &[u32] {
        self.models().iter().find(|(name, _)| *name == self.model).map_or(&[], |(_, scales)| scales.as_slice())
    }

    fn choose_model(&mut self, name: String) {
        self.model = name;
        if !self.scales().contains(&self.scale)
            && let Some(&first) = self.scales().first()
        {
            self.scale = first;
        }
    }

    fn discovered(&mut self, result: Result<Value, String>) {
        let first_choice = self.model.is_empty();
        self.inventory =
            Some(result.and_then(|v| serde_json::from_value(v["upscaleModels"].clone()).map_err(|_| tr("Invalid upscale model inventory.").into())));
        let choice = if first_choice { self.models().first().map(|(name, _)| name.clone()).unwrap_or_default() } else { self.model.clone() };
        self.choose_model(choice);
    }
}

fn scan(app: &mut PhotocraftApp, server: (String, u64)) {
    let state = &mut app.ui.ai_gen.upscale;
    state.server = Some(server);
    state.scan = None;
    state.inventory = None;
    let params = json!({"upscaleOnly": true, "start": false});
    let result =
        if app.background_jobs { app.session.start("ai.comfyModels", params) } else { app.session.execute("ai.comfyModels", params).map(Started::Done) };
    match result {
        Ok(Started::Job(id)) => state.scan = Some(id),
        Ok(Started::Done(value)) => state.discovered(Ok(value)),
        Err(error) => state.discovered(Err(error.to_string())),
    }
}

pub(super) fn on_job_event(app: &mut PhotocraftApp, e: &JobEvent) -> bool {
    let state = &mut app.ui.ai_gen.upscale;
    if state.scan == Some(e.id) {
        state.scan = None;
        let result = match &e.outcome {
            JobOutcome::Done(value) => Ok(value.clone()),
            JobOutcome::Failed(error) => Err(error.clone()),
            JobOutcome::Cancelled => Err(tr("Model refresh canceled.").into()),
        };
        state.discovered(result);
        return true;
    }
    if state.job == Some(e.id) && e.command == COMMAND {
        state.job = None;
        state.outcome = Some(match &e.outcome {
            JobOutcome::Done(_) => (false, tr("Upscale complete.").into()),
            JobOutcome::Failed(error) => (true, error.clone()),
            JobOutcome::Cancelled => (false, tr("Upscale canceled.").into()),
        });
    }
    // The shared job handler still syncs the canvas, layers and notices.
    false
}

fn selection(app: &mut PhotocraftApp) -> Source {
    let session = &app.session;
    let doc = session.active().ok_or_else(|| tr("No layer selected.").to_owned())?;
    let selected = doc.selected_layers();
    let [id] = selected.as_slice() else {
        return Err(tr(if selected.is_empty() { "No layer selected." } else { "Select a single raster layer to upscale." }).into());
    };
    if let Some(layer) = doc.doc.layer(*id) {
        match &layer.content {
            LayerContent::Raster(_) => {}
            LayerContent::Smart(_) => {
                return Err(tr("The selected layer is a Smart Object. Select the original raster layer in the Layers panel to upscale again.").into());
            }
            content => {
                return Err(format!("{} {}", content.kind_name(), tr("layers cannot be upscaled. Select a raster layer in the Layers panel.")));
            }
        }
    }
    let key = (session.active_generation(), doc.revision, session.active_layer_changes());
    let cached = &mut app.ui.ai_gen.upscale.source;
    if let Some((cached_key, value)) = cached
        && *cached_key == key
    {
        return value.clone();
    }
    let value = source_size(session, *id).map(|size| (*id, size)).map_err(|error| error.to_string());
    *cached = Some((key, value.clone()));
    value
}

fn server(app: &PhotocraftApp) -> (String, u64) {
    (app.session.prefs().integrations.ai.comfy_url.clone(), app.session.ai_setup.seq())
}

fn ready(app: &PhotocraftApp, source: (LayerId, (u32, u32))) -> Result<(u32, u32), String> {
    if let Some(reason) = app.session.disabled_reason(COMMAND) {
        return Err(reason);
    }
    if app.session.jobs().iter().any(|job| matches!(job.command.as_str(), COMMAND | GENERATE)) {
        return Err(tr("Wait for the active AI job to finish.").into());
    }
    let state = &app.ui.ai_gen.upscale;
    if state.server.as_ref() != Some(&server(app)) || state.inventory.is_none() {
        return Err(tr("Checking ComfyUI for upscale models…").into());
    }
    if let Some(Err(error)) = &state.inventory {
        return Err(format!("{} {error}", tr("Could not check upscale models on ComfyUI.")));
    }
    if state.models().is_empty() {
        return Err(tr("No supported upscale models found on ComfyUI.").into());
    }
    if state.scales().is_empty() {
        return Err(tr("The selected model is no longer available. Choose another model.").into());
    }
    if !state.scales().contains(&state.scale) {
        return Err(tr("This model does not support the selected scale.").into());
    }
    output_plan(&state.model, state.scale, source.1).map(|plan| plan.output()).map_err(|error| error.to_string())
}

pub(super) fn start(app: &mut PhotocraftApp) -> Result<Value, String> {
    let source = selection(app)?;
    ready(app, source)?;
    let state = &app.ui.ai_gen.upscale;
    let params = json!({"layer":source.0.0,"model":state.model,"scale":state.scale});
    let result = crate::jobs_ui::run(app, COMMAND, params);
    let state = &mut app.ui.ai_gen.upscale;
    match &result {
        Ok(value) if value["pending"] == true => {
            state.job = value["job"].as_u64().map(JobId);
            state.outcome = None;
        }
        Ok(_) => {
            state.outcome = Some((false, tr("Upscale complete.").into()));
            app.sync_views();
        }
        Err(error) => state.outcome = Some((true, error.clone())),
    }
    result
}

pub(super) fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui, entered: bool) {
    let endpoint = server(app);
    if app.ui.ai_gen.upscale.scan.is_none() && (entered || app.ui.ai_gen.upscale.server.as_ref() != Some(&endpoint)) {
        scan(app, endpoint.clone());
    }

    let source = selection(app);
    if let Some(layer) = app.session.active().and_then(|doc| doc.active_layer.and_then(|id| doc.doc.layer(id))) {
        ui.label(egui::RichText::new(&layer.name).strong());
    }
    if let Ok((_, (w, h))) = &source {
        ui.label(format!("{} {w} × {h} px", tr("Original:")));
        ui.label(tr("Raster layer ready"));
    } else if let Err(reason) = &source {
        ui.label(reason);
    }

    let job = app.session.jobs().into_iter().find(|job| job.command == COMMAND);
    let state = &mut app.ui.ai_gen.upscale;
    let mut chosen = state.model.clone();
    ui.add_enabled_ui(job.is_none(), |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Model"));
            let width = ui.available_width();
            ui.add_enabled_ui(!state.models().is_empty(), |ui| {
                craft_ui::ComboBox::from_id_salt("upscale-model")
                    .width(width)
                    .truncate()
                    .selected_text(if state.scales().is_empty() { tr("Choose a model") } else { &state.model })
                    .show_ui(ui, |ui| {
                        for (name, _) in state.models() {
                            ui.selectable_value(&mut chosen, name.clone(), name);
                        }
                    });
            });
        });
        if chosen != state.model {
            state.choose_model(chosen);
        }
        ui.horizontal(|ui| {
            ui.label(tr("Scale"));
            for (scale, title) in [(2, "2×"), (4, "4×")] {
                ui.add_enabled_ui(state.scales().contains(&scale), |ui| {
                    ui.selectable_value(&mut state.scale, scale, title);
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(state.scan.is_none(), egui::Button::new(tr("Refresh"))).clicked() {
                    state.server = None;
                }
            });
        });
    });
    if state.server.is_none() {
        scan(app, endpoint);
    }

    let checked = source.clone().and_then(|source| ready(app, source));
    if let Ok((w, h)) = checked {
        ui.label(format!("{} {w} × {h} px", tr("Upscaled:")));
    }
    if let Some(job) = job {
        let message = if job.message.is_empty() { tr("Starting upscale…") } else { &job.message };
        ui.label(format!("{message} ({:.1} s)", job.elapsed_ms / 1000.0));
        if widgets::secondary_button(ui, tr("Cancel"), 70.0).clicked() {
            crate::jobs_ui::cancel(app, job.id);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        return;
    }
    if let Some((error, message)) = &app.ui.ai_gen.upscale.outcome {
        let color = super::Tokens::get(ui.ctx());
        ui.colored_label(if *error { color.danger } else { color.text }, message);
    }
    if let Err(reason) = &checked
        && source.is_ok()
    {
        ui.label(reason);
    }
    let click = ui.add_enabled_ui(checked.is_ok(), |ui| widgets::primary_button(ui, tr("Upscale Selected Layer"), 160.0)).inner.clicked();
    if click && let Err(error) = start(app) {
        app.ui.ai_gen.upscale.outcome = Some((true, error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_color::{ColorMode, SampleType};
    use photocraft_doc::{Document, Layer};
    use photocraft_geom::{Rect, Size};
    use photocraft_raster::Surface;

    const X2: &str = "RealESRGAN_x2plus.pth";
    const X4: &str = "weights/RealESRGAN_x4plus.safetensors";

    #[test]
    fn inventory_refresh_preserves_choices_and_disables_missing_models() {
        let mut state = UpscaleUi::default();
        state.discovered(Ok(json!({"upscaleModels":[[X2,[2]],[X4,[2,4]]]})));
        assert_eq!((state.model.as_str(), state.scales()), (X2, &[2][..]));
        state.choose_model(X4.into());
        state.scale = 4;
        state.discovered(Err("connection refused".into()));
        assert!(state.scales().is_empty());
        state.discovered(Ok(json!({"upscaleModels":[[X4,[2,4]]]})));
        assert_eq!(state.scale, 4);
        state.discovered(Ok(json!({"upscaleModels":[[X2,[2]]]})));
        assert!(state.scales().is_empty());
        state.choose_model(X2.into());
        assert_eq!(state.scale, 2);
        state.discovered(Ok(json!({"upscaleModels":[]})));
        assert!(state.models().is_empty());
    }

    #[test]
    fn selection_and_preview_follow_full_bounds_and_engine_limits() {
        let mut app = super::super::tests::app();
        assert_eq!(selection(&mut app).unwrap_err(), "No layer selected.");
        let mut doc = Document::new("Fixture", Size::new(8, 8), ColorMode::Rgb, SampleType::U8);
        let mut pixels = Surface::new(doc.pixel_format());
        pixels.fill_rect(Rect::new(-5, 2, 6, 9), &[1.0, 0.0, 0.0, 1.0]);
        let source = Layer::new("Source", LayerContent::Raster(pixels));
        let id = source.id;
        doc.layers.push(source);
        app.session.add_document(doc, None);
        app.session.select_layer(id).unwrap();
        let input = selection(&mut app).unwrap();
        assert_eq!(input, (id, (11, 7)));

        let endpoint = server(&app);
        let state = &mut app.ui.ai_gen.upscale;
        state.server = Some(endpoint);
        state.discovered(Ok(json!({"upscaleModels":[[X2,[2]],[X4,[2,4]]]})));
        assert_eq!(ready(&app, input).unwrap(), (22, 14));
        app.ui.ai_gen.upscale.scale = 4;
        assert!(ready(&app, input).is_err());
        app.ui.ai_gen.upscale.choose_model(X4.into());
        assert_eq!(ready(&app, input).unwrap(), (44, 28));
        app.ui.ai_gen.upscale.scale = 2;
        assert_eq!(ready(&app, input).unwrap(), (22, 14));
        assert!(ready(&app, (id, (u32::MAX, 7))).is_err());

        app.run("layer.new.layer", json!({})).unwrap();
        assert!(selection(&mut app).unwrap_err().contains("no pixels"));
        app.run("layer.new.group", json!({})).unwrap();
        assert!(selection(&mut app).unwrap_err().contains("Group layers cannot be upscaled"));
    }

    #[test]
    fn selecting_the_upscale_result_explains_how_to_return_to_the_original() {
        use photocraft_codecs::{ChannelLayout, Format, Image};
        use photocraft_engine::jobs::JobCtx;

        let mut app = super::super::tests::app();
        app.run("file.new", json!({"width": 8, "height": 8})).unwrap();
        let (source, original_size) = selection(&mut app).unwrap();
        let image = Image::from_u8(16, 16, ChannelLayout::Rgba, vec![255; 16 * 16 * 4]).unwrap();
        let png = photocraft_codecs::encode(&image, Format::Png, &Default::default()).unwrap();
        let result = app.session.place_upscaled_raster(source, png, Size::new(16, 16), 2, &JobCtx::default()).unwrap();
        assert_eq!(app.session.active().unwrap().active_layer, Some(result));
        let error = start(&mut app).unwrap_err();
        assert!(error.contains("Smart Object") && error.contains("original raster layer"));
        app.session.select_layer(source).unwrap();
        assert_eq!(selection(&mut app).unwrap(), (source, original_size));
    }

    #[test]
    fn job_events_follow_the_job_even_when_another_tab_is_open() {
        let mut app = super::super::tests::app();
        for (outcome, expected) in [
            (JobOutcome::Done(json!({"layer": 19})), (false, "Upscale complete.")),
            (JobOutcome::Failed("invalid output".into()), (true, "invalid output")),
            (JobOutcome::Cancelled, (false, "Upscale canceled.")),
        ] {
            let mut event = JobEvent { id: JobId(8), command: COMMAND.into(), label: "Upscale".into(), document: None, outcome, error: None };
            app.ui.ai_gen.mode = super::super::Mode::Edit;
            app.ui.ai_gen.upscale.job = Some(JobId(9));
            assert!(!on_job_event(&mut app, &event));
            assert_eq!(app.ui.ai_gen.upscale.job, Some(JobId(9)));
            event.id = JobId(9);
            assert!(!on_job_event(&mut app, &event), "normal job completion must still sync layers");
            assert_eq!(app.ui.ai_gen.upscale.job, None);
            assert_eq!(app.ui.ai_gen.upscale.outcome.take(), Some((expected.0, expected.1.into())));
        }
    }
}

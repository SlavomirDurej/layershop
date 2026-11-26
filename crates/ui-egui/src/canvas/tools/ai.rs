//! LayerShop's AI overlays on the canvas: while AI Gen's Regions boxes are being edited or Remove BG's dots placed
//! (`ai_gen_ui::wants_pointer`), this tool takes the pointer in place of the selected tool, as a dialog's pick does,
//! and hands each press, drag and release to them in document units.

use craft::behavior::cursor::Cursor;
use craft::behavior::value::CancelReason;
use craft::keymap::Mods;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;

use super::dialog::doc_point;
use super::{FamilyTool, ToolScope};
use crate::ai_gen_ui::{self, CanvasEvent};
use crate::canvas::document_display::CanvasHost;

/// The AI overlays' tool for the canvas `host`, when they want the pointer and `host` shows the active document.
pub(crate) fn ai_tool<'a>(scope: &'a ToolScope<'a>, host: CanvasHost) -> Option<FamilyTool<'a>> {
    scope.with_app(|app| app.session.active_index() == Some(host.idx) && ai_gen_ui::wants_pointer(app)).then(|| Box::new(AiTool { scope }) as FamilyTool<'a>)
}

/// egui's modifiers for the framework's.
fn egui_mods(mods: Mods) -> egui::Modifiers {
    egui::Modifiers { alt: mods.alt, ctrl: mods.ctrl, shift: mods.shift, mac_cmd: false, command: mods.cmd }
}

struct AiTool<'a> {
    scope: &'a ToolScope<'a>,
}

impl AiTool<'_> {
    fn send(&self, ev: CanvasEvent, pointer: &ToolPointer) {
        let (mods, clicks) = (egui_mods(pointer.mods), pointer.clicks);
        self.scope.with_app(|app| ai_gen_ui::pointer(app, ev, mods, clicks));
    }
}

impl CanvasTool for AiTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        let [x, y] = doc_point(pointer, cx);
        self.send(CanvasEvent::Down { x, y }, pointer);
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let [x, y] = doc_point(pointer, cx);
        self.send(CanvasEvent::Move { x, y }, pointer);
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let [x, y] = doc_point(pointer, cx);
        self.send(CanvasEvent::Up { x, y }, pointer);
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.scope.with_app(ai_gen_ui::regions::cancel_drag);
    }

    fn view_only(&self) -> bool {
        false
    }

    /// A box follows the pointer from the press, so a move of a pixel or two moves it.
    fn takes_every_move(&self) -> bool {
        true
    }

    /// The boxes and dots paint themselves over the canvas (`ai_gen_ui::paint_canvas`).
    fn paint(&self, _draw: &Draw<'_>, _cx: &ToolContext<'_>) {}

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        Cursor::System(craft::behavior::cursor::CursorIcon::Default)
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Freehand("AI Gen's regions and Remove BG's points are edited in the AI Gen panel")
    }
}

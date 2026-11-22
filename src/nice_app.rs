//! nice-plug-egui glue: runs an awk studios editor frame closure as a `NiceEguiApp`.
//! The closure owns the editor state; the GUI context only exists while the window is open.
use nice_plug::context::gui::GuiContext;
use nice_plug::prelude::ParamSetter;
use nice_plug_egui::{baseview::HandlerError, EguiEditor, Frame, NiceEguiApp};

pub type FrameFn = Box<dyn FnMut(&mut egui::Ui, &ParamSetter) + Send>;
pub type AwkEditor = EguiEditor<AwkApp>;

pub struct AwkApp {
    fonts: fn(&egui::Context),
    frame: FrameFn,
    gui: Option<GuiContext>,
}
impl AwkApp {
    pub fn new(fonts: fn(&egui::Context), frame: FrameFn) -> Self {
        Self { fonts, frame, gui: None }
    }
}
impl NiceEguiApp for AwkApp {
    fn build(&mut self, egui_ctx: egui::Context, gui: GuiContext, _f: &mut Frame) -> Result<(), HandlerError> {
        (self.fonts)(&egui_ctx);
        self.gui = Some(gui);
        Ok(())
    }
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut Frame) {
        let Some(gui) = self.gui.as_ref() else { return };
        let setter = gui.param_setter();
        (self.frame)(ui, &setter);
    }
    fn editor_closed(&mut self) {
        self.gui = None;
    }
}

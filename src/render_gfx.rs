use crate::errors::Result;
use crate::render::{RenderBackend, RenderParams, RenderPipelineHandle};

pub struct GfxBackend;

impl RenderBackend for GfxBackend {
    fn init_window(&mut self, window: &winit::window::Window) {
        todo!()
    }
    fn render_frame(&mut self, params: RenderParams<'_>) {
        todo!()
    }

    fn new_pipeline(&self, shader_path: &str, shader_source: &str) -> Result<RenderPipelineHandle> {
        todo!()
    }
}

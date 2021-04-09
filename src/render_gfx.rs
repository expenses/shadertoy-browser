use crate::errors::Result;
use crate::render::{RenderBackend, RenderParams, RenderPipelineHandle};

use gfx_hal::{
    self as hal,
    adapter::{Adapter, PhysicalDevice},
    device::Device,
    format::{ChannelType, Format},
    image::{Extent as Extent3D, Layout as ImageLayout},
    pass::{self, Subpass}, pool,
    queue::{QueueFamily, family::QueueGroup},
    window::{Extent2D, PresentationSurface, Surface, SwapchainConfig},
    Instance,
    pso,
};
use gfx_auxil::read_spirv;
use std::iter;
use std::io::Cursor;
use std::sync::Mutex;

pub struct GfxBackend<B: gfx_hal::Backend> {
    adapter: Adapter<B>,
    instance: B::Instance,
    device: B::Device,
    queue_group: QueueGroup<B>,
    pipelines: Mutex<Vec<B::GraphicsPipeline>>,
    vertex_shader_module: B::ShaderModule,
    surface: B::Surface,
    render_pass: B::RenderPass,
    pipeline_layout: B::PipelineLayout,
    pipeline_cache: B::PipelineCache,
}

impl<B: gfx_hal::Backend> GfxBackend<B> {
    pub fn new(window: &winit::window::Window) -> Self {
        let instance = B::Instance::create("shadertoy-browser", 1).unwrap();

        let mut surface = unsafe { instance.create_surface(window) }.unwrap();

        let adapter = {
            let mut adapters = instance.enumerate_adapters();
            for adapter in &adapters {
                println!("{:?}", adapter.info);
            }
            adapters.remove(0)
        };

        let family = adapter
            .queue_families
            .iter()
            .find(|family| {
                surface.supports_queue_family(family) && family.queue_type().supports_graphics()
            })
            .unwrap();

        let physical_device = &adapter.physical_device;
        let sparsely_bound = physical_device
            .features()
            .contains(hal::Features::SPARSE_BINDING | hal::Features::SPARSE_RESIDENCY_IMAGE_2D);
        let mut gpu = unsafe {
            physical_device
                .open(
                    &[(family, &[1.0])],
                    if sparsely_bound {
                        hal::Features::SPARSE_BINDING | hal::Features::SPARSE_RESIDENCY_IMAGE_2D
                    } else {
                        hal::Features::empty()
                    },
                )
                .unwrap()
        };
        let queue_group = gpu.queue_groups.pop().unwrap();
        let device = gpu.device;

        let vert_spv = read_spirv(Cursor::new(include_bytes!("shadertoy_glsl.vert.spv"))).unwrap();

        let vertex_shader_module = unsafe {
            device.create_shader_module(&vert_spv)
        }.unwrap();

        let mut command_pool = unsafe {
            device.create_command_pool(queue_group.family, pool::CommandPoolCreateFlags::empty())
        }
        .unwrap();

        let caps = surface.capabilities(&adapter.physical_device);
        let formats = surface.supported_formats(&adapter.physical_device);
        println!("formats: {:?}", formats);
        let display_format = formats.map_or(Format::Rgba8Srgb, |formats| {
            formats
                .iter()
                .find(|format| format.base_format().1 == ChannelType::Srgb)
                .map(|format| *format)
                .unwrap_or(formats[0])
        });

        let window_size = window.inner_size();

        let dimensions = Extent2D {
            width: window_size.width,
            height: window_size.height,
        };

        let swap_config = SwapchainConfig::from_caps(&caps, display_format, dimensions);
        let framebuffer_attachment = swap_config.framebuffer_attachment();
        println!("{:?}", swap_config);
        let extent = swap_config.extent;
        unsafe {
            surface
                .configure_swapchain(&device, swap_config)
                .expect("Can't configure swapchain");
        };

        let render_pass = {
            let attachment = pass::Attachment {
                format: Some(display_format),
                samples: 1,
                ops: pass::AttachmentOps::new(
                    pass::AttachmentLoadOp::Clear,
                    pass::AttachmentStoreOp::Store,
                ),
                stencil_ops: pass::AttachmentOps::DONT_CARE,
                layouts: ImageLayout::Undefined..ImageLayout::Present,
            };

            let subpass = pass::SubpassDesc {
                colors: &[(0, ImageLayout::ColorAttachmentOptimal)],
                depth_stencil: None,
                inputs: &[],
                resolves: &[],
                preserves: &[],
            };

            unsafe {
                device.create_render_pass(
                    iter::once(attachment),
                    iter::once(subpass),
                    iter::empty(),
                )
            }
            .unwrap()
        };

        let framebuffer = unsafe {
            device.create_framebuffer(
                &render_pass,
                iter::once(framebuffer_attachment),
                Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                },
            )
        }
        .unwrap();

        let pipeline_layout = unsafe {
            device.create_pipeline_layout(iter::empty(), iter::empty())
        }.unwrap();

        let data = std::fs::read("happy_cache.cache").unwrap();

        let pipeline_cache = unsafe {
            device.create_pipeline_cache(Some(&data))
        }.unwrap();

        Self {
            adapter,
            instance,
            device,
            queue_group,
            pipelines: Default::default(),
            vertex_shader_module,
            surface,
            render_pass,
            pipeline_layout,
            pipeline_cache,
        }
    }
}

impl<B: gfx_hal::Backend> RenderBackend for GfxBackend<B> {
    fn render_frame(&mut self, params: RenderParams<'_>) {
        todo!()
    }

    fn new_pipeline(&self, shader_path: &str, shader_source: &str) -> Result<RenderPipelineHandle> {
        let mut compiler = shaderc::Compiler::new().unwrap();

        let spirv_path: std::path::PathBuf = format!("{}.spv", shader_path).into();

        let spv = if spirv_path.exists() {
            read_spirv(std::fs::File::open(spirv_path).unwrap()).unwrap()
        } else {
            let result = compiler.compile_into_spirv(
                shader_source,
                shaderc::ShaderKind::Fragment,
                shader_path,
                "main",
                None
            );

            let artifact = match result {
                Ok(artifact) => artifact,
                Err(error) => return Err(format!("glsl->spv error: {}", error).into())
            };

            std::fs::write(spirv_path, artifact.as_binary_u8()).unwrap();

            artifact.as_binary().to_vec()
        };

        let frag_shader_module = match unsafe {
            self.device.create_shader_module(&spv)
        } {
            Ok(module) => module,
            Err(error) => return Err(format!("create_shader_module error: {}", error).into())
        };

        let (vs_entry, fs_entry) = (
            pso::EntryPoint {
                entry: "main",
                module: &self.vertex_shader_module,
                specialization: pso::Specialization::default(),
            },
            pso::EntryPoint {
                entry: "main",
                module: &frag_shader_module,
                specialization: pso::Specialization::default(),
            },
        );

        let subpass = Subpass {
            index: 0,
            main_pass: &self.render_pass,
        };

        let pipeline_desc = pso::GraphicsPipelineDesc::new(
            pso::PrimitiveAssemblerDesc::Vertex {
                buffers: &[],
                attributes: &[],
                input_assembler: pso::InputAssemblerDesc {
                    primitive: pso::Primitive::TriangleList,
                        with_adjacency: false,
                        restart_index: None,
                },
                vertex: vs_entry,
                geometry: None,
                tessellation: None,
            },
            pso::Rasterizer::FILL,
            Some(fs_entry),
            &self.pipeline_layout,
            subpass
        );

        let pipeline = match unsafe {
            self.device.create_graphics_pipeline(&pipeline_desc, Some(&self.pipeline_cache))
        } {
            Ok(pipeline) => pipeline,
            Err(error) => return Err(format!("pipeline creation failed: {}", error).into())
        };

        let mut pipelines = self.pipelines.lock().unwrap();

        pipelines.push(pipeline);

        Ok(pipelines.len() - 1)
    }

    fn write_pipeline_cache(&self) {
        let data = unsafe {
            self.device.get_pipeline_cache_data(&self.pipeline_cache)
        }.unwrap();

        //std::fs::write("happy_cache.cache", data).unwrap();
    }
}

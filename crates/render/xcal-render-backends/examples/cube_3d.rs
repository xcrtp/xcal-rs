//! cube_3d.rs — 独立 3D demo（不依赖 xcal-app / scheduler）。
//!
//! 启动后：
//! 1. 从 eframe 的 `wgpu_render_state` 取 device/queue，创建
//!    [`xcal_render_backends::WgpuBackend3D`] 并存入 egui-wgpu Renderer 的
//!    `callback_resources`（pipeline 等 GPU 资源的宿主）；
//! 2. 每帧手动更新轨道相机位置 → 填充 [`DrawQueue3D`]（线框立方体）→
//!    通过 [`xcal_render_backends::SceneCompositorCallback`] 注册**单个** egui
//!    绘制回调，在 eframe 主 render pass 中把 3D 离屏纹理合成进来。
//!
//! 运行：`cargo run -p xcal-render-backends --features egui-wgpu --example cube_3d`

use std::sync::Arc;
use std::time::Instant;

use glam::Mat4;
use xcal_scene::domain::{LayerBlend, LayerId, LayerKind};
use xcal_scene::scene::components::{Camera3D, Material3D, MeshGeometry, Projection};
use xcal_scene::scene::primitives::wireframe_cube;
use xcal_render::{DrawQueue, DrawQueue3D};
use xcal_render_backends::{
    Compositor, SceneCompositorCallback, SceneLayer, WgpuBackend3D,
};
use xc_geometry::{Color32, Rect2D, Vec2};

struct Cube3dApp {
    start: Instant,
    cube: Arc<MeshGeometry>,
}

impl eframe::App for Cube3dApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 相机轨道运动：eye 绕 Y 轴、半径 5、角速度 0.5 rad/s。
        let t = self.start.elapsed().as_secs_f32();
        let angle = t * 0.5;
        let eye = glam::Vec3::new(angle.sin() * 5.0, 1.2, angle.cos() * 5.0);
        let camera = Camera3D::new(
            eye,
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
        );

        // eframe 0.36 的 App::ui 直接给出中央区域 Ui（无边距/背景），
        // max_rect 即整个窗口客户区。
        let rect = ui.max_rect();
        let viewport = Rect2D::new(
            Vec2::new(rect.min.x, rect.min.y),
            Vec2::new(rect.max.x, rect.max.y),
        );

        let white = Color32::new(255, 255, 255, 255);
        let mut queue = DrawQueue3D::new();
        queue.draw_mesh(Arc::clone(&self.cube), Mat4::IDENTITY, Material3D::color(white));

        // S3-2/C3：图层先渲染到自己的离屏纹理（透明清屏），再由合成器混合到主 pass。
        // S3-2 修复：2D/3D 合并为**单个**回调，一次性合成全部图层。
        let layers = vec![SceneLayer {
            id: LayerId(0),
            kind: LayerKind::Viewport3D { camera },
            blend: LayerBlend::Alpha,
            opacity: 1.0,
        }];
        let queue_2d = DrawQueue::new(); // 本 demo 无 2D 图层
        let callback = SceneCompositorCallback::new(viewport, layers, &queue_2d, &queue, Vec::new());
        ui.painter().add(egui::epaint::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(rect, callback),
        ));

        ui.painter().text(
            egui::pos2(rect.left() + 12.0, rect.top() + 12.0),
            egui::Align2::LEFT_TOP,
            "xcal-rs 3D demo: wireframe cube + orbit camera",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );

        // 动画持续运行，请求下一帧。
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 720.0])
            .with_title("xcal-rs cube_3d"),
        ..Default::default()
    };

    eframe::run_native(
        "xcal-rs cube_3d",
        options,
        Box::new(|cc| {
            // 3D 后端生命周期与 egui-wgpu Renderer 绑定：
            // 回调为 &self，GPU 资源（pipeline/bind group layout）放在
            // callback_resources TypeMap 中，prepare/paint 时取用。
            let rs = cc
                .wgpu_render_state
                .as_ref()
                .expect("wgpu render state required (NativeOptions.renderer = Wgpu)");
            let backend =
                WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
            rs.renderer.write().callback_resources.insert(backend);
            // S3-2：合成器（把 3D 离屏纹理按声明顺序混合进主 pass）。
            let compositor = Compositor::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(compositor);

            Ok(Box::new(Cube3dApp {
                start: Instant::now(),
                cube: Arc::new(wireframe_cube(1.0)),
            }))
        }),
    )
}

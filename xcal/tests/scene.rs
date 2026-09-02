//! scene 模块集成测试

use xcal::scene::Scene;
use xcal::scene::components::*;

// ─── Scene 测试 ─────────────────────────────────────────────

#[test]
fn scene_new_is_empty() {
    let mut scene = Scene::new();
    let count = scene.render(|w, _| w.len());
    assert_eq!(count, 0);
}

#[test]
fn scene_spawn_entity() {
    let mut scene = Scene::new();
    scene.spawn((Circle::new((0.0, 0.0), 1.0),));
    let count = scene.render(|w, _| w.len());
    assert_eq!(count, 1);
}

#[test]
fn scene_spawn_multiple() {
    let mut scene = Scene::new();
    scene.spawn((Circle::new((0.0, 0.0), 1.0),));
    scene.spawn((Circle::new((1.0, 0.0), 2.0),));
    scene.spawn((Circle::new((2.0, 0.0), 3.0),));
    let count = scene.render(|w, _| w.len());
    assert_eq!(count, 3);
}

#[test]
fn scene_add_and_borrow_resource() {
    let mut scene = Scene::new();
    scene.add_resource(42_i32);
    let r = scene.borrow_resource::<i32>();
    assert_eq!(r, Some(&42));
}

#[test]
fn scene_borrow_resource_mut() {
    let mut scene = Scene::new();
    scene.add_resource(42_i32);
    {
        let r = scene.borrow_resource_mut::<i32>();
        assert_eq!(r, Some(&mut 42));
        if let Some(r) = r {
            *r = 100;
        }
    }
    assert_eq!(scene.borrow_resource::<i32>(), Some(&100));
}

#[test]
fn scene_borrow_missing_resource_returns_none() {
    let scene = Scene::new();
    assert!(scene.borrow_resource::<i32>().is_none());
}

#[test]
fn scene_borrow_mut_missing_resource_returns_none() {
    let mut scene = Scene::new();
    assert!(scene.borrow_resource_mut::<i32>().is_none());
}

#[test]
fn scene_resource_type_isolation() {
    let mut scene = Scene::new();
    scene.add_resource(42_i32);
    scene.add_resource(3.14_f64);
    assert_eq!(scene.borrow_resource::<i32>(), Some(&42));
    assert_eq!(scene.borrow_resource::<f64>(), Some(&3.14));
}

#[test]
fn scene_render_captures_world_and_resources() {
    let mut scene = Scene::new();
    scene.add_resource(99_i32);
    scene.spawn((Circle::new((0.0, 0.0), 5.0),));
    let result = scene.render(|w, r| {
        let count = w.len();
        let res = r
            .get(&std::any::TypeId::of::<i32>())
            .and_then(|b| b.downcast_ref::<i32>())
            .copied();
        (count, res)
    });
    assert_eq!(result.0, 1);
    assert_eq!(result.1, Some(99));
}

#[test]
fn scene_query_components() {
    let mut scene = Scene::new();
    scene.spawn((
        Circle::new((0.0, 0.0), 1.0),
        StrokeSolid::new(2.0, (100, 200, 50, 255)),
    ));
    scene.spawn((Circle::new((3.0, 4.0), 2.0),));

    let circle_count = scene.render(|w, _| w.query::<&Circle>().iter().count());
    assert_eq!(circle_count, 2);

    let stroke_count = scene.render(|w, _| {
        w.query::<(&Circle, &StrokeSolid)>().iter().count()
    });
    assert_eq!(stroke_count, 1);
}

// ─── Circle 组件测试 ────────────────────────────────────────

#[test]
fn circle_new() {
    let c = Circle::new((0.0_f32, 0.0), 5.0);
    assert_eq!(c.center.x, 0.0);
    assert_eq!(c.center.y, 0.0);
    assert_eq!(c.radius, 5.0);
}

#[test]
fn circle_with_offset_center() {
    let c = Circle::new((3.5_f32, -2.0), 10.0);
    assert_eq!(c.center.x, 3.5);
    assert_eq!(c.center.y, -2.0);
    assert_eq!(c.radius, 10.0);
}

#[test]
fn circle_clone() {
    let c = Circle::new((1.0, 2.0), 3.0);
    let c2 = c.clone();
    assert_eq!(c2.center.x, 1.0);
    assert_eq!(c2.center.y, 2.0);
    assert_eq!(c2.radius, 3.0);
}

// ─── Path 组件测试 ─────────────────────────────────────────

#[test]
fn path_new() {
    let p = Path::new(vec![(1.0_f32, 2.0, 3.0).into()], true);
    assert_eq!(p.vertices.len(), 1);
    assert!(p.closed);
}

#[test]
fn path_with_multiple_vertices() {
    let p = Path::new(
        vec![
            (0.0_f32, 0.0, 0.0).into(),
            (1.0_f32, 1.0, 0.0).into(),
            (2.0_f32, 0.0, 0.0).into(),
        ],
        false,
    );
    assert_eq!(p.vertices.len(), 3);
    assert!(!p.closed);
}

// ─── Mesh 组件测试 ──────────────────────────────────────────

#[test]
fn triangle_mesh_construction() {
    let mesh = TriangleMesh {
        vertices: vec![
            (0.0_f32, 0.0, 0.0).into(),
            (1.0_f32, 0.0, 0.0).into(),
            (0.0_f32, 1.0, 0.0).into(),
        ],
        indices: vec![0, 1, 2],
    };
    assert_eq!(mesh.vertices.len(), 3);
    assert_eq!(mesh.indices.len(), 3);
}

#[test]
fn quad_mesh_construction() {
    let mesh = QuadMesh {
        vertices: vec![
            (0.0_f32, 0.0, 0.0).into(),
            (1.0_f32, 0.0, 0.0).into(),
            (1.0_f32, 1.0, 0.0).into(),
            (0.0_f32, 1.0, 0.0).into(),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
    };
    assert_eq!(mesh.vertices.len(), 4);
    assert_eq!(mesh.indices.len(), 6);
}

#[test]
fn path_mesh_clone() {
    let mesh = PathMesh {
        vertices: vec![(0.0_f32, 0.0, 0.0).into()],
        indices: vec![0],
    };
    let cloned = mesh.clone();
    assert_eq!(cloned.vertices.len(), 1);
    assert_eq!(cloned.indices.len(), 1);
}

// ─── 渲染组件测试 ──────────────────────────────────────────

#[test]
fn stroke_solid_new() {
    let s = StrokeSolid::new(2.0, (100, 200, 50, 255));
    assert_eq!(s.width, 2.0);
    assert_eq!(s.color.r(), 100);
    assert_eq!(s.color.g(), 200);
    assert_eq!(s.color.b(), 50);
    assert_eq!(s.color.a(), 255);
}

#[test]
fn fill_solid_new() {
    let f = FillSolid::new((255, 0, 0, 128));
    assert_eq!(f.color.r(), 255);
    assert_eq!(f.color.g(), 0);
    assert_eq!(f.color.b(), 0);
    assert_eq!(f.color.a(), 128);
}

#[test]
fn fill_style_solid_variant() {
    let f = FillStyle::Solid(FillSolid::new((0, 255, 0, 255)));
    if let FillStyle::Solid(inner) = f {
        assert_eq!(inner.color.g(), 255);
    } else {
        panic!("Expected Solid variant");
    }
}

#[test]
fn stroke_style_solid_variant() {
    let s = StrokeStyle::Solid(StrokeSolid::new(3.0, (0, 0, 255, 255)));
    if let StrokeStyle::Solid(inner) = s {
        assert_eq!(inner.width, 3.0);
        assert_eq!(inner.color.b(), 255);
    } else {
        panic!("Expected Solid variant");
    }
}

// ─── 动画组件测试 ──────────────────────────────────────────

#[test]
fn clock_state_variants() {
    let states = [ClockState::Step, ClockState::Suspend, ClockState::Loop];
    assert!(matches!(states[0], ClockState::Step));
    assert!(matches!(states[1], ClockState::Suspend));
    assert!(matches!(states[2], ClockState::Loop));
}

#[test]
fn local_clock_new() {
    let clock = LocalClock::new(None, 0.0, 0.0, ClockState::Step);
    assert!(clock.parent.is_none());
    assert_eq!(clock.parent_offset, 0.0);
    assert_eq!(clock.current, 0.0);
    assert!(matches!(clock.state, ClockState::Step));
}

#[test]
fn local_clock_clone() {
    let clock = LocalClock::new(None, 1.5, 10.0, ClockState::Loop);
    let cloned = clock.clone();
    assert_eq!(cloned.parent_offset, 1.5);
    assert_eq!(cloned.current, 10.0);
    assert!(matches!(cloned.state, ClockState::Loop));
}

// ─── 函数与曲线组件测试 ────────────────────────────────────

#[test]
fn function_x2y_new_and_call() {
    let f = FunctionX2Y::new(|x: f32| x * 2.0);
    assert_eq!((f.f)(5.0), 10.0);
    assert_eq!((f.f)(-3.0), -6.0);
}

#[test]
fn function_x2y_clone() {
    let f = FunctionX2Y::new(|x: f32| x.sin());
    let f2 = f.clone();
    assert!((f2.f)(0.0_f32).abs() < 1e-6);
}

#[test]
fn parameter_curve_t2xy_new_and_call() {
    let curve = ParameterCurveT2XY::<f32, (f32, f32)>::new(|t: f32| (t, t * t));
    let (x, y) = (curve.f)(3.0);
    assert_eq!(x, 3.0);
    assert_eq!(y, 9.0);
}

#[test]
fn parameter_curve_t2xyz_new_and_call() {
    let curve =
        ParameterCurveT2XYZ::<f32, (f32, f32, f32)>::new(|t: f32| (t, t * 2.0, t * 3.0));
    let (x, y, z) = (curve.f)(2.0);
    assert_eq!(x, 2.0);
    assert_eq!(y, 4.0);
    assert_eq!(z, 6.0);
}

// ─── 采样器组件测试 ────────────────────────────────────────

#[test]
fn sampler1d_new() {
    let s = Sampler1D::new(-10.0_f32, 10.0, 0.1);
    assert_eq!(s.start, -10.0);
    assert_eq!(s.end, 10.0);
    assert_eq!(s.step, 0.1);
}

#[test]
fn sampler2d_new() {
    let s = Sampler2D::new(0.0_f32, 1.0, 0.1, 0.0, 1.0, 0.2);
    assert_eq!(s.u_start, 0.0);
    assert_eq!(s.u_end, 1.0);
    assert_eq!(s.u_step, 0.1);
    assert_eq!(s.v_start, 0.0);
    assert_eq!(s.v_end, 1.0);
    assert_eq!(s.v_step, 0.2);
}

// ─── Marker 组件测试 ───────────────────────────────────────

#[test]
fn wgpu_device_implements_device() {
    fn check_device<T: Device>() {}
    check_device::<WGPUDevice>();
}

#[test]
fn cpu_device_implements_device() {
    fn check_device<T: Device>() {}
    check_device::<CPUDevice>();
}

// ─── Transform 组件测试 ────────────────────────────────────

#[test]
fn transform_builder() {
    let t = TransformBuilder::default()
        .position(glam::Vec3::new(1.0, 2.0, 3.0))
        .rotation(glam::Quat::IDENTITY)
        .scale(glam::Vec3::new(2.0, 2.0, 2.0))
        .build()
        .unwrap();
    assert_eq!(t.position, glam::Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(t.rotation, glam::Quat::IDENTITY);
    assert_eq!(t.scale, glam::Vec3::new(2.0, 2.0, 2.0));
}

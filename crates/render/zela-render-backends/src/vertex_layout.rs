// vertex_layout.rs
//
// 顶点格式契约 → wgpu 翻译层（V6，§4.6 + 实例化扩展）。
//
// 纯函数、无状态：只做契约 → `wgpu::VertexBufferLayout` 的翻译，
// **不碰 wgpu_3d.rs 的接线逻辑**（那是 G 任务 G6 的领地）。
//
// 签名说明：计划原文为 `to_wgpu(&VertexLayout) -> wgpu::VertexBufferLayout<'static>`，
// 但 `attributes: &'a [VertexAttribute]` 是借用 —— 要返回 'static 只能靠泄漏内存
// 或全局注册表，违背"纯函数、无状态"。因此改为**调用方提供属性数组存储**：
// 返回的布局借用 `attrs`，在创建 pipeline 期间保持有效即可（pipeline 通常只建一次）。
//
// 实例化扩展：wgpu 一次 draw 可绑多个 vertex buffer（顶点 + 实例数据），
// `to_wgpu_buffers` 翻译完整 [`VertexInputDesc`]；shader location 口径为
// **跨 buffer 全局连续编号**（wgpu 语义），测试锁死。

use zela_render::vertex::{AttribType, StepMode, VertexAttrib, VertexInputDesc, VertexLayout};

fn attrib_format(ty: AttribType) -> wgpu::VertexFormat {
    match ty {
        AttribType::F32 => wgpu::VertexFormat::Float32,
        AttribType::F32x2 => wgpu::VertexFormat::Float32x2,
        AttribType::F32x3 => wgpu::VertexFormat::Float32x3,
        AttribType::F32x4 => wgpu::VertexFormat::Float32x4,
    }
}

fn step_mode_to_wgpu(step: StepMode) -> wgpu::VertexStepMode {
    match step {
        StepMode::Vertex => wgpu::VertexStepMode::Vertex,
        StepMode::Instance => wgpu::VertexStepMode::Instance,
    }
}

fn attrib_to_wgpu(a: &VertexAttrib, shader_location: u32) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        format: attrib_format(a.ty),
        offset: a.offset as wgpu::BufferAddress,
        shader_location,
    }
}

/// 翻译到 wgpu 顶点缓冲布局（单 buffer）。
///
/// - 属性顺序 = 契约 `attribs` 顺序（location = 下标）；
/// - offset / array_stride 直接取契约字节值；step_mode 按契约翻译
///   （默认 Vertex，实例数据 buffer 译为 Instance）；
/// - 入口先做 [`VertexLayout::validate`]（分层校验：Vertex 严格词汇表 /
///   Instance 宽松），非法布局 fail-fast。
pub fn to_wgpu<'a>(
    layout: &VertexLayout,
    attrs: &'a mut Vec<wgpu::VertexAttribute>,
) -> wgpu::VertexBufferLayout<'a> {
    layout
        .validate()
        .expect("to_wgpu: 契约布局非法（VertexLayout::validate 失败）");
    attrs.clear();
    attrs.extend(
        layout
            .attribs
            .iter()
            .enumerate()
            .map(|(location, a)| attrib_to_wgpu(a, location as u32)),
    );
    wgpu::VertexBufferLayout {
        array_stride: layout.stride as wgpu::BufferAddress,
        step_mode: step_mode_to_wgpu(layout.step_mode),
        attributes: attrs,
    }
}

/// 翻译完整顶点输入（多 buffer，实例化支持）。
///
/// - `buffers` 顺序 = wgpu vertex buffer slot（0..n）；
/// - **shader location 跨 buffer 全局连续编号**：buffer0 的属性占 0..n1，
///   buffer1 从 n1 续编……（wgpu 语义，测试锁死）；
/// - `attrs` 为调用方提供的**每 buffer 各自的**属性数组存储（`attrs[slot]`
///   对应 `buffers[slot]`）；返回的各布局借用它，在创建 pipeline 期间保持有效；
/// - 入口先做 [`VertexInputDesc::validate`]，非法描述 fail-fast。
pub fn to_wgpu_buffers<'a>(
    desc: &VertexInputDesc,
    attrs: &'a mut Vec<Vec<wgpu::VertexAttribute>>,
) -> Vec<wgpu::VertexBufferLayout<'a>> {
    desc.validate()
        .expect("to_wgpu_buffers: 顶点输入描述非法（VertexInputDesc::validate 失败）");
    attrs.clear();
    attrs.resize_with(desc.buffers.len(), Vec::new);

    // location 跨 buffer 全局连续（wgpu 语义）
    let mut location = 0u32;
    for (slot, buffer) in desc.buffers.iter().enumerate() {
        for a in &buffer.attribs {
            attrs[slot].push(attrib_to_wgpu(a, location));
            location += 1;
        }
    }

    // 通过共享引用取 'a 生命周期的切片（&mut → & 的全长重借用）
    let attrs_ref: &'a Vec<Vec<wgpu::VertexAttribute>> = attrs;
    attrs_ref
        .iter()
        .zip(desc.buffers.iter())
        .map(|(attributes, buffer)| wgpu::VertexBufferLayout {
            array_stride: buffer.stride as wgpu::BufferAddress,
            step_mode: step_mode_to_wgpu(buffer.step_mode),
            attributes,
        })
        .collect()
}

// ─── 单测（V6） ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use zela_render::vertex::{AaModel, CacheStrategy, VertexAttrib};

    /// 标准布局翻译：属性顺序、offset、stride 与契约一致（R0：四槽 / stride 24）。
    #[test]
    fn standard_layout_translation_matches_contract() {
        let layout = VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static);
        let mut attrs = Vec::new();
        let l = to_wgpu(&layout, &mut attrs);

        assert_eq!(l.array_stride, 24);
        assert_eq!(l.step_mode, wgpu::VertexStepMode::Vertex);
        let a = l.attributes;
        assert_eq!(a.len(), 4);
        // position @ location 0
        assert_eq!(a[0].format, wgpu::VertexFormat::Float32x2);
        assert_eq!(a[0].offset, 0);
        assert_eq!(a[0].shader_location, 0);
        // attrib0 @ location 1
        assert_eq!(a[1].format, wgpu::VertexFormat::Float32x2);
        assert_eq!(a[1].offset, 8);
        assert_eq!(a[1].shader_location, 1);
        // attrib1 @ location 2
        assert_eq!(a[2].format, wgpu::VertexFormat::Float32);
        assert_eq!(a[2].offset, 16);
        assert_eq!(a[2].shader_location, 2);
        // attrib2 @ location 3（R0：归一化弧长）
        assert_eq!(a[3].format, wgpu::VertexFormat::Float32);
        assert_eq!(a[3].offset, 20);
        assert_eq!(a[3].shader_location, 3);
    }

    /// 自定义布局（含对齐填充）：offset / stride 与契约计算一致。
    #[test]
    fn custom_layout_translation_preserves_offsets() {
        let mut attribs = vec![
            VertexAttrib {
                semantic: "position",
                ty: AttribType::F32,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib0",
                ty: AttribType::F32x3,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib1",
                ty: AttribType::F32x2,
                offset: 0,
            },
        ];
        let stride = VertexLayout::compute_layout(&mut attribs);
        let layout = VertexLayout {
            attribs,
            stride,
            aa_model: AaModel::None,
            cache: CacheStrategy::PerFrame,
            step_mode: StepMode::Vertex,
        };

        let mut attrs = Vec::new();
        let l = to_wgpu(&layout, &mut attrs);
        assert_eq!(l.array_stride as u32, stride);
        assert_eq!(l.attributes[0].format, wgpu::VertexFormat::Float32);
        assert_eq!(l.attributes[0].offset, 0);
        assert_eq!(l.attributes[1].format, wgpu::VertexFormat::Float32x3);
        assert_eq!(l.attributes[1].offset, 4);
        assert_eq!(l.attributes[2].format, wgpu::VertexFormat::Float32x2);
        assert_eq!(l.attributes[2].offset, 16);
        assert_eq!(l.attributes[2].shader_location, 2);
    }

    /// 非法契约布局 fail-fast（重叠属性）。
    #[test]
    #[should_panic(expected = "to_wgpu: 契约布局非法")]
    fn invalid_contract_layout_fails_fast() {
        let mut layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
        layout.attribs[1].offset = 0; // 与 position 重叠
        let mut attrs = Vec::new();
        let _ = to_wgpu(&layout, &mut attrs);
    }

    /// 非法契约布局 fail-fast：Vertex buffer 用宿主自定义语义
    /// （分层校验经 to_wgpu 入口生效）。
    #[test]
    #[should_panic(expected = "to_wgpu: 契约布局非法")]
    fn invalid_semantic_on_vertex_buffer_fails_fast() {
        let mut layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
        layout.attribs[0].semantic = "instance_offset";
        let mut attrs = Vec::new();
        let _ = to_wgpu(&layout, &mut attrs);
    }

    // ── 实例化扩展 ────────────────────────────────────────────────────────

    fn instance_layout() -> VertexLayout {
        let mut attribs = vec![
            VertexAttrib {
                semantic: "instance_offset",
                ty: AttribType::F32x4,
                offset: 0,
            },
            VertexAttrib {
                semantic: "instance_color",
                ty: AttribType::F32x4,
                offset: 16,
            },
        ];
        let stride = VertexLayout::compute_layout(&mut attribs);
        VertexLayout {
            attribs,
            stride,
            aa_model: AaModel::None,
            cache: CacheStrategy::PerFrame,
            step_mode: StepMode::Instance,
        }
    }

    /// 单 buffer 升级：`VertexInputDesc::single()` 的翻译行为与原 `to_wgpu` 一致。
    #[test]
    fn single_desc_translation_matches_to_wgpu() {
        let layout = VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static);

        let mut attrs_a = Vec::new();
        let a = to_wgpu(&layout, &mut attrs_a);

        let mut attrs_b = Vec::new();
        let bs = to_wgpu_buffers(&VertexInputDesc::single(layout.clone()), &mut attrs_b);
        assert_eq!(bs.len(), 1);
        let b = &bs[0];

        assert_eq!(a.array_stride, b.array_stride);
        assert_eq!(a.step_mode, b.step_mode);
        assert_eq!(a.attributes, b.attributes, "单 buffer 升级必须零行为差异");
    }

    /// 混合布局翻译：Vertex + Instance buffer 的 step_mode / offset / stride /
    /// shader_location 逐一正确；**location 跨 buffer 全局连续**（wgpu 语义，锁死口径）。
    #[test]
    fn mixed_translation_global_continuous_locations() {
        let desc = VertexInputDesc {
            buffers: vec![
                VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static),
                instance_layout(),
            ],
        };
        let mut attrs = Vec::new();
        let layouts = to_wgpu_buffers(&desc, &mut attrs);

        assert_eq!(layouts.len(), 2);

        // slot 0：顶点数据（R0：四槽 / stride 24）
        let v = &layouts[0];
        assert_eq!(v.array_stride, 24);
        assert_eq!(v.step_mode, wgpu::VertexStepMode::Vertex);
        assert_eq!(v.attributes.len(), 4);
        assert_eq!(v.attributes[0].shader_location, 0);
        assert_eq!(v.attributes[1].shader_location, 1);
        assert_eq!(v.attributes[2].shader_location, 2);
        assert_eq!(v.attributes[3].shader_location, 3);
        assert_eq!(v.attributes[0].offset, 0);
        assert_eq!(v.attributes[1].offset, 8);
        assert_eq!(v.attributes[2].offset, 16);
        assert_eq!(v.attributes[3].offset, 20);

        // slot 1：实例数据 —— location 从 4 续编（全局连续，不归零）
        let i = &layouts[1];
        assert_eq!(i.array_stride, 32);
        assert_eq!(i.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(i.attributes.len(), 2);
        assert_eq!(i.attributes[0].format, wgpu::VertexFormat::Float32x4);
        assert_eq!(i.attributes[0].offset, 0);
        assert_eq!(i.attributes[0].shader_location, 4);
        assert_eq!(i.attributes[1].format, wgpu::VertexFormat::Float32x4);
        assert_eq!(i.attributes[1].offset, 16);
        assert_eq!(i.attributes[1].shader_location, 5);

        // 每 buffer 各自的存储被正确填充
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].len(), 4);
        assert_eq!(attrs[1].len(), 2);
    }

    /// 单 Instance buffer（无 Vertex buffer）→ VertexInputDesc::validate 失败，
    /// to_wgpu_buffers fail-fast。
    #[test]
    #[should_panic(expected = "to_wgpu_buffers: 顶点输入描述非法")]
    fn instance_only_desc_fails_fast() {
        let desc = VertexInputDesc {
            buffers: vec![instance_layout()],
        };
        let mut attrs = Vec::new();
        let _ = to_wgpu_buffers(&desc, &mut attrs);
    }

    /// Instance buffer 也可经单 buffer `to_wgpu` 翻译（step_mode 正确映射）；
    /// 单 buffer 场景下组合合法性归 [`VertexInputDesc`] 管。
    #[test]
    fn single_instance_layout_translates_step_mode() {
        let mut attrs = Vec::new();
        let l = to_wgpu(&instance_layout(), &mut attrs);
        assert_eq!(l.step_mode, wgpu::VertexStepMode::Instance);
        assert_eq!(l.array_stride, 32);
        assert_eq!(l.attributes.len(), 2);
        assert_eq!(l.attributes[0].shader_location, 0);
        assert_eq!(l.attributes[1].shader_location, 1);
    }
}

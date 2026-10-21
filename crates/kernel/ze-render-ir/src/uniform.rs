// uniform.rs — uniform 布局声明（B2-0d / D18，对标 `VertexLayout`）。
//
// 目的：消灭后端 `pack_uniform_bytes` / `pack_light_bytes` 里的**魔数偏移**。
// 字段按 std140 规则布局，`compute_offsets` 纯计算，`validate` 校验；wgsl 侧的
// 结构由「已知向量测试」锁定（同 `frame_hash.rs` 范式，不引入构建脚本）。
//
// std140 关键规则（WGSL uniform 地址空间）：
//   - 标量对齐 4 / vec2 对齐 8 / **vec3、vec4 对齐 16**（vec3 的 size 是 12，
//     但下一字段必须从 16 的倍数开始 —— 这就是「vec3 陷阱」）；
//   - mat4 按列 16 对齐，size 64；
//   - `array<vec4<f32>, N>` 元素 stride 16，size N×16；
//   - struct 总大小向上取整到 16 的倍数。

/// uniform 字段类型（std140 口径，纯数据无 GPU 类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UniformType {
    F32,
    F32x2,
    /// vec3：size 12，**对齐 16**（vec3 陷阱的来源）。
    F32x3,
    F32x4,
    /// mat4x4<f32>：对齐 16，size 64。
    Mat4,
    /// `array<vec4<f32>, N>`：元素 stride 16。
    ArrayOfF32x4(u32),
}

impl UniformType {
    /// std140 对齐（字节）。
    pub const fn alignment(self) -> u32 {
        match self {
            Self::F32 => 4,
            Self::F32x2 => 8,
            Self::F32x3 | Self::F32x4 | Self::Mat4 | Self::ArrayOfF32x4(_) => 16,
        }
    }

    /// std140 大小（字节，不含尾部填充）。
    pub const fn size(self) -> u32 {
        match self {
            Self::F32 => 4,
            Self::F32x2 => 8,
            Self::F32x3 => 12,
            Self::F32x4 => 16,
            Self::Mat4 => 64,
            Self::ArrayOfF32x4(n) => n * 16,
        }
    }
}

/// 单个 uniform 字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UniformField {
    pub name: &'static str,
    pub ty: UniformType,
    /// 字节偏移。由 [`UniformLayout::compute_offsets`] 填写；手工构造时置 0。
    pub offset: u32,
}

impl UniformField {
    pub const fn new(name: &'static str, ty: UniformType) -> Self {
        Self {
            name,
            ty,
            offset: 0,
        }
    }
}

/// uniform 布局自洽性错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UniformLayoutError {
    #[error("uniform 布局至少需要一个字段")]
    Empty,
    #[error("字段 {name} 的 offset {offset} 未对齐到 std140 对齐 {align}")]
    Misaligned {
        name: &'static str,
        offset: u32,
        align: u32,
    },
    #[error("字段 {name} 的 offset {offset} 与前一字段重叠")]
    Overlap { name: &'static str, offset: u32 },
    #[error("布局 size {size} 未覆盖字段 {name} 的结尾 {needed}")]
    SizeTooSmall {
        size: u32,
        name: &'static str,
        needed: u32,
    },
}

/// uniform 缓冲布局。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UniformLayout {
    pub fields: Vec<UniformField>,
    /// 总大小（字节，已向上取整到 std140 struct 对齐 16）。
    pub size: u32,
}

impl UniformLayout {
    /// 按 std140 计算每个字段的 offset 与总大小（纯函数，确定性）。
    pub fn compute_offsets(fields: &mut [UniformField]) -> u32 {
        let mut cursor = 0u32;
        for f in fields.iter_mut() {
            let align = f.ty.alignment();
            cursor = cursor.div_ceil(align) * align;
            f.offset = cursor;
            cursor += f.ty.size();
        }
        // struct 整体对齐 16（std140）。
        cursor.div_ceil(16) * 16
    }

    /// 按字段声明顺序构造并计算偏移。
    pub fn new(mut fields: Vec<UniformField>) -> Self {
        let size = Self::compute_offsets(&mut fields);
        Self { fields, size }
    }

    /// 按名字取字段偏移（消费者用；未声明即 `None`，不静默返回 0）。
    pub fn offset_of(&self, name: &str) -> Option<u32> {
        self.fields.iter().find(|f| f.name == name).map(|f| f.offset)
    }

    /// 校验：对齐 / 不重叠 / size 覆盖。
    pub fn validate(&self) -> Result<(), UniformLayoutError> {
        if self.fields.is_empty() {
            return Err(UniformLayoutError::Empty);
        }
        let mut end = 0u32;
        for f in &self.fields {
            let align = f.ty.alignment();
            if f.offset % align != 0 {
                return Err(UniformLayoutError::Misaligned {
                    name: f.name,
                    offset: f.offset,
                    align,
                });
            }
            if f.offset < end {
                return Err(UniformLayoutError::Overlap {
                    name: f.name,
                    offset: f.offset,
                });
            }
            end = f.offset + f.ty.size();
        }
        if self.size < end {
            return Err(UniformLayoutError::SizeTooSmall {
                size: self.size,
                name: self.fields[self.fields.len() - 1].name,
                needed: end,
            });
        }
        Ok(())
    }
}

// ─── cube.wgsl 的两个 uniform 结构的已知向量（锁定 wgsl 侧） ───────────────

/// `cube.wgsl::Uniforms`：`mvp(mat4) + model(mat4) + color(vec4) + params(vec4)`。
///
/// 与 [`wgpu_3d::UNIFORM_SIZE`]（160）逐位对应；wgsl 改动 ⇒ 测试红。
pub fn cube_uniforms_layout() -> UniformLayout {
    UniformLayout::new(vec![
        UniformField::new("mvp", UniformType::Mat4),
        UniformField::new("model", UniformType::Mat4),
        UniformField::new("color", UniformType::F32x4),
        UniformField::new("params", UniformType::F32x4),
    ])
}

/// `cube.wgsl::Lights`：场景级光源（B1-A/B）。
///
/// 与 `wgpu_3d::LIGHT_UNIFORM_SIZE`（288）逐位对应。
pub fn cube_lights_layout() -> UniformLayout {
    UniformLayout::new(vec![
        UniformField::new("counts", UniformType::F32x4),
        UniformField::new("dirs", UniformType::ArrayOfF32x4(4)),
        UniformField::new("colors", UniformType::ArrayOfF32x4(4)),
        UniformField::new("points", UniformType::ArrayOfF32x4(4)),
        UniformField::new("point_colors", UniformType::ArrayOfF32x4(4)),
        UniformField::new("camera", UniformType::F32x4),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已知向量锁定：`Uniforms` 的字段偏移与总大小（wgsl 侧不能悄悄漂移）。
    #[test]
    fn uniforms_known_vector() {
        let l = cube_uniforms_layout();
        assert_eq!(l.offset_of("mvp"), Some(0));
        assert_eq!(l.offset_of("model"), Some(64));
        assert_eq!(l.offset_of("color"), Some(128));
        assert_eq!(l.offset_of("params"), Some(144));
        assert_eq!(l.size, 160);
        l.validate().unwrap();
    }

    /// 已知向量锁定：`Lights`（含 4 个 vec4 数组）。
    #[test]
    fn lights_known_vector() {
        let l = cube_lights_layout();
        assert_eq!(l.offset_of("counts"), Some(0));
        assert_eq!(l.offset_of("dirs"), Some(16));
        assert_eq!(l.offset_of("colors"), Some(80));
        assert_eq!(l.offset_of("points"), Some(144));
        assert_eq!(l.offset_of("point_colors"), Some(208));
        assert_eq!(l.offset_of("camera"), Some(272));
        assert_eq!(l.size, 288);
        l.validate().unwrap();
    }

    /// vec3 陷阱：size 12 但对齐 16 —— 下一字段必须跳过 4 字节空洞。
    /// 若把 vec3 当 12 字节连续排，wgsl 侧读到的就是错位数据。
    #[test]
    fn vec3_trap_forces_sixteen_byte_alignment() {
        let mut fields = vec![
            UniformField::new("a", UniformType::F32),
            UniformField::new("v", UniformType::F32x3),
            UniformField::new("b", UniformType::F32),
        ];
        let size = UniformLayout::compute_offsets(&mut fields);
        assert_eq!(fields[0].offset, 0);
        assert_eq!(fields[1].offset, 16, "vec3 必须对齐到 16");
        assert_eq!(fields[2].offset, 28, "vec3 size 仍为 12");
        assert_eq!(size, 32, "总大小向上取整到 16 的倍数");

        // 手工错写 offset ⇒ validate 显式拒绝（不静默）
        let bad = UniformLayout {
            fields: vec![
                UniformField {
                    name: "a",
                    ty: UniformType::F32,
                    offset: 0,
                },
                UniformField {
                    name: "v",
                    ty: UniformType::F32x3,
                    offset: 4,
                },
            ],
            size: 16,
        };
        assert!(matches!(
            bad.validate(),
            Err(UniformLayoutError::Misaligned { name: "v", .. })
        ));
    }

    /// 空布局被拒；size 不足以覆盖字段被拒。
    #[test]
    fn validate_rejects_empty_and_small_size() {
        let l = UniformLayout {
            fields: vec![],
            size: 0,
        };
        assert!(matches!(l.validate(), Err(UniformLayoutError::Empty)));

        let l = UniformLayout {
            fields: vec![UniformField {
                name: "m",
                ty: UniformType::Mat4,
                offset: 0,
            }],
            size: 32,
        };
        assert!(matches!(
            l.validate(),
            Err(UniformLayoutError::SizeTooSmall { .. })
        ));
    }
}

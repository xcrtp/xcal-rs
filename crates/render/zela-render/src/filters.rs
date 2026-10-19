// filters.rs — 合成后滤镜链（**纯数据**，C0–C3）。
//
// # 为什么是"链 + 固定槽位"而不是"每滤镜一条管线"
//
// wgpu 的两个硬限制：**无动态 blend**、**无动态入口点** ⇒ 滤镜若成为管线维度，
// 组合数按 2^N 爆炸。本模块把滤镜做成**数据**（固定 4 个槽位，`kind` + `params`
// 走 uniform），于是：
// - 管线数仍是 3 blend × 3 域转换 = 9（C1 的目标：**新增滤镜不改管线代码**）；
// - shader 内按 `kind` 分支 —— **uniform 分支**（同像素走同一分支，非 divergent），代价小。
//
// # 滤镜位置：post-composite（合成后整屏）
//
// 滤镜作用在**全部图层合成之后**（几何重映射类滤镜只有在这层才有意义：
// 放 per-layer 会把单层内容扭掉而其他层不动）。代价是必须多一张中间纹理 + 一次全屏
// pass（`compositor.rs` 的 post 路径）。
//
// # 预乘 alpha 硬约束（§10.5）
//
// 中间纹理里是**预乘 alpha** 的颜色（这是 `Alpha` 混合能用 `src: One` 的前提）。
// ⇒ **所有涉及插值的滤镜必须在预乘空间做**；在非预乘空间做模糊，半透明边缘会出现
// 黑边 / halo（透明黑 = (0,0,0,0) 会渗进 rgb）。本模块的三类滤镜都按此设计：
// - 点运算：**对 rgb 与 a 同乘**（`BrightnessGain`），或对 rgb 做 ≤1 的**增益**、
//   a 不动（`ColorGain`）—— 两者都保持 `rgb ≤ a`；
// - 几何重映射：改的是**采样坐标**，不参与插值 ⇒ 天然安全；
// - 邻域（模糊）：对预乘值做加权平均 ⇒ 安全（反之则会出黑边）。

/// 固定滤镜槽位数（C1 决策，见交付说明 §11.7）。
///
/// 为什么是 4：三类代表滤镜（点 / 几何 / 邻域）常用组合不超过 2–3 个；
/// 4 槽 = 144 B uniform（16 B 头 + 4×32 B），单帧一次写入，开销可忽略。
pub const MAX_FILTER_SLOTS: usize = 4;

/// post uniform 的字节数：
/// `vec4<u32>` 头（槽位数）+ `vec4<f32>` 全局参数（纹素步长）+ `array<Slot, 4>`。
pub const POST_UNIFORM_SIZE: u64 = 16 + 16 + (MAX_FILTER_SLOTS as u64) * 32;

/// 滤镜种类（`code()` **必须**与 `post_filter.wgsl` 的分支表一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterKind {
    /// 直通（不改变颜色）—— 用于"只走 post 路径"的对照（测试与调试）。
    #[default]
    None = 0,
    /// 点运算：亮度 / 色增益（`params = [r_gain, g_gain, b_gain, a_gain]`）。
    Gain = 1,
    /// 几何重映射（桶形/枕形畸变）：
    /// `params = [k1, k2, scale, unused]`，`uv' = center + (uv - center) * scale * (1 + k1 r² + k2 r⁴)`。
    Distortion = 2,
    /// 邻域：3×3 高斯模糊（`params = [strength, unused, unused, unused]`）。
    Blur = 3,
}

impl FilterKind {
    /// WGSL 分支用的数值码（与 shader 内 `KIND_*` 常量一一对应）。
    pub const fn code(self) -> u32 {
        self as u32
    }
}

/// 一个滤镜槽位（数据）：种类 + 4 个参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSlot {
    pub kind: FilterKind,
    /// 参数含义由 `kind` 决定（见各变体文档）；未用到的分量须为 0。
    pub params: [f32; 4],
}

impl FilterSlot {
    /// 直通槽（对照 / 调试用）。
    pub const fn none() -> Self {
        Self {
            kind: FilterKind::None,
            params: [0.0; 4],
        }
    }

    /// 亮度增益（**预乘安全**：rgb 与 a 同乘）。
    pub const fn brightness(gain: f32) -> Self {
        Self {
            kind: FilterKind::Gain,
            params: [gain, gain, gain, gain],
        }
    }

    /// 色增益（rgb 各自缩放、a 不动）。**增益须 ≤ 1** 才保持 `rgb ≤ a`（见模块头）。
    pub const fn color_gain(r: f32, g: f32, b: f32) -> Self {
        Self {
            kind: FilterKind::Gain,
            params: [r, g, b, 1.0],
        }
    }

    /// 桶形/枕形畸变（`k1 = 0` 时恒等；`k1 > 0` 桶形、`< 0` 枕形）。
    pub const fn distortion(k1: f32, k2: f32, scale: f32) -> Self {
        Self {
            kind: FilterKind::Distortion,
            params: [k1, k2, scale, 0.0],
        }
    }

    /// 3×3 高斯模糊（`strength ∈ [0,1]`，0 = 恒等）。
    pub const fn blur(strength: f32) -> Self {
        Self {
            kind: FilterKind::Blur,
            params: [strength, 0.0, 0.0, 0.0],
        }
    }
}

/// 合成后滤镜链（Resource）：顺序即应用顺序，最多 [`MAX_FILTER_SLOTS`] 个。
///
/// 空链 ⇒ 渲染层走**直通路径**（不建中间纹理、不多一次全屏 pass，画面逐位不变）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostFilterChain {
    pub slots: Vec<FilterSlot>,
}

impl PostFilterChain {
    /// 空链（默认；渲染层直通）。
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// 追加一个滤镜；超出 [`MAX_FILTER_SLOTS`] 的**被忽略**（不 panic、不静默扩容）。
    pub fn push(&mut self, slot: FilterSlot) -> &mut Self {
        if self.slots.len() < MAX_FILTER_SLOTS {
            self.slots.push(slot);
        }
        self
    }

    /// 清空（回到直通路径）。
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// 是否为空（= 渲染层是否走直通）。
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 编码为 post uniform 字节（`compositor.rs` 直接上传）。
    ///
    /// `texel_px`：中间纹理的**纹素步长** `(1/宽, 1/高)` —— 邻域滤镜（模糊）需要它。
    ///
    /// 布局（与 WGSL 逐字段对应）：`[count, 0, 0, 0]` + `[texel.x, texel.y, 0, 0]` +
    /// 每槽 `[kind_code, 0, 0, 0]` + `params[0..4]`；未填的槽位全 0。
    pub fn to_uniform_bytes(&self, texel_px: [f32; 2]) -> [u8; POST_UNIFORM_SIZE as usize] {
        let mut out = [0u8; POST_UNIFORM_SIZE as usize];
        let count = self.slots.len().min(MAX_FILTER_SLOTS) as u32;
        out[0..4].copy_from_slice(&count.to_le_bytes());
        out[16..20].copy_from_slice(&texel_px[0].to_le_bytes());
        out[20..24].copy_from_slice(&texel_px[1].to_le_bytes());
        for (i, slot) in self.slots.iter().take(MAX_FILTER_SLOTS).enumerate() {
            let base = 32 + i * 32;
            out[base..base + 4].copy_from_slice(&slot.kind.code().to_le_bytes());
            for (j, p) in slot.params.iter().enumerate() {
                let o = base + 16 + j * 4;
                out[o..o + 4].copy_from_slice(&p.to_le_bytes());
            }
        }
        out
    }

    /// 本链是否需要 post 路径（= 非空）。
    ///
    /// 注意：`FilterKind::None` 也算"需要" —— 它是一次**真实的全屏 pass**
    /// （用于验证"中间纹理路径本身不改变画面"，见 GPU 用例）。
    pub fn needs_post(&self) -> bool {
        !self.slots.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_shader_branch_table() {
        // 数值码是 WGSL 的 ABI：改动会让滤镜走错分支（shader 内同名常量）。
        assert_eq!(FilterKind::None.code(), 0);
        assert_eq!(FilterKind::Gain.code(), 1);
        assert_eq!(FilterKind::Distortion.code(), 2);
        assert_eq!(FilterKind::Blur.code(), 3);
        assert_eq!(FilterKind::default(), FilterKind::None);
    }

    #[test]
    fn uniform_layout_is_header_plus_four_slots() {
        assert_eq!(POST_UNIFORM_SIZE, 160);
        let mut chain = PostFilterChain::new();
        chain.push(FilterSlot::brightness(0.5));
        chain.push(FilterSlot::distortion(0.2, 0.05, 1.0));
        let bytes = chain.to_uniform_bytes([1.0 / 64.0, 1.0 / 32.0]);
        assert_eq!(bytes.len(), 160);
        // 头：count = 2；全局参数：纹素步长。
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 2);
        assert_eq!(
            f32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            1.0 / 64.0
        );
        assert_eq!(
            f32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            1.0 / 32.0
        );
        // 槽 0：kind = 1（Gain），params 全 0.5。
        assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 1);
        assert_eq!(f32::from_le_bytes(bytes[48..52].try_into().unwrap()), 0.5);
        // 槽 1：kind = 2（Distortion），k1 = 0.2。
        assert_eq!(u32::from_le_bytes(bytes[64..68].try_into().unwrap()), 2);
        assert_eq!(f32::from_le_bytes(bytes[80..84].try_into().unwrap()), 0.2);
        // 未用槽位全 0。
        assert!(bytes[96..].iter().all(|b| *b == 0));
    }

    #[test]
    fn chain_is_capped_at_four_slots() {
        let mut chain = PostFilterChain::new();
        for _ in 0..10 {
            chain.push(FilterSlot::blur(0.5));
        }
        assert_eq!(
            chain.slots.len(),
            MAX_FILTER_SLOTS,
            "超出槽位应被忽略（不扩容）"
        );
        // 编码里 count 也不超过槽位数。
        let bytes = chain.to_uniform_bytes([0.0, 0.0]);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 4);
    }

    #[test]
    fn empty_chain_uses_direct_path() {
        let chain = PostFilterChain::new();
        assert!(chain.is_empty());
        assert!(!chain.needs_post(), "空链必须走直通（画面逐位不变）");
        // 直通槽 ⇒ 需要 post（它是真实的一次全屏 pass）。
        let mut one = PostFilterChain::new();
        one.push(FilterSlot::none());
        assert!(one.needs_post());
        one.clear();
        assert!(!one.needs_post());
    }
}

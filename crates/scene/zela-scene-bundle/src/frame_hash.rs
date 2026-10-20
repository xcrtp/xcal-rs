// frame_hash.rs — 帧像素序列哈希（跨运行时画面对拍的规范口径）。
//
// # 为什么不用 mp4 / PNG 文件哈希
//
// 对编码产物算哈希会把**编码器版本、容器元数据、时间戳**带进基线：
// - 不可跨平台复现；
// - 浏览器侧不导出 mp4，第二阶段无法对拍。
//
// 本模块只覆盖**帧缓冲像素字节**（规范化为紧凑 RGBA8），因此任意一端
// （native headless / 浏览器 WebGPU 回读）只要拿到同一块像素，就能算出同一个值。
//
// # 规范 V1（两端必须逐字节一致）
//
// - 像素布局：RGBA8、行优先、左上原点、行距 `width * 4`（无对齐填充）、
//   8-bit 直色（非预乘）；
// - `state0 = SHA-256("ZELA-FRAME-SEQ-V1" || width_le_u32 || height_le_u32)`；
// - `digest_i = SHA-256(frame_i)`；
// - `state_{i+1} = SHA-256(state_i || i_le_u32 || digest_i)`；
// - 最终值 = `hex(state_n)`（小写十六进制，64 字符）。
//
// 尺寸进入初始状态，避免把不同分辨率的画面误判为同一序列；像素格式不进入——
// 两端都必须先**规范化**为上述 RGBA8（通道顺序按同一目标格式转换；V1 不做
// sRGB / 线性域转换，因此两端必须使用相同的目标纹理格式）。

use crate::sha256::{hex, sha256};

/// 逐帧累积的像素序列哈希（内存 O(1)，不保存历史帧）。
pub struct FrameSequenceHash {
    state: [u8; 32],
    width: u32,
    height: u32,
    frames: u32,
}

impl FrameSequenceHash {
    /// 以画布尺寸初始化（尺寸参与初始状态）。
    pub fn new(width: u32, height: u32) -> Self {
        let mut init = Vec::with_capacity(32);
        init.extend_from_slice(b"ZELA-FRAME-SEQ-V1");
        init.extend_from_slice(&width.to_le_bytes());
        init.extend_from_slice(&height.to_le_bytes());
        Self {
            state: sha256(&init),
            width,
            height,
            frames: 0,
        }
    }

    /// 追加一帧规范化的 RGBA8 像素（长度必须恰为 `width * height * 4`）。
    pub fn push_frame(&mut self, rgba: &[u8]) {
        let expected = self.width as usize * self.height as usize * 4;
        assert_eq!(
            rgba.len(),
            expected,
            "帧像素必须是紧凑 RGBA8（{expected} B = {0}×{1}×4）",
            self.width,
            self.height
        );

        let digest = sha256(rgba);
        let mut buf = Vec::with_capacity(32 + 4 + 32);
        buf.extend_from_slice(&self.state);
        buf.extend_from_slice(&self.frames.to_le_bytes());
        buf.extend_from_slice(&digest);
        self.state = sha256(&buf);
        self.frames += 1;
    }

    /// 当前序列哈希（小写十六进制）。序列为空时也可取（即初始状态）。
    pub fn hex(&self) -> String {
        hex(&self.state)
    }

    /// 已累积的帧数。
    pub fn frames(&self) -> u32 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector_single_pixel_two_frames() {
        // 与浏览器端共享的规范用例：1×1 RGBA8，红 → 绿。
        let mut h = FrameSequenceHash::new(1, 1);
        h.push_frame(&[255, 0, 0, 255]);
        h.push_frame(&[0, 255, 0, 255]);
        assert_eq!(
            h.hex(),
            "7716117571aeb0172735668f994d56e6c039cfedb890fa2c12023a576a380f0c",
            "算法或编码变化会破坏跨运行时对拍"
        );
        assert_eq!(h.frames(), 2);
    }

    #[test]
    fn same_sequence_same_hash() {
        let frames: [[u8; 8]; 3] = [
            [1, 2, 3, 4, 5, 6, 7, 8],
            [9, 10, 11, 12, 13, 14, 15, 16],
            [17, 18, 19, 20, 21, 22, 23, 24],
        ];
        let mut a = FrameSequenceHash::new(2, 1);
        let mut b = FrameSequenceHash::new(2, 1);
        for f in &frames {
            a.push_frame(f);
            b.push_frame(f);
        }
        assert_eq!(a.hex(), b.hex());
    }

    #[test]
    fn frame_order_and_size_matter() {
        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];

        let mut a = FrameSequenceHash::new(1, 1);
        a.push_frame(&red);
        a.push_frame(&green);

        let mut b = FrameSequenceHash::new(1, 1);
        b.push_frame(&green);
        b.push_frame(&red);

        assert_ne!(a.hex(), b.hex(), "帧序必须进入哈希");

        let mut c = FrameSequenceHash::new(1, 1);
        c.push_frame(&red);
        let mut d = FrameSequenceHash::new(1, 2);
        d.push_frame(&[255, 0, 0, 255, 0, 0, 0, 0]);
        assert_ne!(c.hex(), d.hex(), "尺寸必须进入初始状态");
    }

    #[test]
    #[should_panic(expected = "紧凑 RGBA8")]
    fn wrong_pixel_len_panics() {
        let mut h = FrameSequenceHash::new(2, 2);
        h.push_frame(&[0; 15]);
    }
}

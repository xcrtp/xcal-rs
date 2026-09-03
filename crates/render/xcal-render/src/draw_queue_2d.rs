// draw_queue_2d.rs
//
// 新风格 2D 绘制队列（预留）。
//
// 注意：旧 DrawQueue（draw.rs，稳定归并排序 + ZIndex）是冻结 API，保持不变。
// 本模块是面向 Shape2D/Style2D 新图元体系的命令队列，排序语义保持
// 与旧队列一致（稳定排序：先按 z 升序，同 z 保序），供新管线使用。

use xc_geometry::Transform2D;

use crate::painter_2d::Style2D;
use crate::shape::Shape2D;

/// 一条 2D 绘制命令：图元 + 变换 + 样式 + 层序。
#[derive(Debug, Clone)]
pub struct DrawCommand2D {
    pub shape: Shape2D,
    pub transform: Transform2D,
    pub style: Style2D,
    pub z: i32,
}

/// 新风格 2D 绘制队列。
///
/// 排序为**稳定**排序：先按 z 升序，z 相同则保持录制顺序
/// （与旧 DrawQueue 的冻结语义一致）。
#[derive(Default)]
pub struct DrawQueue2D {
    commands: Vec<DrawCommand2D>,
}

impl DrawQueue2D {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            commands: Vec::with_capacity(cap),
        }
    }

    /// 追加一条绘制命令。
    pub fn push(&mut self, command: DrawCommand2D) {
        self.commands.push(command);
    }

    /// 便捷追加（图元 + z，使用默认变换与样式）。
    pub fn draw(&mut self, shape: Shape2D, z: i32) {
        self.push(DrawCommand2D {
            shape,
            transform: Transform2D::identity(),
            style: Style2D::default(),
            z,
        });
    }

    /// 清空队列（每帧录制前调用）。
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// 返回按 (z 升序, 录制顺序) 稳定排序后的命令序列。
    /// 不修改内部录制顺序（排序结果由调用方消费）。
    pub fn sorted_commands(&self) -> Vec<&DrawCommand2D> {
        let mut refs: Vec<&DrawCommand2D> = self.commands.iter().collect();
        // sort_by_key 为稳定排序：z 相同则保留录制顺序。
        refs.sort_by_key(|c| c.z);
        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_geometry::Vec2;

    #[test]
    fn test_stable_order_preserved_within_same_z() {
        let mut q = DrawQueue2D::new();
        q.draw(Shape2D::Empty, 0);
        q.draw(
            Shape2D::Circle {
                center: Vec2::ZERO,
                radius: 1.0,
            },
            0,
        );
        q.draw(Shape2D::Empty, 0);

        let sorted = q.sorted_commands();
        assert_eq!(sorted.len(), 3);
        assert!(matches!(sorted[1].shape, Shape2D::Circle { .. }));
    }

    #[test]
    fn test_sorted_by_z() {
        let mut q = DrawQueue2D::new();
        q.draw(Shape2D::Empty, 5);
        q.draw(Shape2D::Empty, 1);
        q.draw(Shape2D::Empty, 3);

        let zs: Vec<i32> = q.sorted_commands().iter().map(|c| c.z).collect();
        assert_eq!(zs, vec![1, 3, 5]);
    }

    #[test]
    fn test_clear() {
        let mut q = DrawQueue2D::new();
        q.draw(Shape2D::Empty, 0);
        assert_eq!(q.len(), 1);
        q.clear();
        assert!(q.is_empty());
    }
}

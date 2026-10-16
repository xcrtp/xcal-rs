// event.rs — 场景语义事件（向上契约：UI → 场景）
//
// UI 适配器把原生输入翻译成 [`SceneEvent`]；事件里不得有任何 `egui::` 类型。
// 坐标一律是场景坐标（换算由适配器借助 `Camera2D::screen_to_world` 完成）；
// 控件按 [`WidgetId`] 标识——场景侧只认 ID，不知道控件长什么样。
//
// [`InputEvents`] 是普通 Vec，可 headless 构造 → 场景 System 可以脱离窗口单测。

use xc_notion::Vec2;

/// 控件唯一标识（场景契约的一部分，UI 侧按 ID 发事件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WidgetId(pub u32);

/// 集中定义的控件 ID（当前全部控件；新增控件在此登记）。
pub mod ids {
    use super::WidgetId;

    // ── 控制面板（UiConfig）──
    pub const SHOW_GRID: WidgetId = WidgetId(1);
    pub const SHOW_FIELD: WidgetId = WidgetId(2);

    // ── 时间控制（DirectorState）──
    pub const PLAY_PAUSE: WidgetId = WidgetId(3);
    pub const SPEED: WidgetId = WidgetId(4);
    pub const SEEK: WidgetId = WidgetId(5);

    // ── 参数面板（BindingTable）──
    pub const CIRCLE_BASE: WidgetId = WidgetId(10);
    pub const CIRCLE_AMP: WidgetId = WidgetId(11);
    pub const CIRCLE_OMEGA: WidgetId = WidgetId(12);
    pub const PHASE_OMEGA: WidgetId = WidgetId(13);
    pub const ORBIT_RADIUS: WidgetId = WidgetId(14);
    pub const ORBIT_SPEED: WidgetId = WidgetId(15);
    pub const ORBIT_ANGLE: WidgetId = WidgetId(16);
}

/// 场景语义事件。
///
/// 注：不派生 `PartialEq`——`Vec2` 包装类型未实现（含 f32 语义问题）；
/// 测试中用 `matches!` 按字段断言。
#[derive(Debug, Clone, Copy)]
pub enum SceneEvent {
    // ─── 视口拾取（坐标已换算为场景坐标；非相机操作）───
    ViewportHover { layer: LayerId, scene_pos: Vec2 },
    ViewportClick { layer: LayerId, scene_pos: Vec2 },

    // ─── 相机命令（语义化，不含 center/scale/eye 等字段名）───
    // 命令如何改变相机状态由相机控制器按 CameraMode 决定（PanZoom/Orbit/Path）。
    Camera {
        layer: LayerId,
        command: crate::domain::camera::CameraCommand,
    },

    // ─── 控件事件（按 ID 标识，场景侧认不出 egui）───
    Toggle { id: WidgetId, value: bool },
    Slider { id: WidgetId, value: f32 },
    Button { id: WidgetId },
}

use crate::domain::view::LayerId;

/// 每帧收集的事件（Resource，由 UI 适配器填充、System 消费）。
#[derive(Debug, Clone, Default)]
pub struct InputEvents {
    events: Vec<SceneEvent>,
}

impl InputEvents {
    pub fn from_events(events: Vec<SceneEvent>) -> Self {
        Self { events }
    }

    pub fn push(&mut self, event: SceneEvent) {
        self.events.push(event);
    }

    pub fn extend<I: IntoIterator<Item = SceneEvent>>(&mut self, events: I) {
        self.events.extend(events);
    }

    pub fn iter(&self) -> impl Iterator<Item = &SceneEvent> {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 取走全部事件（清空队列）。
    pub fn drain(&mut self) -> std::vec::Drain<'_, SceneEvent> {
        self.events.drain(..)
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::view::LayerId;

    /// headless 构造：不需要任何窗口/UI 上下文。
    #[test]
    fn input_events_can_be_constructed_headless() {
        let mut events = InputEvents::default();
        assert!(events.is_empty());

        events.push(SceneEvent::Camera {
            layer: LayerId(0),
            command: crate::domain::CameraCommand::Zoom { factor: 1.1 },
        });
        events.push(SceneEvent::Button { id: ids::PLAY_PAUSE });
        events.push(SceneEvent::Slider {
            id: ids::SPEED,
            value: 2.0,
        });

        assert_eq!(events.len(), 3);
        assert!(matches!(
            events.iter().next(),
            Some(SceneEvent::Camera {
                command: crate::domain::CameraCommand::Zoom { factor },
                ..
            }) if *factor == 1.1
        ));

        // drain 取走后清空
        let drained: Vec<_> = events.drain().collect();
        assert_eq!(drained.len(), 3);
        assert!(events.is_empty());
    }
}

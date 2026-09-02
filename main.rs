//! 自包含、解耦的 hecs 最小骨架
//!
//! 分层（每层只依赖自己需要的接口，互不耦合）：
//! - `components` : 纯数据，零逻辑
//! - `resources`  : 全局资源（时间/输入/…），与 ECS World 平级，靠参数传入
//! - `systems`    : 普通函数，只通过 (World, Resources) 访问数据
//! - `schedule`   : 决定 system 的执行顺序（唯一知道顺序的地方）
//! - `CommandBuffer`: 延迟写，避免遍历中改 World
//! - `main`       : 固定步长主循环，与逻辑完全分离

use std::time::{Duration, Instant};

use hecs::{CommandBuffer, World};

// ─── 1. Components ────────────────────────────────────────────────────────
// 纯数据，不含逻辑，不引用 World。可跨系统自由组合。

#[derive(Debug, Clone, Copy)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Velocity {
    pub x: f32,
    pub y: f32,
}

/// Tag 组件：零字段，只用于查询过滤。
#[derive(Debug, Clone, Copy)]
pub struct Player;

// ─── 2. Resources ─────────────────────────────────────────────────────────
// 与 World 平级的全局状态。hecs 不提供 Resource，故自建一个结构体。
// 各 system 只借用自己需要的字段，避免“大 struct 全量借用”冲突。

#[derive(Debug)]
pub struct Time {
    pub delta: Duration,   // 上一帧耗时
    pub elapsed: Duration, // 累计时间
}

#[derive(Debug, Default)]
pub struct Input {
    pub move_right: bool,
    pub jump: bool,
}

// ─── 3. Commands（延迟写） ───────────────────────────────────────────────
// 用 hecs 自带的 CommandBuffer，遍历中记录 spawn/despawn/insert，
// 安全点一次性提交，避免遍历中改 World 的借用冲突。

#[derive(Debug, Default)]
pub struct Commands {
    buf: CommandBuffer,
}

impl Commands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(&mut self, bundle: impl hecs::DynamicBundle) {
        self.buf.spawn(bundle);
    }

    pub fn despawn(&mut self, e: hecs::Entity) {
        self.buf.despawn(e);
    }

    pub fn insert(&mut self, e: hecs::Entity, bundle: impl hecs::DynamicBundle) {
        self.buf.insert(e, bundle);
    }

    /// 主循环每帧调用一次：把缓冲应用到 World，然后清空。
    pub fn apply(&mut self, world: &mut World) {
        self.buf.run_on(world);
        // run_on 会消费内部状态；这里重建一个空 buffer 以复用 Commands
        self.buf = CommandBuffer::new();
    }
}

// ─── 4. Systems（普通函数，完全解耦） ────────────────────────────────────
// 规则：
// - 不持有 World/Resources，全部靠参数传入（依赖注入）
// - 一个 system 只做一件事，方便组合/替换/并行化

/// 移动系统：Velocity 累加到 Position。
pub fn movement_system(world: &mut World) {
    for (_id, (pos, vel)) in world.query_mut::<(&mut Position, &Velocity)>() {
        pos.x += vel.x;
        pos.y += vel.y;
    }
}

/// 输入系统：根据 Input 修改玩家 Velocity（示范 Resources 只读借用）。
pub fn input_system(world: &mut World, input: &Input) {
    for (_id, (vel, _)) in world.query_mut::<(&mut Velocity, &Player)>() {
        vel.x = if input.move_right { 1.0 } else { 0.0 };
    }
}

/// 生命周期系统：示范用 Commands 延迟销毁（遍历中不直接 despawn）。
/// 这里把“寿命耗尽”判定就地做，仅记录命令，实际 apply 在 schedule 层。
pub fn lifetime_system(world: &World, cmds: &mut Commands, time: &Time) {
    for (id, _pos) in world.query::<&Position>() {
        // 例：y 超出边界则销毁；真实项目换成寿命/血量等
        if time.elapsed.as_secs() > 5 && id.index() % 2 == 0 {
            cmds.despawn(id);
        }
    }
}

/// 演示系统：spawn 新实体也走 Commands，保证遍历期不触碰 World。
pub fn spawn_system(cmds: &mut Commands, time: &Time) {
    if time.elapsed.as_secs() % 3 == 0 {
        cmds.spawn((Position { x: 0.0, y: 0.0 }, Velocity { x: 0.5, y: 0.0 }));
    }
}

// ─── 5. Schedule（唯一知道 system 执行顺序的地方） ───────────────────────
// 把“做什么”和“按什么顺序做”集中在此，main 循环不关心具体系统。

#[derive(Debug, Default)]
pub struct Schedule {
    pub commands: Commands,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// 一帧的逻辑更新。Resources 由调用方传入，保持解耦。
    pub fn update(&mut self, world: &mut World, time: &Time, input: &Input) {
        // 顺序敏感时按需要排列；互相独立的 system 将来可并行
        input_system(world, input);
        movement_system(world);

        // 读侧/写侧命令收集（只读 World 的 system 放这里）
        lifetime_system(world, &mut self.commands, time);
        spawn_system(&mut self.commands, time);

        // 遍历全部结束后，一次性提交延迟写
        self.commands.apply(world);
    }
}

// ─── 6. Main：固定步长主循环（与逻辑完全分离） ──────────────────────────

const FIXED_DT: Duration = Duration::from_millis(16); // ≈60Hz 逻辑步长

fn setup(world: &mut World) {
    world.spawn((Position { x: 0.0, y: 0.0 }, Velocity { x: 1.0, y: 0.0 }, Player));
    world.spawn((Position { x: 5.0, y: 0.0 }, Velocity { x: 0.0, y: 0.5 }));
}

fn main() {
    let mut world = World::new();
    let mut schedule = Schedule::new();
    let mut time = Time {
        delta: Duration::ZERO,
        elapsed: Duration::ZERO,
    };
    let input = Input::default();

    setup(&mut world);

    // 固定步长循环：accumulator 保证逻辑更新与帧率解耦
    let mut last = Instant::now();
    let mut accumulator = Duration::ZERO;

    // 运行若干逻辑步即退出（示例用，真实程序用事件循环）
    let mut steps = 0u32;
    while steps < 120 {
        let now = Instant::now();
        let frame_time = now - last;
        last = now;
        accumulator += frame_time;

        while accumulator >= FIXED_DT {
            time.delta = FIXED_DT;
            time.elapsed += FIXED_DT;

            schedule.update(&mut world, &time, &input);

            accumulator -= FIXED_DT;
            steps += 1;
        }

        // 此处可做渲染/插值（使用 accumulator/FIXED_DT 做 alpha）
        // render(&world, accumulator.as_secs_f32() / FIXED_DT.as_secs_f32());
    }

    // 打印结果便于验证
    println!("final entities:");
    for (_id, (pos, vel)) in world.query::<(&Position, &Velocity)>() {
        println!("  pos={:?} vel={:?}", pos, vel);
    }
    println!("total: {}", world.len());
}

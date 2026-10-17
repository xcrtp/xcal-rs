// content.rs — **内容编译能力**（Injector 的第二次真实使用）。
//
// # 为什么是独立 Service，而不是 SceneApi 里的两个函数
//
// 公式 / 文字需要 `InMemoryEngine`（App 侧建一次的重资源）。把它塞进场景服务会让
// "场景装配" 与 "内容排版" 耦合；经 `Injector` 注入成第二个 Service，才说明
// 注入机制服务的是**能力**，不是某一个模块。
//
// # 为什么返回数据而不是对象
//
// `content.formula(...)` 返回 `Array<Path>`（按**阅读顺序**）。"阅读顺序" 是宿主
// 语义，不是通用场景操作 —— 脚本只消费结果，决定 spawn 几个对象。
//
// # 与 `xcal-app::formula` 的关系
//
// 落点转换（IR → 展平子路径 → 世界坐标）与 App 的 `formula.rs` 逐式一致
// （同一份 `xc-content` / `xc-notion` 原语）；本模块不依赖 `xcal-render`
// （那会让 scene 层反向依赖 render 层）。

use std::any::Any;
use std::sync::{Arc, Mutex};

use xc_content::{ContentIR, StyledPath};
use xc_notion::{Color32 as NotionColor, Path2D, Transform2D, Vec2 as NotionVec2};
use xc_prerender::{ContentConfig, ContentEngine, InMemoryEngine};
use xc_pod::{FunctionId, Handle, HandleTable, QualifiedName, RegistryPod, Pod, PodError, PodId};

use crate::pod::types::{Color, Formula, Path, PathList, Text};

/// 注入时使用的 Service 名。
pub const CONTENT_SERVICE_NAME: &str = "content";

/// 曲线展平容差（pt，相对标称字号尺度）——与 App 的 `formula.rs` 同值。
const FLATTEN_TOLERANCE: f32 = 0.1;
/// 只有 fill、没有 stroke 时的默认描边宽度（标称 pt）。
const DEFAULT_STROKE_WIDTH_PT: f32 = 1.0;
/// Typst 编译用的标称字号（pt）。
const DEFAULT_NOMINAL_FONT_SIZE: f32 = 12.0;
/// 子路径首尾点距离 ≤ 此值视为闭合。
const CLOSE_EPS: f32 = 1e-4;

/// **内容路径表**：内容服务写入、场景服务读取的共享资源。
///
/// 句柄 owner 是本表自己的 `PodId`（不是两个 Service 之一）——
/// 两个 Service 都只是这张表的用户，跨 Service 传递因此不触发
/// [`xc_pod::HandleTable`] 的跨 Pod 检查。
#[derive(Clone)]
pub struct PathStore(Arc<Mutex<PathStoreInner>>);

struct PathStoreInner {
    owner: PodId,
    table: HandleTable<Path2D>,
}

impl Default for PathStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PathStore {
    pub fn new() -> Self {
        let owner = PodId::next();
        Self(Arc::new(Mutex::new(PathStoreInner {
            owner,
            table: HandleTable::new(owner),
        })))
    }

    pub(crate) fn insert(&self, path: Path2D) -> Handle {
        self.0.lock().expect("路径表锁中毒").table.insert(path)
    }

    pub(crate) fn get(&self, handle: &Handle) -> Result<Path2D, PodError> {
        self.0
            .lock()
            .expect("路径表锁中毒")
            .table
            .get(handle)
            .cloned()
    }

    /// 每轮脚本开始前清空（旧句柄随之失效）。
    pub(crate) fn reset(&self) {
        let mut inner = self.0.lock().expect("路径表锁中毒");
        inner.owner = PodId::next();
        inner.table = HandleTable::new(inner.owner);
    }
}

/// 宿主侧内容能力 Pod。
pub struct ContentServicePod {
    id: PodId,
    registry: RegistryPod,
    engine: Arc<InMemoryEngine>,
    paths: PathStore,
}

impl ContentServicePod {
    /// 新建并注册 `formula` / `text`。
    pub fn new(engine: Arc<InMemoryEngine>, paths: PathStore) -> Self {
        let id = PodId::next();
        let mut registry = RegistryPod::new();
        registry.register_fn1::<Formula, PathList>("xcal.content::formula", content_formula);
        registry.register_fn1::<Text, PathList>("xcal.content::text", content_text);
        Self {
            id,
            registry,
            engine,
            paths,
        }
    }
}

impl Pod for ContentServicePod {
    fn id(&self) -> PodId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }

    fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        Err(PodError::CallFailed(format!(
            "ContentServicePod 的 `{id}` 只支持 typed 快路径（脚本经 ContentApi 调用）"
        )))
    }
}

fn service(pod: &dyn Pod) -> &ContentServicePod {
    pod.as_any()
        .downcast_ref::<ContentServicePod>()
        .expect("ContentApi 的调用者必须是 ContentServicePod")
}

// ─── typed 函数实现 ─────────────────────────────────────────────────────

fn content_formula(pod: &dyn Pod, spec: Formula) -> PathList {
    compile(pod, &spec.source, spec.font_size, spec.x, spec.y, true)
}

fn content_text(pod: &dyn Pod, spec: Text) -> PathList {
    let source = text_literal_source(&spec.source);
    compile(pod, &source, spec.font_size, spec.x, spec.y, false)
}

fn compile(
    pod: &dyn Pod,
    source: &str,
    font_size: f32,
    x: f32,
    y: f32,
    sort_reading_order: bool,
) -> PathList {
    let this = service(pod);
    let config = ContentConfig {
        font_size: DEFAULT_NOMINAL_FONT_SIZE,
        fill: NotionColor::WHITE,
        scale_factor: 1.0,
    };
    let handle = this.engine.request(source, &config);
    let Some(Ok(ir)) = this.engine.take(handle) else {
        return PathList(Vec::new());
    };
    PathList(convert(
        &this.paths,
        &ir,
        font_size,
        NotionVec2::new(x, y),
        sort_reading_order,
    ))
}

/// IR → 世界坐标描边路径（逐式对应 `xcal-app::formula::compile_to_stroke_paths`）。
fn convert(
    store: &PathStore,
    ir: &ContentIR,
    font_size: f32,
    world_pos: NotionVec2,
    sort_reading_order: bool,
) -> Vec<Path> {
    if ir.is_empty() {
        return Vec::new();
    }
    let scale = if ir.nominal_font_size > 0.0 {
        font_size / ir.nominal_font_size
    } else {
        1.0
    };
    let transform =
        Transform2D::scale(NotionVec2::splat(scale)).compose(Transform2D::translate(world_pos));

    let mut built: Vec<(Path2D, Color, f32)> = Vec::new();
    for item in &ir.items {
        let Some((color, width)) = stroke_style(item, scale) else {
            continue;
        };
        for sub in item.path.flatten_subpaths(FLATTEN_TOLERANCE) {
            if sub.is_empty() {
                continue;
            }
            let closed = is_closed(&sub);
            let world: Vec<NotionVec2> = sub.iter().map(|p| transform.apply_point(*p)).collect();
            built.push((
                Path2D::from_polyline(&world, closed),
                to_color(color),
                width,
            ));
        }
    }

    if sort_reading_order {
        order_by_reading_order(&mut built);
    }

    built
        .into_iter()
        .map(|(path, color, width)| Path {
            path: store.insert(path),
            color,
            width,
        })
        .collect()
}

/// 描边色 / 宽度回退规则（与 `formula.rs` 同口径）。
fn stroke_style(item: &StyledPath, scale: f32) -> Option<(NotionColor, f32)> {
    let default_w = DEFAULT_STROKE_WIDTH_PT * scale;
    match (&item.stroke, item.fill) {
        (Some(stroke), _) if stroke.width > 0.0 => Some((stroke.color, stroke.width * scale)),
        (Some(stroke), _) => Some((stroke.color, default_w)),
        (None, Some(fill)) => Some((fill, default_w)),
        (None, None) => None,
    }
}

fn is_closed(sub: &[NotionVec2]) -> bool {
    match (sub.first(), sub.last()) {
        (Some(a), Some(b)) => sub.len() >= 2 && (*a - *b).length() <= CLOSE_EPS,
        _ => false,
    }
}

fn to_color(color: NotionColor) -> Color {
    Color {
        r: color.r(),
        g: color.g(),
        b: color.b(),
        a: color.a(),
    }
}

/// 阅读顺序：包围盒 `min_x` 升序；无包围盒排最后；同 `min_x` 保持输入顺序。
///
/// 与 `xcal-app::storyboard::order_by_reading_order` 同口径（稳定排序）。
fn order_by_reading_order(paths: &mut [(Path2D, Color, f32)]) {
    paths.sort_by(|a, b| {
        let key = |p: &(Path2D, Color, f32)| p.0.compute_bounds().map(|r| r.min.x);
        match (key(a), key(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

/// 纯文本 → Typst 数学模式下的字符串字面量（直立正文外观）。
fn text_literal_source(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

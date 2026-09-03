// typst/world.rs — 极简 TypstWorld：内存主文件、无真实文件系统、系统发现字体。
//
// 设计要点：
// - math 主场景必须有至少一个带数学表的字体（Latin Modern Math / Cambria Math / STIX Two Math）。
//   优先系统发现；找不到时给用户明确提示。
// - 所有加载函数（main/source/file/font）做内部缓存（Typst 官方设计意图，comemo 记忆化前置）。
// - main source 与原始 source 分离：包装后的 #set / $..$ 文本存入 main；
//   原始归一化文本另存，用于 span → 归一化字节区间的"再映射"（convert.rs 用）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, Span};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};

/// 共享单例：Library（跨 World 实例复用）。
fn library() -> &'static LazyHash<Library> {
    static LIB: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIB.get_or_init(|| LazyHash::new(Library::builder().build()))
}

/// 共享单例：字体列表（系统发现）。第一次做 O(字体文件数) IO，之后复用。
fn fonts() -> &'static (LazyHash<FontBook>, Vec<Font>) {
    static F: OnceLock<(LazyHash<FontBook>, Vec<Font>)> = OnceLock::new();
    F.get_or_init(discover_fonts)
}

fn discover_fonts() -> (LazyHash<FontBook>, Vec<Font>) {
    let mut book = FontBook::new();
    let mut fonts: Vec<Font> = Vec::new();

    // 系统字体目录（fontdb 发现 + typst Font::new 解析）。
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for face in db.faces() {
        let id = face.id;
        let Some((data, face_index)) = db.with_face_data(id, |data, i| (data.to_vec(), i)) else {
            continue;
        };
        let bytes = Bytes::new(data);
        if let Some(font) = Font::new(bytes, face_index as u32) {
            book.push(font.info().clone());
            fonts.push(font);
        }
    }

    // math 字体兜底：如果 book 里没任何数学字体，给出警告（但不 panic）。
    let has_math = book.families().any(|(family_name, _)| is_math_family(family_name));
    if !has_math {
        eprintln!(
            "[xc-prerender::typst] 警告：未检测到支持数学表的字体（Latin Modern Math / STIX / Cambria Math 等）。\
             复杂数学公式将无法正确显示。"
        );
    }

    (LazyHash::new(book), fonts)
}

fn is_math_family(f: &str) -> bool {
    let low = f.to_ascii_lowercase();
    low.contains("math")
        || low.contains("cambria")
        || low.contains("latin modern")
        || low.contains("stix")
        || low.contains("asana")
        || low.contains("garamond")
}

/// 虚拟 FileId（无根；每个 World 一个 id）。
fn make_main_id() -> FileId {
    use typst::syntax::VirtualPath;
    FileId::new(None, VirtualPath::new("main.typ"))
}

/// 一个可单请求复用的 World 实现。
pub struct TypstWorld {
    main_id: FileId,
    main_source: Mutex<Source>,
    original_source: Mutex<String>,
    /// body（用户原始归一化源码）在 wrapped_src 中的字节偏移。
    /// span_to_range 返回 wrapped_src 坐标，减去 body_offset 即得原始坐标。
    body_offset: Mutex<usize>,
    sources_by_id: Mutex<HashMap<FileId, Source>>,
}

impl TypstWorld {
    pub fn new() -> Self {
        let main_id = make_main_id();
        let main_source = Source::new(main_id, String::new());
        Self {
            main_id,
            main_source: Mutex::new(main_source),
            original_source: Mutex::new(String::new()),
            body_offset: Mutex::new(0),
            sources_by_id: Mutex::new(HashMap::new()),
        }
    }

    /// 设置主文件源码（每次请求调用一次）。
    pub fn set_main_source(&self, text: impl Into<String>) {
        let src = Source::new(self.main_id, text.into());
        self.sources_by_id
            .lock()
            .unwrap()
            .insert(self.main_id, src.clone());
        *self.main_source.lock().unwrap() = src;
    }

    /// 保存原始归一化源码（供 convert 做 span 再映射）。
    pub fn set_original_source(&self, text: impl Into<String>) {
        *self.original_source.lock().unwrap() = text.into();
    }

    /// 取得原始归一化源码文本。
    pub fn original_source(&self) -> String {
        self.original_source.lock().unwrap().clone()
    }

    /// 设置 body 在 wrapped_src 中的字节偏移（compile 时算好写入）。
    pub fn set_body_offset(&self, offset: usize) {
        *self.body_offset.lock().unwrap() = offset;
    }

    /// 取得 body 偏移。
    pub fn body_offset(&self) -> usize {
        *self.body_offset.lock().unwrap()
    }

    /// 把 typst Span 转换为 main 文件中的字节区间。
    ///
    /// 如果 span.id() 不属于 main 文件，返回 None（锚点可空，按计划 3.2 允许）。
    pub fn span_to_range(&self, span: Span) -> Option<std::ops::Range<usize>> {
        // Typst Span::id() 返回 Option<FileId>。
        if span.id() != Some(self.main_id) {
            return None;
        }
        // WorldExt::range(self, span) 已经返回 Option<Range<usize>>（docs.rs 显示为 Result 不准确）。
        // 实际是 Option：typst-syntax 对于虚拟文件可能失败。
        WorldExt::range(self, span)
    }
}

impl Default for TypstWorld {
    fn default() -> Self { Self::new() }
}

impl World for TypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &fonts().0
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        let map = self.sources_by_id.lock().unwrap();
        map.get(&id)
            .cloned()
            .ok_or_else(|| {
                let path = id.vpath().as_rooted_path().to_path_buf();
                FileError::NotFound(path)
            })
    }

    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        // 短文本场景不需要真实文件系统。
        Err(FileError::NotFound(PathBuf::from("<virtual>")))
    }

    fn font(&self, index: usize) -> Option<Font> {
        fonts().1.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        // 公式排版不关心日期；返回 None 让 datetime() 报可控错误即可。
        None
    }
}

//! `zela-compile`：把脚本编译成 ZSB 装配产物。
//!
//! ```text
//! zela-compile <脚本路径> --out FILE [--json FILE]
//! ```
//!
//! - 输入：`.lua` / `.py`（语言按扩展名推断）；
//! - 装配：复用 `zela_app::compile_script`（与 GUI / `--script` **同一条路径**）；
//! - 输出：`.zsb`（Header + Payload + Meta）；`--json` 额外写出 payload 明文，
//!   便于调试 diff（明文**不参与**内容 hash）。
//!
//! 编译器与回放端是两个独立二进制：`SpecId` 用**名字**（不是注册序号），
//! 因此两侧注册顺序不同也不会画错。

use std::path::PathBuf;

use zela_app::{ScriptLanguage, compile_script, language_from_path};
use zela_scene_bundle::{Bundle, MetaInput};

struct Args {
    script: PathBuf,
    out: PathBuf,
    json: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() || raw.iter().any(|a| a == "--help" || a == "-h") {
            return Err("用法：zela-compile <脚本路径> --out FILE [--json FILE]".to_string());
        }

        let mut script: Option<PathBuf> = None;
        let mut out: Option<PathBuf> = None;
        let mut json = None;

        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--out" => {
                    i += 1;
                    out = Some(PathBuf::from(raw.get(i).ok_or("--out 需要一个文件参数")?));
                }
                "--json" => {
                    i += 1;
                    json = Some(PathBuf::from(raw.get(i).ok_or("--json 需要一个文件参数")?));
                }
                other if other.starts_with("--") => {
                    return Err(format!("未知参数：{other}"));
                }
                other => {
                    if script.is_some() {
                        return Err(format!("多余的参数：{other}"));
                    }
                    script = Some(PathBuf::from(other));
                }
            }
            i += 1;
        }

        Ok(Self {
            script: script.ok_or("缺少脚本路径")?,
            out: out.ok_or("缺少 --out")?,
            json,
        })
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("zela-compile: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;

    let source = std::fs::read_to_string(&args.script)
        .map_err(|e| format!("读取脚本 {} 失败：{e}", args.script.display()))?;
    let language = language_from_path(&args.script).map_err(|e| e.to_string())?;
    if language == ScriptLanguage::Python && !cfg!(feature = "python") {
        return Err(
            "脚本是 .py，但本二进制编译时未启用 `python` feature（请重新构建或改用 .lua）"
                .to_string(),
        );
    }
    if language == ScriptLanguage::JavaScript && !cfg!(feature = "js") {
        return Err(
            "脚本是 .js，但本二进制编译时未启用 `js` feature（请重新构建或改用 .lua）"
                .to_string(),
        );
    }

    let compiled = compile_script(&source, language).map_err(|e| e.to_string())?;
    let bundle = Bundle::encode(
        &compiled.script,
        MetaInput {
            capabilities: compiled.capabilities.clone(),
            required_capabilities: compiled.required_capabilities.clone(),
            pods: compiled.pods.clone(),
            required_pods: compiled.required_pods.clone(),
            // 当前脚本不使用 Shape2D::Custom ⇒ 无降级记录。
            downgrades: Vec::new(),
            source_sha256: Some(compiled.source_sha256.clone()),
        },
    )
    .map_err(|e| format!("编码 ZSB 失败：{e}"))?;

    let bytes = bundle.to_bytes();
    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败：{e}", parent.display()))?;
        }
    }
    std::fs::write(&args.out, &bytes)
        .map_err(|e| format!("写 {} 失败：{e}", args.out.display()))?;

    if let Some(json_path) = &args.json {
        std::fs::write(json_path, bundle.payload())
            .map_err(|e| format!("写 {} 失败：{e}", json_path.display()))?;
    }

    eprintln!(
        "[zela-compile] {} → {}（{} 个对象，{}s）\n  payload hash = {}\n  capabilities = {:?}\n  pods = {:?}",
        args.script.display(),
        args.out.display(),
        bundle.meta().object_count,
        bundle.meta().duration,
        bundle.payload_hash_hex(),
        bundle
            .meta()
            .capabilities
            .iter()
            .map(|c| format!("{}@{:?}", c.name, c.version))
            .collect::<Vec<_>>(),
        bundle
            .meta()
            .pods
            .iter()
            .map(|p| format!("{}@{}", p.name, p.version))
            .collect::<Vec<_>>(),
    );
    Ok(())
}

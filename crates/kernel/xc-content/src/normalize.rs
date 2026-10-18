// normalize.rs — 跨引擎共用的源码归一化 + 哈希。
// 规则：去注释（// 与 /* */）+ 折叠空白（多空白 → 单空格）+ 去首尾空白。
// 关键：所有引擎必须共用同一套规则，否则跨引擎锚点匹配与缓存命中失效。

pub fn normalize_source(raw: &str) -> String {
    // 1. 去注释
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = if i + 1 < chars.len() {
                i + 2
            } else {
                chars.len()
            };
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }

    // 2. 折叠空白
    let mut folded = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !prev_space {
                folded.push(' ');
                prev_space = true;
            }
        } else {
            folded.push(c);
            prev_space = false;
        }
    }
    folded.trim().to_string()
}

/// FNV-1a 64 位常量（与 vertex/check.rs 对齐，跨进程/跨版本稳定）。
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 归一化文本哈希（FNV-1a 64 位，跨进程/跨 Rust 版本稳定，可用于持久化缓存键）。
pub fn hash_source(normalized: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for b in normalized.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_whitespace() {
        assert_eq!(normalize_source("x  ^   2"), "x ^ 2");
        assert_eq!(normalize_source("x\n^ \t\r\n2"), "x ^ 2");
        assert_eq!(normalize_source("   x^2   "), "x^2");
    }

    #[test]
    fn strip_comments() {
        assert_eq!(normalize_source("x^2 // comment"), "x^2");
        assert_eq!(normalize_source("x /* m */ ^2"), "x ^2");
        assert_eq!(normalize_source("a // line\n+ b"), "a + b");
        assert_eq!(normalize_source("x /* no end"), "x");
    }

    #[test]
    fn empty_input() {
        assert_eq!(normalize_source(""), "");
        assert_eq!(normalize_source("   \n\t  "), "");
    }

    #[test]
    fn hash_consistency() {
        assert_eq!(hash_source("a"), hash_source("a"));
        assert_ne!(hash_source("x^2"), hash_source("y^2"));
        let a = normalize_source("x ^  2");
        let b = normalize_source("x  ^ 2");
        assert_eq!(hash_source(&a), hash_source(&b));
    }

    /// FNV-1a 已知向量：空串与 "a" 的哈希值应与标准一致
    ///（与 vertex/check.rs 的 FNV-1a 实现交叉验证）。
    #[test]
    fn hash_fnv1a_known_vectors() {
        // FNV-1a offset basis
        assert_eq!(hash_source(""), FNV_OFFSET);
        // hash("") ^ 'a' * prime
        let h = FNV_OFFSET ^ (b'a' as u64);
        let h = h.wrapping_mul(FNV_PRIME);
        assert_eq!(hash_source("a"), h);
    }
}

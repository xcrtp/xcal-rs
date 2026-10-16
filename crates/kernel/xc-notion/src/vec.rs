// vec.rs
//
// 向量类型：直接再导出 glam 类型，零额外抽象。
// 理念层所有图元/路径/变换均使用这些类型。
//
// 整数向量 `I8Vec4` / `U8Vec4` 于 M1 从 `xcal-scene::types` 收回：此前 scene 各自
// 用 `export_vec!` 造本地 newtype（连同本地 `From<(i8,i8,i8,i8)>` 实现），是"每层各自
// 依赖 glam"的必然后果。收回后直接复用 glam 自带的 tuple 转换
// （`glam` 的 `i8/i8vec4.rs` 等已实现 `From<(i8, i8, i8, i8)>`），
// 无需再在理念层手写——本 crate 也无法为外部类型实现外部 trait（孤儿规则）。

pub use glam::{I8Vec4, U8Vec4, Vec2, Vec3, Vec4};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i8vec4_from_tuple() {
        let v: I8Vec4 = (1_i8, 2, 3, 4).into();
        assert_eq!(v.x, 1);
        assert_eq!(v.y, 2);
        assert_eq!(v.z, 3);
        assert_eq!(v.w, 4);
    }

    #[test]
    fn test_u8vec4_from_tuple() {
        let v: U8Vec4 = (1_u8, 2, 3, 4).into();
        assert_eq!(v.x, 1);
        assert_eq!(v.y, 2);
        assert_eq!(v.z, 3);
        assert_eq!(v.w, 4);
    }
}

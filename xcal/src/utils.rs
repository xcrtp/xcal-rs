

#[macro_export(local_inner_macros)]
macro_rules! auto_into {
    ([$($expr:expr),*]) => {
        [$($expr.into()),*]
    };
}

#[cfg(test)]
mod tests {
    use crate::types::Vec3;

    #[test]
    fn test_auto_into_basic() {
        // i32 -> f64 is a valid Into conversion
        let v: [f64; 3] = auto_into!([1_i32, 2, 3]);
        assert_eq!(v, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_auto_into_empty() {
        let v: [i32; 0] = auto_into!([]);
        assert!(v.is_empty());
    }

    #[test]
    fn test_auto_into_single() {
        let v: [f64; 1] = auto_into!([42_i32]);
        assert_eq!(v, [42.0]);
    }

    #[test]
    fn test_auto_into_preserves_order() {
        let v: [i32; 5] = auto_into!([5, 3, 1, 4, 2]);
        assert_eq!(v, [5, 3, 1, 4, 2]);
    }

    #[test]
    fn test_auto_into_to_vec() {
        let v: Vec<f64> = auto_into!([1_i32, 2, 3]).into();
        assert_eq!(v, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_auto_into_into_vec3() {
        let v: [Vec3; 2] = auto_into!([(1.0_f32, 2.0, 3.0), (4.0, 5.0, 6.0)]);
        assert_eq!(v[0].x, 1.0);
        assert_eq!(v[0].y, 2.0);
        assert_eq!(v[0].z, 3.0);
        assert_eq!(v[1].x, 4.0);
        assert_eq!(v[1].y, 5.0);
        assert_eq!(v[1].z, 6.0);
    }

    #[test]
    fn test_auto_into_str_to_string() {
        let v: [String; 2] = auto_into!(["hello", "world"]);
        assert_eq!(v, ["hello".to_string(), "world".to_string()]);
    }
}

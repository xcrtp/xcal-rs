

#[macro_export(local_inner_macros)]
macro_rules! auto_into {
    ([$($expr:expr),*]) => {
        [$($expr.into()),*]
    };
}
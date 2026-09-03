//! The property model: a serializable, UI-neutral description of every
//! parameter a node can expose.
//!
//! [`Property`] is the *schema + current value* (label, constraints, value);
//! [`PropertyValue`] is the bare, type-erased value carried alongside. They are
//! kept symmetric so that:
//!   * a UI layer can map each variant to a natural widget without knowing the
//!     concrete node type (no `downcast`);
//!   * a headless evaluator can read/write values without any UI dependency;
//!   * user-defined ([`crate::DynamicObject`]) fields reuse the exact same
//!     vocabulary as declarative (`NodeSpec`) fields.
//!
//! Design rules enforced here:
//!   * every variant is `Serialize + Deserialize + Clone + PartialEq`;
//!   * every variant has a matching [`PropertyValue`] branch and a
//!     range/type-validating [`Property::set_value`];
//!   * no `egui`, no `glam` — only `serde` + `thiserror` + `std`.

use serde::{Deserialize, Serialize};

use crate::dynamic::DynamicObject;
use crate::error::PropertyError;

/// Maximum number of components stored for a vector property (Vec2/3/4 share
/// one storage with a `size` discriminator).
pub const VEC_MAX_COMPONENTS: usize = 4;

/// Variant tag for string-like properties (drives widget choice in the UI layer
/// without inventing a separate sub-system).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringKind {
    /// Single-line text.
    #[default]
    Text,
    /// Multi-line text area.
    Multiline,
    /// Filesystem path.
    Path,
    /// Masked input.
    Password,
}

/// Reference to a function-producing output socket (the `Evalable` hook for the
/// future higher-order `Function` variant). Kept plain-data and serializable so
/// property graphs survive a round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionHandle {
    /// Owning node id (matches `xc_node::NodeId.0`).
    pub node: u64,
    /// Output socket index on that node.
    pub socket: u32,
    /// Human-readable label for menus/diagnostics.
    pub label: String,
}

// ---------------------------------------------------------------------------
// Per-variant config structs
// ---------------------------------------------------------------------------

macro_rules! scalar_config {
    ($name:ident, $ty:ty) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            pub label: String,
            pub value: $ty,
            pub min: $ty,
            pub max: $ty,
            pub step: Option<$ty>,
            pub unit: Option<String>,
        }
    };
}

scalar_config!(ScalarConfig, f32);
scalar_config!(DoubleConfig, f64);
scalar_config!(IntegerConfig, i64);

impl IntegerConfig {
    /// Whether integers are signed; reserved for future unsigned support.
    pub const SIGNED: bool = true;
}

/// Boolean toggle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoolConfig {
    pub label: String,
    pub value: bool,
}

/// Text property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StringConfig {
    pub label: String,
    pub value: String,
    /// Widget flavour. Renamed on the wire to avoid clashing with the enum's
    /// `kind` discriminant tag.
    #[serde(default, rename = "string_kind")]
    pub kind: StringKind,
}

/// Vec2/3/4 property sharing one constraint set across all components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VecNConfig {
    pub label: String,
    /// Active component count (2, 3 or 4). `components[size..]` ignored.
    pub size: u8,
    pub components: [f32; VEC_MAX_COMPONENTS],
    pub min: Option<[f32; VEC_MAX_COMPONENTS]>,
    pub max: Option<[f32; VEC_MAX_COMPONENTS]>,
    pub step: Option<f32>,
}

/// RGBA color property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorConfig {
    pub label: String,
    pub value: [u8; 4],
    pub with_alpha: bool,
}

/// Discrete choice property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumConfig {
    pub label: String,
    /// Currently selected option (must be one of `options`).
    pub value: String,
    pub options: Vec<String>,
}

/// Interval property `[lo, hi]` constrained to `[bound_min, bound_max]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeConfig {
    pub label: String,
    /// `[lo, hi]`, `lo <= hi` expected.
    pub value: [f64; 2],
    pub bound_min: f64,
    pub bound_max: f64,
}

/// Function-object reference property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionConfig {
    pub label: String,
    pub value: Option<FunctionHandle>,
}

/// User-defined arbitrary structure (see [`DynamicObject`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicConfig {
    pub label: String,
    pub value: DynamicObject,
}

/// Homogeneous list property. `element` describes the schema of every element;
/// `values` holds the current elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListConfig {
    pub label: String,
    pub element: Box<Property>,
    pub values: Vec<PropertyValue>,
    pub min_len: Option<usize>,
    pub max_len: Option<usize>,
}

// ---------------------------------------------------------------------------
// Property / PropertyValue
// ---------------------------------------------------------------------------

/// A node property: schema + current value. One variant per supported kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Property {
    Scalar(ScalarConfig),
    Double(DoubleConfig),
    Integer(IntegerConfig),
    Bool(BoolConfig),
    String(StringConfig),
    VecN(VecNConfig),
    Color(ColorConfig),
    Enum(EnumConfig),
    Range(RangeConfig),
    Function(FunctionConfig),
    Dynamic(DynamicConfig),
    List(ListConfig),
}

/// Bare value carried by a [`Property`]. Symmetric variant set, serializable.
///
/// Externally tagged (the default) because several variants wrap primitives
/// (`f32`, `bool`, `String`, …) which serde cannot represent under internal
/// tagging. This keeps `PropertyValue` self-describing on the wire:
/// `{"Scalar":3.3}`, `{"VecN":{"size":3,"components":[...]}}`, …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyValue {
    Scalar(f32),
    Double(f64),
    Integer(i64),
    Bool(bool),
    String(String),
    VecN {
        size: u8,
        components: [f32; VEC_MAX_COMPONENTS],
    },
    Color([u8; 4]),
    Enum(String),
    Range {
        lo: f64,
        hi: f64,
    },
    Function(Option<FunctionHandle>),
    Dynamic(DynamicObject),
    List(Vec<PropertyValue>),
}

impl Property {
    /// Human-readable label shown next to the widget.
    pub fn label(&self) -> &str {
        match self {
            Property::Scalar(c) => &c.label,
            Property::Double(c) => &c.label,
            Property::Integer(c) => &c.label,
            Property::Bool(c) => &c.label,
            Property::String(c) => &c.label,
            Property::VecN(c) => &c.label,
            Property::Color(c) => &c.label,
            Property::Enum(c) => &c.label,
            Property::Range(c) => &c.label,
            Property::Function(c) => &c.label,
            Property::Dynamic(c) => &c.label,
            Property::List(c) => &c.label,
        }
    }

    /// Stable kind name (matches the serde `kind` tag) for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Property::Scalar(_) => "scalar",
            Property::Double(_) => "double",
            Property::Integer(_) => "integer",
            Property::Bool(_) => "bool",
            Property::String(_) => "string",
            Property::VecN(_) => "vec_n",
            Property::Color(_) => "color",
            Property::Enum(_) => "enum",
            Property::Range(_) => "range",
            Property::Function(_) => "function",
            Property::Dynamic(_) => "dynamic",
            Property::List(_) => "list",
        }
    }

    /// Extract the current value as a bare [`PropertyValue`].
    pub fn value(&self) -> PropertyValue {
        match self {
            Property::Scalar(c) => PropertyValue::Scalar(c.value),
            Property::Double(c) => PropertyValue::Double(c.value),
            Property::Integer(c) => PropertyValue::Integer(c.value),
            Property::Bool(c) => PropertyValue::Bool(c.value),
            Property::String(c) => PropertyValue::String(c.value.clone()),
            Property::VecN(c) => PropertyValue::VecN {
                size: c.size,
                components: c.components,
            },
            Property::Color(c) => PropertyValue::Color(c.value),
            Property::Enum(c) => PropertyValue::Enum(c.value.clone()),
            Property::Range(c) => PropertyValue::Range {
                lo: c.value[0],
                hi: c.value[1],
            },
            Property::Function(c) => PropertyValue::Function(c.value.clone()),
            Property::Dynamic(c) => PropertyValue::Dynamic(c.value.clone()),
            Property::List(c) => PropertyValue::List(c.values.clone()),
        }
    }

    /// Validate `value` against this property's type + constraints and, if ok,
    /// store it. Returns [`PropertyError`] on mismatch — never panics.
    pub fn set_value(&mut self, value: PropertyValue) -> Result<(), PropertyError> {
        match (self, value) {
            (Property::Scalar(c), PropertyValue::Scalar(v)) => {
                check_finite_f32(v)?;
                clamp_into_range(&mut c.value, v, c.min, c.max, c.step, "scalar")
            }
            (Property::Double(c), PropertyValue::Double(v)) => {
                check_finite_f64(v)?;
                clamp_into_range(&mut c.value, v, c.min, c.max, c.step, "double")
            }
            (Property::Integer(c), PropertyValue::Integer(v)) => {
                clamp_into_range(&mut c.value, v, c.min, c.max, c.step, "integer")
            }
            (Property::Bool(c), PropertyValue::Bool(v)) => {
                c.value = v;
                Ok(())
            }
            (Property::String(c), PropertyValue::String(v)) => {
                c.value = v;
                Ok(())
            }
            (Property::VecN(c), PropertyValue::VecN { size, components }) => {
                if size != c.size {
                    return Err(PropertyError::TypeMismatch {
                        expected: format!("vec{}", c.size),
                        got: format!("vec{size}"),
                    });
                }
                if !(2..=VEC_MAX_COMPONENTS as u8).contains(&size) {
                    return Err(PropertyError::InvalidValue {
                        reason: format!("invalid vec size {size}"),
                    });
                }
                if let Some(min) = c.min {
                    for i in 0..size as usize {
                        if components[i] < min[i] {
                            return Err(PropertyError::OutOfRange {
                                which: format!("vec[{i}] = {}", components[i]),
                                min: min[i].to_string(),
                                max: c.max.map(|m| m[i].to_string()).unwrap_or_default(),
                            });
                        }
                    }
                }
                if let Some(max) = c.max {
                    for i in 0..size as usize {
                        if components[i] > max[i] {
                            return Err(PropertyError::OutOfRange {
                                which: format!("vec[{i}] = {}", components[i]),
                                min: c.min.map(|m| m[i].to_string()).unwrap_or_default(),
                                max: max[i].to_string(),
                            });
                        }
                    }
                }
                c.components = components;
                Ok(())
            }
            (Property::Color(c), PropertyValue::Color(v)) => {
                c.value = v;
                Ok(())
            }
            (Property::Enum(c), PropertyValue::Enum(v)) => {
                if !c.options.contains(&v) {
                    return Err(PropertyError::InvalidValue {
                        reason: format!("`{v}` is not a valid option"),
                    });
                }
                c.value = v;
                Ok(())
            }
            (Property::Range(c), PropertyValue::Range { lo, hi }) => {
                if lo > hi {
                    return Err(PropertyError::InvalidValue {
                        reason: "range lo > hi".into(),
                    });
                }
                if lo < c.bound_min || hi > c.bound_max {
                    return Err(PropertyError::OutOfRange {
                        which: "range".into(),
                        min: c.bound_min.to_string(),
                        max: c.bound_max.to_string(),
                    });
                }
                c.value = [lo, hi];
                Ok(())
            }
            (Property::Function(c), PropertyValue::Function(v)) => {
                c.value = v;
                Ok(())
            }
            (Property::Dynamic(c), PropertyValue::Dynamic(v)) => {
                c.value = v;
                Ok(())
            }
            (Property::List(c), PropertyValue::List(v)) => {
                if let Some(min) = c.min_len
                    && v.len() < min
                {
                    return Err(PropertyError::OutOfRange {
                        which: "list length".into(),
                        min: min.to_string(),
                        max: c.max_len.map(|m| m.to_string()).unwrap_or_default(),
                    });
                }
                if let Some(max) = c.max_len
                    && v.len() > max
                {
                    return Err(PropertyError::OutOfRange {
                        which: "list length".into(),
                        min: c.min_len.map(|m| m.to_string()).unwrap_or_default(),
                        max: max.to_string(),
                    });
                }
                // Validate every element against the element template.
                for el in &v {
                    let mut probe = c.element.as_ref().clone();
                    probe.set_value(el.clone())?;
                }
                c.values = v;
                Ok(())
            }
            (prop, val) => Err(PropertyError::TypeMismatch {
                expected: prop.kind_name().into(),
                got: val.kind_name().into(),
            }),
        }
    }
}

impl PropertyValue {
    /// Stable kind name matching the serde `kind` tag.
    pub fn kind_name(&self) -> &'static str {
        match self {
            PropertyValue::Scalar(_) => "scalar",
            PropertyValue::Double(_) => "double",
            PropertyValue::Integer(_) => "integer",
            PropertyValue::Bool(_) => "bool",
            PropertyValue::String(_) => "string",
            PropertyValue::VecN { .. } => "vec_n",
            PropertyValue::Color(_) => "color",
            PropertyValue::Enum(_) => "enum",
            PropertyValue::Range { .. } => "range",
            PropertyValue::Function(_) => "function",
            PropertyValue::Dynamic(_) => "dynamic",
            PropertyValue::List(_) => "list",
        }
    }
}

/// Clamp `v` into `[min, max]` and store it in `target`. Step snapping is left
/// to the UI layer (a slider's own step); `set_value` only enforces the hard
/// bounds so programmatic writes stay exact. NaN is rejected for floats.
fn clamp_into_range<T: Copy + PartialOrd>(
    target: &mut T,
    v: T,
    min: T,
    max: T,
    _step: Option<T>,
    _kind: &'static str,
) -> Result<(), PropertyError> {
    let clamped = if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    };
    *target = clamped;
    Ok(())
}

/// Reject NaN for float property writes (called from the float branches).
fn check_finite_f32(v: f32) -> Result<(), PropertyError> {
    if v.is_nan() {
        Err(PropertyError::InvalidValue {
            reason: "NaN not allowed".into(),
        })
    } else {
        Ok(())
    }
}

fn check_finite_f64(v: f64) -> Result<(), PropertyError> {
    if v.is_nan() {
        Err(PropertyError::InvalidValue {
            reason: "NaN not allowed".into(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_round_trip() {
        let mut p = Property::Scalar(ScalarConfig {
            label: "v".into(),
            value: 0.0,
            min: 0.0,
            max: 10.0,
            step: Some(0.5),
            unit: None,
        });
        assert!(p.set_value(PropertyValue::Scalar(3.3)).is_ok());
        assert_eq!(p.value(), PropertyValue::Scalar(3.3)); // clamped, not snapped
        // out-of-range clamps to the bound
        assert!(p.set_value(PropertyValue::Scalar(99.0)).is_ok());
        assert_eq!(p.value(), PropertyValue::Scalar(10.0));
        let err = p.set_value(PropertyValue::Bool(true)).unwrap_err();
        assert!(matches!(err, PropertyError::TypeMismatch { .. }));
    }

    #[test]
    fn serde_round_trip_all_variants() {
        let props = vec![
            Property::Scalar(ScalarConfig {
                label: "s".into(),
                value: 1.0,
                min: 0.0,
                max: 2.0,
                step: None,
                unit: None,
            }),
            Property::Double(DoubleConfig {
                label: "d".into(),
                value: 1.5,
                min: 0.0,
                max: 2.0,
                step: None,
                unit: None,
            }),
            Property::Integer(IntegerConfig {
                label: "i".into(),
                value: 3,
                min: 0,
                max: 10,
                step: None,
                unit: None,
            }),
            Property::Bool(BoolConfig {
                label: "b".into(),
                value: true,
            }),
            Property::String(StringConfig {
                label: "str".into(),
                value: "hi".into(),
                kind: StringKind::Multiline,
            }),
            Property::VecN(VecNConfig {
                label: "v".into(),
                size: 3,
                components: [1.0, 2.0, 3.0, 0.0],
                min: None,
                max: None,
                step: None,
            }),
            Property::Color(ColorConfig {
                label: "c".into(),
                value: [1, 2, 3, 4],
                with_alpha: true,
            }),
            Property::Enum(EnumConfig {
                label: "e".into(),
                value: "x".into(),
                options: vec!["x".into(), "y".into()],
            }),
            Property::Range(RangeConfig {
                label: "r".into(),
                value: [0.0, 1.0],
                bound_min: 0.0,
                bound_max: 10.0,
            }),
            Property::Function(FunctionConfig {
                label: "f".into(),
                value: None,
            }),
            Property::Dynamic(DynamicConfig {
                label: "dyn".into(),
                value: crate::dynamic::DynamicObject::new(),
            }),
            Property::List(ListConfig {
                label: "l".into(),
                element: Box::new(Property::Scalar(ScalarConfig {
                    label: "el".into(),
                    value: 0.0,
                    min: 0.0,
                    max: 1.0,
                    step: None,
                    unit: None,
                })),
                values: vec![PropertyValue::Scalar(0.25)],
                min_len: None,
                max_len: None,
            }),
        ];
        let json = serde_json::to_string(&props).unwrap();
        let back: Vec<Property> = serde_json::from_str(&json).unwrap();
        assert_eq!(props, back);
    }

    #[test]
    fn enum_rejects_unknown_option() {
        let mut p = Property::Enum(EnumConfig {
            label: "e".into(),
            value: "x".into(),
            options: vec!["x".into(), "y".into()],
        });
        assert!(p.set_value(PropertyValue::Enum("z".into())).is_err());
        assert!(p.set_value(PropertyValue::Enum("y".into())).is_ok());
    }

    #[test]
    fn range_rejects_lo_gt_hi() {
        let mut p = Property::Range(RangeConfig {
            label: "r".into(),
            value: [0.0, 1.0],
            bound_min: 0.0,
            bound_max: 10.0,
        });
        assert!(
            p.set_value(PropertyValue::Range { lo: 5.0, hi: 2.0 })
                .is_err()
        );
    }
}

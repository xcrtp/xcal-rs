//! [`DynamicObject`] — a runtime-extensible, ordered map of named properties.
//!
//! This is the "user can add fields without recompiling" half of the property
//! model. It reuses [`crate::Property`] verbatim, so a dynamic field and a
//! declarative `NodeSpec` field are the exact same kind of value — they can
//! coexist in one graph and be consumed by one evaluator.
//!
//! Constraints (per the plan §5):
//!   * field name → property mapping, supports runtime add/remove;
//!   * type-safe assignment: wrong kind returns [`PropertyError`], no panic;
//!   * constraints live on each property (no separate constraint system);
//!   * serializable as a whole;
//!   * bidirectional with [`crate::NodeSpec::properties`].

use serde::{Deserialize, Serialize};

use crate::error::PropertyError;
use crate::property::{Property, PropertyValue};

/// An ordered, named collection of properties a user can extend at runtime.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DynamicObject {
    fields: Vec<DynamicField>,
}

/// One entry in a [`DynamicObject`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicField {
    pub name: String,
    pub property: Property,
    /// Whether the field must be present / non-default for the object to be
    /// considered valid (enforced by [`DynamicObject::validate`]).
    #[serde(default)]
    pub required: bool,
}

impl DynamicObject {
    /// Create an empty object.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the object has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// All fields, in insertion order.
    pub fn fields(&self) -> &[DynamicField] {
        &self.fields
    }

    /// Iterate over `(name, property)` pairs (owned clones) for compatibility
    /// with [`crate::NodeSpec::properties`].
    pub fn properties(&self) -> Vec<(String, Property)> {
        self.fields
            .iter()
            .map(|f| (f.name.clone(), f.property.clone()))
            .collect()
    }

    /// Look up a field by name.
    pub fn get(&self, name: &str) -> Option<&Property> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| &f.property)
    }

    /// Look up a field mutably by name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.fields
            .iter_mut()
            .find(|f| f.name == name)
            .map(|f| &mut f.property)
    }

    /// Add a new field. Returns [`PropertyError::DuplicateField`] if the name is
    /// already in use.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        property: Property,
    ) -> Result<(), PropertyError> {
        let name = name.into();
        if self.fields.iter().any(|f| f.name == name) {
            return Err(PropertyError::DuplicateField(name));
        }
        self.fields.push(DynamicField {
            name,
            property,
            required: false,
        });
        Ok(())
    }

    /// Add a new field, marking it required.
    pub fn add_required(
        &mut self,
        name: impl Into<String>,
        property: Property,
    ) -> Result<(), PropertyError> {
        let name = name.into();
        if self.fields.iter().any(|f| f.name == name) {
            return Err(PropertyError::DuplicateField(name));
        }
        self.fields.push(DynamicField {
            name,
            property,
            required: true,
        });
        Ok(())
    }

    /// Remove a field by name, returning the removed property.
    pub fn remove(&mut self, name: &str) -> Option<Property> {
        let idx = self.fields.iter().position(|f| f.name == name)?;
        Some(self.fields.remove(idx).property)
    }

    /// Rename a field.
    pub fn rename(&mut self, from: &str, to: impl Into<String>) -> Result<(), PropertyError> {
        let to = to.into();
        if self.fields.iter().any(|f| f.name == to) {
            return Err(PropertyError::DuplicateField(to));
        }
        let f = self
            .fields
            .iter_mut()
            .find(|f| f.name == from)
            .ok_or_else(|| PropertyError::UnknownProperty(from.into()))?;
        f.name = to;
        Ok(())
    }

    /// Set the value of a field by name (type-checked via the property).
    pub fn set_value(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        let prop = self
            .get_mut(name)
            .ok_or_else(|| PropertyError::UnknownProperty(name.into()))?;
        prop.set_value(value)
    }

    /// Read the current value of a field by name.
    pub fn value(&self, name: &str) -> Option<PropertyValue> {
        self.get(name).map(Property::value)
    }

    /// Validate all required fields: a required field's value must differ from
    /// its "default" sentinel. Here we treat the property's current value as
    /// valid by default; `required` is a hint for UI/serialization. Returns
    /// `Ok(())` if all required fields are present (they always are, since they
    /// live in the object) — this hook exists so future validators can reject
    /// sentinel defaults without changing the API.
    pub fn validate(&self) -> Result<(), PropertyError> {
        for f in &self.fields {
            if f.required && matches!(&f.property, Property::Function(c) if c.value.is_none()) {
                return Err(PropertyError::MissingField(f.name.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::{BoolConfig, ScalarConfig};

    fn scalar(name: &str, v: f32) -> Property {
        Property::Scalar(ScalarConfig {
            label: name.into(),
            value: v,
            min: 0.0,
            max: 10.0,
            step: None,
            unit: None,
        })
    }

    #[test]
    fn add_remove_get() {
        let mut o = DynamicObject::new();
        o.add("a", scalar("a", 1.0)).unwrap();
        assert!(o.add("a", scalar("a", 2.0)).is_err());
        o.add(
            "b",
            Property::Bool(BoolConfig {
                label: "b".into(),
                value: false,
            }),
        )
        .unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o.value("a"), Some(PropertyValue::Scalar(1.0)));
        assert!(o.remove("a").is_some());
        assert_eq!(o.len(), 1);
        assert!(o.get("a").is_none());
    }

    #[test]
    fn set_value_type_checked() {
        let mut o = DynamicObject::new();
        o.add("a", scalar("a", 0.0)).unwrap();
        assert!(o.set_value("a", PropertyValue::Bool(true)).is_err());
        assert!(o.set_value("a", PropertyValue::Scalar(5.0)).is_ok());
        assert_eq!(o.value("a"), Some(PropertyValue::Scalar(5.0)));
        assert!(o.set_value("missing", PropertyValue::Bool(true)).is_err());
    }

    #[test]
    fn serde_round_trip() {
        let mut o = DynamicObject::new();
        o.add("a", scalar("a", 1.0)).unwrap();
        let json = serde_json::to_string(&o).unwrap();
        let back: DynamicObject = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }
}

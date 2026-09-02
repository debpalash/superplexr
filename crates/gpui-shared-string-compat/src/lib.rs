//! Independently implemented, permissively licensed shared-string seam for GPUI.
//!
//! GPUI only requires an immutable string with cheap clones, a `const` static
//! constructor, standard string conversions, Serde, and JSON-schema support.

use std::{
    borrow::{Borrow, Cow},
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
    sync::Arc,
};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An immutable string that either borrows static storage or shares heap storage.
#[derive(Clone)]
pub struct SharedString(Repr);

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Repr {
    Static(&'static str),
    Shared(Arc<str>),
}

impl SharedString {
    /// Creates a shared copy of an arbitrary string-like value.
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(Repr::Shared(Arc::from(value.as_ref())))
    }

    /// Creates a shared string without allocation from static storage.
    pub const fn new_static(value: &'static str) -> Self {
        Self(Repr::Static(value))
    }

    /// Returns this value as a string slice.
    pub fn as_str(&self) -> &str {
        self
    }
}

impl PartialEq for SharedString {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for SharedString {}

impl PartialOrd for SharedString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SharedString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl Hash for SharedString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl Deref for SharedString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Repr::Static(value) => value,
            Repr::Shared(value) => value,
        }
    }
}

impl Default for SharedString {
    fn default() -> Self {
        Self::new_static("")
    }
}

impl AsRef<str> for SharedString {
    fn as_ref(&self) -> &str {
        self
    }
}

impl Borrow<str> for SharedString {
    fn borrow(&self) -> &str {
        self
    }
}

impl fmt::Debug for SharedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

impl fmt::Display for SharedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self)
    }
}

impl PartialEq<String> for SharedString {
    fn eq(&self, other: &String) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<SharedString> for String {
    fn eq(&self, other: &SharedString) -> bool {
        self == other.as_ref()
    }
}

impl PartialEq<str> for SharedString {
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<&str> for SharedString {
    fn eq(&self, other: &&str) -> bool {
        self.as_ref() == *other
    }
}

impl From<&SharedString> for SharedString {
    fn from(value: &SharedString) -> Self {
        value.clone()
    }
}

impl From<&str> for SharedString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<char> for SharedString {
    fn from(value: char) -> Self {
        Self::from(value.to_string())
    }
}

impl From<&mut str> for SharedString {
    fn from(value: &mut str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for SharedString {
    fn from(value: &String) -> Self {
        Self::new(value)
    }
}

impl From<String> for SharedString {
    fn from(value: String) -> Self {
        Self(Repr::Shared(Arc::from(value)))
    }
}

impl From<Box<str>> for SharedString {
    fn from(value: Box<str>) -> Self {
        Self(Repr::Shared(Arc::from(value)))
    }
}

impl From<Arc<str>> for SharedString {
    fn from(value: Arc<str>) -> Self {
        Self(Repr::Shared(value))
    }
}

impl From<&Arc<str>> for SharedString {
    fn from(value: &Arc<str>) -> Self {
        Self(Repr::Shared(Arc::clone(value)))
    }
}

impl From<Cow<'_, str>> for SharedString {
    fn from(value: Cow<'_, str>) -> Self {
        match value {
            Cow::Borrowed(value) => Self::new(value),
            Cow::Owned(value) => Self::from(value),
        }
    }
}

impl From<SharedString> for Arc<str> {
    fn from(value: SharedString) -> Self {
        match value.0 {
            Repr::Static(value) => Arc::from(value),
            Repr::Shared(value) => value,
        }
    }
}

impl From<SharedString> for String {
    fn from(value: SharedString) -> Self {
        value.as_ref().to_owned()
    }
}

impl Serialize for SharedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self)
    }
}

impl<'de> Deserialize<'de> for SharedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

impl JsonSchema for SharedString {
    fn inline_schema() -> bool {
        <String as JsonSchema>::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("SharedString")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::SharedString;

    #[test]
    fn static_and_owned_values_compare_and_hash_equally() {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let static_value = SharedString::new_static("termi9ne");
        let owned_value = SharedString::from(String::from("termi9ne"));
        let mut static_hash = DefaultHasher::new();
        let mut owned_hash = DefaultHasher::new();
        static_value.hash(&mut static_hash);
        owned_value.hash(&mut owned_hash);

        assert_eq!(static_value, owned_value);
        assert_eq!(static_hash.finish(), owned_hash.finish());
    }
}

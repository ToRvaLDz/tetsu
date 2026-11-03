//! Serde helper for fields that distinguish "absent" from "null".

use serde::{Deserialize, Deserializer};

/// Reads a present value, `null` included, as `Some`. Pair it with
/// `#[serde(default)]` so an absent field stays `None`.
pub(crate) fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

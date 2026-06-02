use std::fmt;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Supported guest language runtimes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeLanguage {
    #[default]
    Python,
    JavaScript,
}

impl RuntimeLanguage {
    /// Returns a lowercase string identifier for the language.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
        }
    }
}

impl fmt::Display for RuntimeLanguage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RuntimeLanguage {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "python" => Ok(Self::Python),
            "javascript" | "js" => Ok(Self::JavaScript),
            _ => Err(()),
        }
    }
}

impl Serialize for RuntimeLanguage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RuntimeLanguage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(|_| {
            de::Error::custom(format!(
                "unsupported runtime language '{}'; expected 'python' or 'javascript'",
                value
            ))
        })
    }
}

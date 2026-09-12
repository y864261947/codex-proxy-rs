use std::num::NonZeroU16;

use crate::validation::IdentifierError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePreference {
    priority: NonZeroU16,
    weight: NonZeroU16,
}

impl SourcePreference {
    pub fn new(priority: u16, weight: u16) -> Result<Self, IdentifierError> {
        Ok(Self {
            priority: NonZeroU16::new(priority).ok_or(IdentifierError::InvalidFormat)?,
            weight: NonZeroU16::new(weight).ok_or(IdentifierError::InvalidFormat)?,
        })
    }

    #[must_use]
    pub const fn priority(self) -> u16 {
        self.priority.get()
    }

    #[must_use]
    pub const fn weight(self) -> u16 {
        self.weight.get()
    }
}

impl Default for SourcePreference {
    fn default() -> Self {
        Self {
            priority: NonZeroU16::MIN,
            weight: NonZeroU16::MIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePreferenceOverride {
    priority: Option<NonZeroU16>,
    weight: Option<NonZeroU16>,
}

impl SourcePreferenceOverride {
    pub fn new(priority: Option<u16>, weight: Option<u16>) -> Result<Self, IdentifierError> {
        if priority.is_none() && weight.is_none() {
            return Err(IdentifierError::InvalidFormat);
        }
        Ok(Self {
            priority: priority
                .map(|value| NonZeroU16::new(value).ok_or(IdentifierError::InvalidFormat))
                .transpose()?,
            weight: weight
                .map(|value| NonZeroU16::new(value).ok_or(IdentifierError::InvalidFormat))
                .transpose()?,
        })
    }

    #[must_use]
    pub fn priority(self) -> Option<u16> {
        self.priority.map(NonZeroU16::get)
    }

    #[must_use]
    pub fn weight(self) -> Option<u16> {
        self.weight.map(NonZeroU16::get)
    }

    #[must_use]
    pub fn resolve(self, defaults: SourcePreference) -> SourcePreference {
        SourcePreference {
            priority: self.priority.unwrap_or(defaults.priority),
            weight: self.weight.unwrap_or(defaults.weight),
        }
    }
}

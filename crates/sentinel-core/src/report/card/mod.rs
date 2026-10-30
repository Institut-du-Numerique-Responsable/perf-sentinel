//! Share cards: the signed payloads a card carries, its keys, and their
//! rendering. Contract C2/C3 in `docs/SHARE-CARD.md`.

#[cfg_attr(not(test), expect(dead_code, reason = "called by render_card, Task 8"))]
mod copy;
pub mod keys;
#[cfg_attr(not(test), expect(dead_code, reason = "called by render_card, Task 8"))]
mod layout;
pub mod payload;
pub mod pdf;
pub mod raster;
#[cfg(test)]
mod test_fixtures;

/// The family a card failure belongs to, which fixes the exit code and
/// the `result` key of the error line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardResult {
    InputError,
    Refused,
    Unrenderable,
}

impl CardResult {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputError => "input_error",
            Self::Refused => "refused",
            Self::Unrenderable => "unrenderable",
        }
    }

    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::InputError => 3,
            Self::Refused => 1,
            Self::Unrenderable => 5,
        }
    }
}

/// A card failure. `reason` comes from the closed list of C2; `field` is
/// set only on `unsupported_characters`, to the `card.json` path of the
/// first text the fonts cannot draw.
#[derive(Debug, thiserror::Error)]
#[error("{reason}: {detail}")]
pub struct CardError {
    pub result: CardResult,
    pub reason: &'static str,
    pub detail: String,
    pub field: Option<&'static str>,
}

impl CardError {
    pub(crate) fn new(result: CardResult, reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            result,
            reason,
            detail: detail.into(),
            field: None,
        }
    }
}

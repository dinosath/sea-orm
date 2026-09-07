//! Errors produced by the type-safe Formula subsystem.
//!
//! All type-checking and relationship-resolution failures surface as a
//! [`FormulaError`] carrying a human readable message that identifies the
//! offending operand as well as the expected and actual types.

use std::fmt;

use crate::formula::FormulaType;

/// Result alias used throughout the formula subsystem.
pub type Result<T> = std::result::Result<T, FormulaError>;

/// An error raised while building, validating or compiling a [`crate::formula`]
/// expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaError {
    /// Human readable description of the failure.
    pub message: String,
}

impl FormulaError {
    /// Create a generic error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Build a type mismatch error that names the offending operand, the
    /// expected type and the actual type of that operand.
    pub fn type_mismatch(
        operation: &str,
        operand: &str,
        expected: FormulaType,
        actual: FormulaType,
    ) -> Self {
        Self::new(format!(
            "type mismatch in formula {operation}: operand `{operand}` requires \
             `{expected}`, but was given a value of type `{actual}`"
        ))
    }

    /// Error produced when two operands of a binary operator are incompatible.
    pub fn incompatible(operation: &str, left: FormulaType, right: FormulaType) -> Self {
        Self::new(format!(
            "cannot apply formula operator `{operation}` to operands of type \
             `{left}` and `{right}`"
        ))
    }
}

impl fmt::Display for FormulaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FormulaError {}

impl From<FormulaError> for crate::DbErr {
    fn from(err: FormulaError) -> Self {
        crate::DbErr::Custom(err.message)
    }
}

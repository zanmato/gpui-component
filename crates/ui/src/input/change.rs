use std::fmt::Debug;

use crate::{history::HistoryItem, input::Selection};

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum OperationType {
    Insert,
    Delete,
}

/// A single change in the text.
/// For multi-cursor operations, each cursor gets its own Change with the same version
/// and operation_id. The operation_id allows grouping changes by operation when
/// multiple operations share the same version (time-based grouping).
#[derive(Debug, PartialEq, Clone)]
pub struct Change {
    pub(crate) old_range: Selection,
    pub(crate) old_text: String,
    pub(crate) new_range: Selection,
    pub(crate) new_text: String,
    version: usize,
    pub(crate) operation_id: usize,
}

impl Change {
    pub fn new(
        old_range: impl Into<Selection>,
        old_text: &str,
        new_range: impl Into<Selection>,
        new_text: &str,
        operation_id: usize,
    ) -> Self {
        Self {
            old_range: old_range.into(),
            old_text: old_text.to_string(),
            new_range: new_range.into(),
            new_text: new_text.to_string(),
            version: 0,
            operation_id,
        }
    }
}

impl HistoryItem for Change {
    fn version(&self) -> usize {
        self.version
    }

    fn set_version(&mut self, version: usize) {
        self.version = version;
    }
}

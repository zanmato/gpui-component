use std::ops::{Range, RangeBounds};

use super::selection::CursorId;

/// A text selection range with cursor state.
/// Each selection can represent either a collapsed cursor (start == end)
/// or a selected text range.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub id: CursorId,   // Unique identifier
    pub start: usize,   // UTF-8 byte offset (always <= end)
    pub end: usize,     // UTF-8 byte offset (always >= start)
    pub reversed: bool, // True if selection was made backwards
    /// Remembered column position for vertical movement (None = not set, Some = column in characters)
    pub column_anchor: Option<usize>,
}

impl Selection {
    pub fn new(id: CursorId, start: usize, end: usize) -> Self {
        Self {
            id,
            start,
            end,
            reversed: false,
            column_anchor: None,
        }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Clears the selection, setting start and end to 0.
    pub fn clear(&mut self) {
        self.start = 0;
        self.end = 0;
    }

    /// Checks if the given offset is within the selection range.
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Returns the position where the cursor is visually located
    pub fn cursor_offset(&self) -> usize {
        if self.reversed { self.start } else { self.end }
    }

    /// Place this selection as a collapsed cursor at the given offset
    pub fn place_at(&mut self, offset: usize, column_anchor: Option<usize>) {
        self.start = offset;
        self.end = offset;
        self.column_anchor = column_anchor;
    }

    /// Returns true if this is a collapsed cursor (no text selected)
    pub fn is_collapsed(&self) -> bool {
        self.start == self.end
    }
}

impl From<Range<usize>> for Selection {
    fn from(value: Range<usize>) -> Self {
        Self::new(CursorId::default(), value.start, value.end)
    }
}
impl From<Selection> for Range<usize> {
    fn from(value: Selection) -> Self {
        value.start..value.end
    }
}
impl RangeBounds<usize> for Selection {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Included(&self.start)
    }

    fn end_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Excluded(&self.end)
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::new(CursorId::default(), 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::input::Position;

    #[test]
    fn test_line_column_from_to() {
        assert_eq!(
            Position::new(1, 2),
            Position {
                line: 1,
                character: 2
            }
        );
    }
}

/// Manages multiple cursors and selections in the text.
/// Always maintains at least one selection.
/// The first cursor (index 0) is always the active cursor that receives keyboard input.
pub struct Selections {
    selections: Vec<Selection>,
    next_id: usize,
}

impl Selections {
    pub fn new() -> Self {
        let first_id = CursorId::new(0);
        Self {
            selections: vec![Selection::new(first_id, 0, 0)],
            next_id: 1,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.selections.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Selection> {
        self.selections.iter_mut()
    }

    pub fn active(&self) -> &Selection {
        self.selections
            .first()
            .expect("Selections always has at least one selection")
    }

    pub fn add(&mut self, selection: Selection) {
        self.selections.push(selection);
    }

    pub fn replace_all(&mut self, selections: Vec<Selection>) {
        if !selections.is_empty() {
            self.selections = selections;
        }
    }

    pub fn remove_all_but_active(&mut self) {
        // Always keep only the first cursor (index 0) as active
        self.selections.truncate(1);
    }

    pub fn generate_id(&mut self) -> CursorId {
        let id = CursorId::new(self.next_id);
        self.next_id += 1;
        id
    }

    /// Merge overlapping or adjacent selections to avoid redundancy
    pub fn merge_overlapping(&mut self) {
        // Sort by start position
        self.selections.sort_by_key(|s| s.start);

        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for selection in &self.selections {
            if let Some(last) = merged.last_mut() {
                if selection.start <= last.end {
                    // Overlapping or adjacent, extend the last one
                    last.end = last.end.max(selection.end);
                    // If the selection we're merging has reversed=true, keep it that way
                    // This handles the case where the cursor is at the left end
                    if selection.reversed {
                        last.reversed = true;
                    }
                } else {
                    merged.push(selection.clone());
                }
            } else {
                merged.push(selection.clone());
            }
        }
        self.selections = merged;
    }

    /// Returns an iterator over all selection ranges.
    pub fn all_ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.selections.iter().map(|sel| sel.start..sel.end)
    }

    /// Returns the number of selections
    pub fn len(&self) -> usize {
        self.selections.len()
    }

    /// Returns true if there is only one selection
    pub fn is_single(&self) -> bool {
        self.selections.len() == 1
    }
}

impl Default for Selections {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests_selections {
    use super::*;

    #[test]
    fn test_selections_merge_overlapping() {
        let mut selections = Selections::new();

        // Replace with overlapping selections
        let id1 = selections.generate_id();
        let id2 = selections.generate_id();
        let id3 = selections.generate_id();

        selections.replace_all(vec![
            Selection::new(id1, 0, 10),
            Selection::new(id2, 5, 15),  // Overlaps with first
            Selection::new(id3, 20, 30), // Non-overlapping
        ]);

        selections.merge_overlapping();

        // After merge, we should have 2 selections: (0, 15) and (20, 30)
        assert_eq!(selections.len(), 2);

        let merged: Vec<_> = selections.iter().map(|s| (s.start, s.end)).collect();
        // First should cover 0-15 (merged 0-10 and 5-15)
        assert!(merged.iter().any(|(start, end)| *start == 0 && *end == 15));
        // Second should be 20-30
        assert!(merged.iter().any(|(start, end)| *start == 20 && *end == 30));
    }
}

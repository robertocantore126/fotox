//! Undo/redo by snapshots.
//!
//! Because [`Document`] clones are cheap (shared `Arc<Layer>`s and tile
//! handles), each history state is simply the document *before* a command.
//! Memory cost of a step = the layers that command touched, and only their
//! changed tiles stay alive. Tiles are freed when the step falls off the end.

use crate::command::{Command, CommandContext, CommandEffect, CommandError};
use crate::document::Document;

pub struct HistoryEntry {
	pub label: String,
	/// The command that produced the *next* state (kept for macro recording).
	pub command: Command,
	/// The document before `command` was applied.
	pub before: Document,
}

pub struct History {
	undo: Vec<HistoryEntry>,
	redo: Vec<HistoryEntry>,
	/// Photoshop default is 50 states.
	pub limit: usize,
}

impl Default for History {
	fn default() -> Self {
		Self {
			undo: Vec::new(),
			redo: Vec::new(),
			limit: 50,
		}
	}
}

impl History {
	/// Apply a command to `doc` and record it. Selection-only commands are
	/// applied without creating an undo step.
	pub fn execute(&mut self, doc: &mut Document, command: Command, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
		let before = doc.clone();
		let effect = command.apply(doc, ctx)?;
		if !effect.selection_only {
			self.redo.clear();
			self.undo.push(HistoryEntry {
				label: effect.label.clone(),
				command,
				before,
			});
			if self.undo.len() > self.limit {
				self.undo.remove(0);
			}
		}
		Ok(effect)
	}

	/// Record a command whose result was computed elsewhere (a filter job, a
	/// live brush stroke — docs/tasks/HOWTO.md R1b): `before` is the document
	/// as it was, the caller has already put the new state in place. Same
	/// limit and redo rules as [`execute`](Self::execute).
	pub fn record(&mut self, before: Document, command: Command, label: String) {
		self.redo.clear();
		self.undo.push(HistoryEntry { label, command, before });
		if self.undo.len() > self.limit {
			self.undo.remove(0);
		}
	}

	/// Returns false if there is nothing to undo.
	pub fn undo(&mut self, doc: &mut Document) -> bool {
		let Some(mut entry) = self.undo.pop() else { return false };
		std::mem::swap(doc, &mut entry.before);
		// `entry.before` now holds the state to redo into.
		self.redo.push(entry);
		true
	}

	pub fn redo(&mut self, doc: &mut Document) -> bool {
		let Some(mut entry) = self.redo.pop() else { return false };
		std::mem::swap(doc, &mut entry.before);
		self.undo.push(entry);
		true
	}

	pub fn labels(&self) -> impl Iterator<Item = &str> {
		self.undo.iter().map(|e| e.label.as_str())
	}

	/// Labels of the undone steps, in the order they would be redone (the
	/// History panel shows them greyed out after the current state).
	pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
		self.redo.iter().rev().map(|e| e.label.as_str())
	}

	/// The document of History panel row `row` (M8-T07, D-067): row 0 is the
	/// oldest state kept, row `labels().count()` the current one (`current`),
	/// the rows after it the undone states.
	pub fn state<'a>(&'a self, row: usize, current: &'a Document) -> Option<&'a Document> {
		let now = self.undo.len();
		if row < now {
			return Some(&self.undo[row].before);
		}
		if row == now {
			return Some(current);
		}
		let k = row - now;
		self.redo.len().checked_sub(k).map(|i| &self.redo[i].before)
	}

	pub fn can_undo(&self) -> bool {
		!self.undo.is_empty()
	}

	pub fn can_redo(&self) -> bool {
		!self.redo.is_empty()
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use fx_tiles::{TileStore, TileStoreConfig};

	use super::*;
	use crate::color::{BitDepth, ColorProfile, DocumentColor};
	use crate::command::{LayerPropsPatch, LayerRef};
	use crate::layer::{Layer, LayerKind};

	#[test]
	fn undo_redo_roundtrip() {
		let tiles = TileStore::new(TileStoreConfig::for_tests(std::env::temp_dir())).unwrap();
		let mut ctx = CommandContext { tiles: &tiles, ops: None };
		let mut doc = Document::new(
			10,
			10,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let id = doc.allocate_layer_id();
		doc.layers.push(Arc::new(Layer::new(id, "L", LayerKind::SolidFill { rgba: [0; 4] })));
		let mut history = History::default();

		let set = |o: f32| Command::SetLayerProps {
			layer: LayerRef::Id(id),
			props: LayerPropsPatch {
				opacity: Some(o),
				..Default::default()
			},
		};
		history.execute(&mut doc, set(0.25), &mut ctx).unwrap();
		history.execute(&mut doc, set(0.75), &mut ctx).unwrap();
		assert_eq!(doc.layer(id).unwrap().opacity, 0.75);
		assert!(history.undo(&mut doc));
		assert_eq!(doc.layer(id).unwrap().opacity, 0.25);
		assert!(history.undo(&mut doc));
		assert_eq!(doc.layer(id).unwrap().opacity, 1.0);
		assert!(!history.undo(&mut doc));
		assert!(history.redo(&mut doc));
		assert_eq!(doc.layer(id).unwrap().opacity, 0.25);
	}
}

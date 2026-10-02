//! Undo/redo by snapshots.
//!
//! Because [`Document`] clones are cheap (shared `Arc<Layer>`s and tile
//! handles), each history state is simply the document *before* a command.
//! Memory cost of a step = the layers that command touched, and only their
//! changed tiles stay alive. Tiles are freed when the step falls off the end.

use crate::command::{Command, CommandContext, CommandEffect, CommandError};
use crate::document::Document;

pub struct HistoryEntry {
	pub before_id: usize,
	pub after_id: usize,
	pub content_changed: bool,
	pub label: String,
	/// The command that produced the *next* state (kept for macro recording).
	pub command: Command,
	/// The document before `command` was applied.
	pub before: Document,
}

pub struct History {
	current_id: usize,
	next_id: usize,
	undo: Vec<HistoryEntry>,
	redo: Vec<HistoryEntry>,
	/// Photoshop default is 50 states.
	pub limit: usize,
}

impl Default for History {
	fn default() -> Self {
		Self {
			current_id: 0,
			next_id: 1,
			undo: Vec::new(),
			redo: Vec::new(),
			limit: 50,
		}
	}
}

impl History {
	/// Apply a command to `doc` and record it. Selection-only commands are
	/// applied without creating an undo step.
	///
	/// A command is all or nothing: when it fails, `doc` is put back as it was,
	/// whatever the command had changed before it gave up (HARDEN H3 — several
	/// commands validate as they go, and a half-applied edit with no History
	/// step could not be undone).
	pub fn execute(&mut self, doc: &mut Document, command: Command, ctx: &mut CommandContext<'_>) -> Result<CommandEffect, CommandError> {
		let before = doc.clone();
		let effect = match command.apply(doc, ctx) {
			Ok(effect) => effect,
			Err(error) => {
				*doc = before;
				return Err(error);
			}
		};
		doc.fit_levels();
		if !effect.selection_only {
			let before_id = self.current_id;
			self.current_id = self.next_id;
			self.next_id += 1;
			self.redo.clear();
			self.undo.push(HistoryEntry {
				before_id,
				after_id: self.current_id,
				content_changed: !effect.history_only,
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
		self.record_with_content(before, command, label, true);
	}

	pub fn record_with_content(&mut self, before: Document, command: Command, label: String, content_changed: bool) {
		let before_id = self.current_id;
		self.current_id = self.next_id;
		self.next_id += 1;
		self.redo.clear();
		self.undo.push(HistoryEntry {
			label,
			command,
			before,
			before_id,
			after_id: self.current_id,
			content_changed,
		});
		if self.undo.len() > self.limit {
			self.undo.remove(0);
		}
	}

	/// Returns false if there is nothing to undo.
	pub fn undo(&mut self, doc: &mut Document) -> bool {
		let Some(mut entry) = self.undo.pop() else { return false };
		self.current_id = entry.before_id;
		std::mem::swap(doc, &mut entry.before);
		// `entry.before` now holds the state to redo into.
		self.redo.push(entry);
		true
	}

	pub fn redo(&mut self, doc: &mut Document) -> bool {
		let Some(mut entry) = self.redo.pop() else { return false };
		self.current_id = entry.after_id;
		std::mem::swap(doc, &mut entry.before);
		self.undo.push(entry);
		true
	}

	/// Stable state identity, independent of the retained row count.
	pub fn current_id(&self) -> usize {
		self.current_id
	}
	pub fn state_id(&self, row: usize) -> Option<usize> {
		if row < self.undo.len() {
			Some(self.undo[row].before_id)
		} else if row == self.undo.len() {
			Some(self.current_id)
		} else {
			self.redo.len().checked_sub(row - self.undo.len()).map(|i| self.redo[i].after_id)
		}
	}
	pub fn state_row(&self, id: usize) -> Option<usize> {
		(0..=self.undo.len() + self.redo.len()).find(|&row| self.state_id(row) == Some(id))
	}
	pub fn state_by_id<'a>(&'a self, id: usize, current: &'a Document) -> Option<&'a Document> {
		if id == self.current_id {
			return Some(current);
		}
		self.undo
			.iter()
			.find(|e| e.before_id == id)
			.map(|e| &e.before)
			.or_else(|| self.redo.iter().find(|e| e.after_id == id).map(|e| &e.before))
	}
	pub fn next_changes_content(&self, redo: bool) -> bool {
		(if redo { self.redo.last() } else { self.undo.last() }).is_some_and(|e| e.content_changed)
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

	/// HARDEN H3: a command that fails half-way leaves the document as it was
	/// (MoveEach moved the first layer, then hit a locked one).
	#[test]
	fn a_failed_command_changes_nothing() {
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
		let pixel = |id| {
			Layer::new(
				id,
				"P",
				LayerKind::Pixel {
					image: fx_tiles::TiledImage::new(10, 10, fx_tiles::PixelFormat::Rgba8),
					offset: (0, 0),
				},
			)
		};
		let a = doc.allocate_layer_id();
		let b = doc.allocate_layer_id();
		let mut locked = pixel(b);
		locked.locked_position = true;
		doc.layers = vec![Arc::new(pixel(a)), Arc::new(locked)];
		let mut history = History::default();
		let result = history.execute(
			&mut doc,
			Command::MoveEach {
				moves: vec![(LayerRef::Id(a), 5, 5), (LayerRef::Id(b), 5, 5)],
				label: "Move".into(),
			},
			&mut ctx,
		);
		assert!(result.is_err(), "the locked layer refuses");
		let LayerKind::Pixel { offset, .. } = &doc.layer(a).unwrap().kind else {
			unreachable!()
		};
		assert_eq!(*offset, (0, 0), "and the first move is undone with it");
		assert!(!history.can_undo(), "no step was recorded");
	}
}

#[cfg(test)]
mod triage_tests {
	use super::*;
	use crate::{BitDepth, ColorProfile, DocumentColor};
	#[test]
	fn capped_history_preserves_cancel_checkpoint_and_source_identity() {
		let mut doc = Document::new(
			10,
			10,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let mut history = History::default();
		for _ in 0..50 {
			history.record(doc.clone(), Command::SelectAll, "edit".into());
		}
		let checkpoint = history.current_id();
		let source = history.state_id(25).unwrap();
		history.record(doc.clone(), Command::SelectAll, "preview".into());
		assert_eq!(history.labels().count(), 50);
		assert_ne!(history.current_id(), checkpoint);
		assert_eq!(history.state_id(24), Some(source));
		assert!(history.state_by_id(source, &doc).is_some());
		assert!(history.undo(&mut doc));
		assert_eq!(history.current_id(), checkpoint);
		assert_eq!(history.state_id(51), None);
	}
	#[test]
	fn selection_history_does_not_change_content_on_undo_or_redo() {
		let mut doc = Document::new(
			10,
			10,
			DocumentColor {
				depth: BitDepth::U8,
				profile: ColorProfile::Srgb,
			},
			72.0,
		);
		let mut history = History::default();
		history.record_with_content(doc.clone(), Command::SelectAll, "selection".into(), false);
		assert!(!history.next_changes_content(false));
		assert!(history.undo(&mut doc));
		assert!(!history.next_changes_content(true));
		assert!(history.redo(&mut doc));
	}
}

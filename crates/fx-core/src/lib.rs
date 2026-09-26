//! # fx-core — the document model
//!
//! * [`Document`] is a cheap-to-clone value: layers are `Arc<Layer>`, pixels are
//!   tile handles. Cloning a document is how we take snapshots for undo and for
//!   the render thread. Editing uses `Arc::make_mut`, so only the layers that
//!   actually change are copied (and only their tile *handles*).
//! * Every change to a document goes through a [`Command`]. Commands are plain
//!   serialisable data: the same value drives the UI, undo, macros ("actions")
//!   and batch processing. There is no other way to mutate a document.
//! * This crate does no rendering and no file I/O.

pub mod annotations;
pub mod blend;
pub mod channel;
pub mod color;
pub mod command;
pub mod document;
pub mod fill;
pub mod gradient;
pub mod history;
pub mod layer;
pub mod ops;
pub mod pattern;
pub mod pixels;
pub mod select_ops;
pub mod selection;
pub mod stroke;
pub mod styles;
pub mod text;
pub mod transform;
pub mod vector;

pub use blend::BlendMode;
pub use color::{BitDepth, ColorProfile, DocumentColor, RenderingIntent};
pub use command::{Command, CommandContext, CommandEffect, CommandError, LayerRef};
pub use document::NAME_KINDS;
pub use document::{Document, Guide};
pub use history::History;
pub use layer::{Adjustment, GradientStop, Layer, LayerId, LayerKind, Mask};
pub use ops::{Conversion, FilterParams, PixelOps};
pub use selection::{SelectMode, SelectModify, Selection, SelectionShape, WandParams};
pub use text::{FontStyle, TextAlign, TextAntialias, TextContent, TextFrame, TextRun};
pub use transform::{Anchor9, BezierPatch, Filter, Mapping, Permutation, dest_rect};
pub use vector::{Paint, PathEl, StrokeAlign, StrokeStyle, VectorShape};

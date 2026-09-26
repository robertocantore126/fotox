//! Selections computed from the document's pixels (M9-T02..T06). The engine
//! computes them (`PixelOps::select_op`); `Command::SelectBy` combines the
//! result with the current selection.

use serde::{Deserialize, Serialize};

/// Color Range's Select menu (M9-T03).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeKind {
	#[default]
	Sampled,
	Reds,
	Yellows,
	Greens,
	Cyans,
	Blues,
	Magentas,
	Highlights,
	Midtones,
	Shadows,
	SkinTones,
	OutOfGamut,
}

/// Select and Mask's settings (M9-T05).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refine {
	/// Edge Detection radius, pixels.
	pub radius: f32,
	#[serde(default)]
	pub smart_radius: bool,
	/// `0..=100`.
	#[serde(default)]
	pub smooth: f32,
	/// Pixels.
	#[serde(default)]
	pub feather: f32,
	/// `0..=100` %.
	#[serde(default)]
	pub contrast: f32,
	/// `-100..=100` %.
	#[serde(default)]
	pub shift_edge: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectOp {
	/// Select ▸ Grow: the selection's neighbours within `tolerance` of its
	/// colours, contiguous (M9-T02).
	Grow { tolerance: f64, sample_all: bool },
	/// Select ▸ Similar: the same, everywhere (M9-T02).
	Similar { tolerance: f64, sample_all: bool },
	/// Select ▸ Color Range (M9-T03). `samples` are straight RGB `0..=1`
	/// (Sampled Colors); `fuzziness` `0..=200`; `range` (%) and `center`
	/// localize the clusters when set.
	ColorRange {
		range: RangeKind,
		#[serde(default)]
		samples: Vec<[f32; 3]>,
		fuzziness: f32,
		#[serde(default)]
		localized: Option<(f64, f64, f64)>,
		#[serde(default)]
		invert: bool,
		/// Highlights / Shadows / Midtones split points `0..=255`.
		#[serde(default)]
		low: f32,
		#[serde(default = "full")]
		high: f32,
	},
	/// Select ▸ Focus Area (M9-T04): `in_focus` `0..=1` (higher = only the
	/// sharpest), `noise` `0..=1`, `soften` on/off.
	FocusArea { in_focus: f32, noise: f32, soften: bool },
	/// Select and Mask (M9-T05) on the current selection.
	Refine(Refine),
	/// The Quick Selection tool's stroke (M9-T06): dabs `(x, y, radius)`.
	QuickSelect {
		dabs: Vec<(f64, f64, f64)>,
		sample_all: bool,
		enhance_edge: bool,
	},
	/// A model's foreground probability (M13-T02/T04), upsampled to the canvas
	/// and refined in the edge band.
	Model(ModelMask),
}

/// Which model made a [`ModelMask`] (the History label).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
	/// Select ▸ Subject, Remove Background (BiRefNet).
	#[default]
	Subject,
	/// The Object Selection tool (EfficientSAM).
	Object,
	/// A mask Fotox made itself (Generative Expand's new area).
	Plain,
}

/// A mask at a model's working resolution: `width × height` values
/// `0..=255`, row-major, stretched over the canvas rectangle `rect`
/// (`x0, y0, x1, y1`, exclusive). Bounded by the model's resolution, never
/// the document's (rule 2). Kept in the command, so replay needs no model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelMask {
	#[serde(default)]
	pub kind: ModelKind,
	pub rect: (i64, i64, i64, i64),
	pub width: u32,
	pub height: u32,
	pub values: Vec<u8>,
	/// The edge refinement's radius in canvas pixels (M9-T05's guided
	/// filter); 0 = the upsampled mask as it is.
	#[serde(default)]
	pub refine_radius: f32,
}

impl ModelMask {
	/// The mask at canvas `(x, y)` (pixel centre), bilinear, `0..=1`; 0
	/// outside `rect`.
	pub fn at(&self, x: f64, y: f64) -> f32 {
		let (x0, y0, x1, y1) = self.rect;
		if x < x0 as f64 || y < y0 as f64 || x >= x1 as f64 || y >= y1 as f64 || self.width == 0 || self.height == 0 {
			return 0.0;
		}
		let (w, h) = (self.width as usize, self.height as usize);
		let u = (x - x0 as f64) * w as f64 / (x1 - x0).max(1) as f64 - 0.5;
		let v = (y - y0 as f64) * h as f64 / (y1 - y0).max(1) as f64 - 0.5;
		let fx = u.clamp(0.0, (w - 1) as f64);
		let fy = v.clamp(0.0, (h - 1) as f64);
		let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
		let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
		let (tx, ty) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
		let g = |xx: usize, yy: usize| f32::from(self.values[yy * w + xx]) / 255.0;
		let a = g(ix, iy) + (g(jx, iy) - g(ix, iy)) * tx;
		let b = g(ix, jy) + (g(jx, jy) - g(ix, jy)) * tx;
		a + (b - a) * ty
	}

	/// The lowest and highest mask value over the canvas rectangle `[x0, x1)
	/// × [y0, y1)` (one mask cell of margin), for the uniform-tile shortcut.
	pub fn range_over(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> (u8, u8) {
		let (rx0, ry0, rx1, ry1) = self.rect;
		if x1 <= rx0 || y1 <= ry0 || x0 >= rx1 || y0 >= ry1 || self.width == 0 || self.height == 0 {
			return (0, 0);
		}
		let (w, h) = (i64::from(self.width), i64::from(self.height));
		let cx = |x: i64| ((x - rx0) * w).div_euclid((rx1 - rx0).max(1)).clamp(0, w - 1);
		let cy = |y: i64| ((y - ry0) * h).div_euclid((ry1 - ry0).max(1)).clamp(0, h - 1);
		let (i0, i1) = ((cx(x0) - 1).max(0), (cx(x1) + 1).min(w - 1));
		let (j0, j1) = ((cy(y0) - 1).max(0), (cy(y1) + 1).min(h - 1));
		let mut lo = u8::MAX;
		let mut hi = 0u8;
		for j in j0..=j1 {
			for i in i0..=i1 {
				let v = self.values[(j * w + i) as usize];
				lo = lo.min(v);
				hi = hi.max(v);
			}
		}
		// Part of the rectangle lies outside the mask's: that part is 0.
		if x0 < rx0 || y0 < ry0 || x1 > rx1 || y1 > ry1 {
			lo = 0;
		}
		(lo, hi)
	}
}

fn full() -> f32 {
	255.0
}

impl SelectOp {
	pub fn label(&self) -> &'static str {
		match self {
			SelectOp::Grow { .. } => "Grow",
			SelectOp::Similar { .. } => "Similar",
			SelectOp::ColorRange { .. } => "Color Range",
			SelectOp::FocusArea { .. } => "Focus Area",
			SelectOp::Refine(_) => "Select and Mask",
			SelectOp::QuickSelect { .. } => "Quick Selection",
			SelectOp::Model(m) => match m.kind {
				ModelKind::Subject => "Select Subject",
				ModelKind::Object => "Object Selection",
				ModelKind::Plain => "Select",
			},
		}
	}
}

/// A vector mask as plain data (M10-T06).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorMaskSpec {
	pub path: crate::path::Path,
	#[serde(default = "yes")]
	pub enabled: bool,
	#[serde(default)]
	pub feather: f64,
	#[serde(default = "one")]
	pub density: f32,
}

fn yes() -> bool {
	true
}
fn one() -> f32 {
	1.0
}

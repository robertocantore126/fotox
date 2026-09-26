//! Smart Filters (M12-T03, D-083): the filter list re-evaluated on a Smart
//! Object's rendered pixels per requested tile and level.
//!
//! Each filter is a [`LevelSource`] stage over the one below it; the bottom
//! stage resamples the source composite lazily (any tile, any level, so a
//! filter's apron and its coarser blur levels are served too). Tiles are
//! memoised per stage for the duration of one draw.
//!
//! FAST: no filter mask; per-filter blending is opacity over Normal only (the
//! mode is ignored); every draw recomputes the aprons of its tiles.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fx_core::smart::SmartFilter;
use fx_ops::filter::{Geometry, filter_tile};
use fx_ops::neighbourhood::{LevelSource, TileRef};
use fx_ops::resample::SourceInfo;
use fx_tiles::{PixelFormat, TileBuffer, TileError, TileStore};

use crate::ops::ImageSource;

type Memo = Mutex<HashMap<(usize, i64, i64), Option<TileRef>>>;

/// The Smart Object's rendered (unfiltered) tiles, resampled on demand.
struct Rendered<'a> {
	view: &'a ImageSource<'a>,
	source: SourceInfo,
	transform: fx_core::Mapping,
	grid: (i64, i64),
	memo: Memo,
}

impl LevelSource for Rendered<'_> {
	fn format(&self) -> PixelFormat {
		self.view.image.format()
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		let (cols, rows) = ((self.grid.0 >> level).max(1) + 1, (self.grid.1 >> level).max(1) + 1);
		if tx < 0 || ty < 0 || tx >= cols || ty >= rows {
			return Ok(None);
		}
		if let Some(t) = self.memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(level, tx, ty)) {
			return Ok(t.clone());
		}
		let out = fx_ops::resample::resample(
			self.view,
			self.source,
			self.transform,
			fx_core::Filter::Bicubic,
			level,
			&[(tx as u32, ty as u32)],
		)?;
		let t = out.into_iter().next().map(|(_, b)| TileRef::Data(Arc::new(b)));
		self.memo
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.insert((level, tx, ty), t.clone());
		Ok(t)
	}
}

/// One Smart Filter over the stage below.
struct Stage<'a> {
	below: &'a dyn LevelSource,
	filter: &'a SmartFilter,
	geometry: Geometry,
	memo: Memo,
}

impl LevelSource for Stage<'_> {
	fn format(&self) -> PixelFormat {
		self.below.format()
	}

	fn tile(&self, level: usize, tx: i64, ty: i64) -> Result<Option<TileRef>, TileError> {
		if tx < 0 || ty < 0 {
			return Ok(None);
		}
		if let Some(t) = self.memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(level, tx, ty)) {
			return Ok(t.clone());
		}
		let mut filtered = filter_tile(self.below, &self.geometry, &self.filter.filter, level, tx as u32, ty as u32)?;
		if self.filter.opacity < 1.0 {
			let below = self.below.tile(level, tx, ty)?;
			mix(&mut filtered, below.as_ref(), self.filter.opacity, self.format());
		}
		let t = Some(TileRef::Data(Arc::new(filtered)));
		self.memo
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.insert((level, tx, ty), t.clone());
		Ok(t)
	}
}

/// `out = below + (out − below) · opacity`, per 16-bit / 8-bit channel.
fn mix(out: &mut TileBuffer, below: Option<&TileRef>, opacity: f32, format: PixelFormat) {
	let k = opacity.clamp(0.0, 1.0);
	match format {
		PixelFormat::Rgba16 => {
			let dst = out.as_u16_mut();
			for (i, v) in dst.iter_mut().enumerate() {
				let b = match below {
					Some(TileRef::Solid(s)) => s[i % 4],
					Some(TileRef::Data(d)) => d.as_u16()[i],
					None => 0,
				};
				*v = (f32::from(b) + (f32::from(*v) - f32::from(b)) * k).round() as u16;
			}
		}
		_ => {
			let dst = out.bytes_mut();
			for (i, v) in dst.iter_mut().enumerate() {
				let b = match below {
					Some(TileRef::Solid(s)) => (s[i % 4] >> 8) as u8,
					Some(TileRef::Data(d)) => d.bytes()[i],
					None => 0,
				};
				*v = (f32::from(b) + (f32::from(*v) - f32::from(b)) * k).round() as u8;
			}
		}
	}
}

/// Run `filters` over the requested tiles of one level (their unfiltered
/// resample is `tiles`; the aprons are resampled as needed).
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(
	view: &ImageSource<'_>,
	source: SourceInfo,
	transform: fx_core::Mapping,
	filters: &[SmartFilter],
	level: usize,
	canvas: (u32, u32),
	tiles: Vec<((u32, u32), TileBuffer)>,
	_store: &TileStore,
) -> Result<Vec<((u32, u32), TileBuffer)>, TileError> {
	let t = i64::from(fx_tiles::TILE_SIZE);
	let grid = ((i64::from(canvas.0) + t - 1) / t, (i64::from(canvas.1) + t - 1) / t);
	let rendered = Rendered {
		view,
		source,
		transform,
		grid,
		memo: Mutex::new(
			tiles
				.iter()
				.map(|((tx, ty), b)| ((level, i64::from(*tx), i64::from(*ty)), Some(TileRef::Data(Arc::new(b.clone())))))
				.collect(),
		),
	};
	let geometry = Geometry {
		offset: (0, 0),
		canvas,
		image: canvas,
	};
	if filters.is_empty() {
		return Ok(tiles);
	}
	let wanted: Vec<(u32, u32)> = tiles.iter().map(|(k, _)| *k).collect();
	run(&rendered, filters, geometry, level, &wanted, view.image.format())
}

/// Stack `filters` (bottom first) over `below` and read the wanted tiles of
/// the top stage.
fn run(
	below: &dyn LevelSource,
	filters: &[SmartFilter],
	geometry: Geometry,
	level: usize,
	wanted: &[(u32, u32)],
	format: PixelFormat,
) -> Result<Vec<((u32, u32), TileBuffer)>, TileError> {
	let Some((filter, rest)) = filters.split_first() else {
		return wanted
			.iter()
			.map(|&(tx, ty)| {
				let out = match below.tile(level, i64::from(tx), i64::from(ty))? {
					Some(TileRef::Data(b)) => (*b).clone(),
					Some(TileRef::Solid(v)) => TileBuffer::filled(format, fx_tiles::PixelValue(v)),
					None => TileBuffer::zeroed(format),
				};
				Ok(((tx, ty), out))
			})
			.collect();
	};
	let stage = Stage {
		below,
		filter,
		geometry,
		memo: Mutex::new(HashMap::new()),
	};
	run(&stage, rest, geometry, level, wanted, format)
}

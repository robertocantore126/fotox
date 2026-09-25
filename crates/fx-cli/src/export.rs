//! `fotox-cli export` — open an image, optionally build B3 on it, and export
//! the flattened result exactly like File ▸ Export in the app (M3).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use fx_core::{Document, DocumentColor, Layer, LayerKind};
use fx_engine::b3;
use fx_engine::export::{export_document, opaque_background, options_for};
use fx_tiles::{TileStore, TileStoreConfig};

pub fn run(input: &Path, output: &Path, with_b3: bool) -> Result<()> {
	let dir = std::env::temp_dir().join("fotox-cli-export");
	let store = TileStore::new(TileStoreConfig::reference_machine(dir)).context("cannot create the tile store")?;

	let start = Instant::now();
	let imported = fx_io::import_file(input, &store, &mut |_| true).map_err(|e| anyhow!("cannot open {}: {e}", input.display()))?;
	let mut doc = Document::new(
		imported.width,
		imported.height,
		DocumentColor {
			depth: imported.depth,
			profile: imported.profile,
		},
		imported.ppi,
	);
	let id = doc.allocate_layer_id();
	doc.layers.push(Arc::new(Layer::new(
		id,
		"Background",
		LayerKind::Pixel {
			image: imported.image,
			offset: (0, 0),
		},
	)));
	println!(
		"opened {} ({} × {}) in {:.2} s",
		input.display(),
		doc.width,
		doc.height,
		start.elapsed().as_secs_f64()
	);

	if with_b3 {
		let start = Instant::now();
		let ids: Vec<_> = (0..b3::PIXEL_LAYERS + b3::ADJUSTMENT_LAYERS).map(|_| doc.allocate_layer_id()).collect();
		let layers = b3::build(doc.width, doc.height, doc.color.depth.rgba_format(), &ids, 3, &store);
		doc.layers.extend(layers);
		println!("built B3: {} layers in {:.2} s", doc.layers.len(), start.elapsed().as_secs_f64());
	}

	let start = Instant::now();
	let opaque = opaque_background(&doc, &store);
	let options = options_for(&doc, output, opaque).map_err(|e| anyhow!("{e}"))?;
	let mut last = 0.0;
	export_document(&doc, &store, output, options, &mut |fraction| {
		if fraction - last >= 0.1 || fraction >= 1.0 {
			last = fraction;
			eprint!("\rexporting… {:3.0} %", fraction * 100.0);
		}
		true
	})
	.map_err(|e| anyhow!("cannot export {}: {e}", output.display()))?;
	eprintln!();
	let seconds = start.elapsed().as_secs_f64();
	let megapixels = f64::from(doc.width) * f64::from(doc.height) / 1e6;
	let bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
	println!(
		"exported {} ({:?}, {} bit, alpha {}) in {seconds:.2} s — {:.1} MP/s, {:.1} MB",
		output.display(),
		options.format,
		options.bits,
		options.alpha,
		megapixels / seconds,
		bytes as f64 / 1e6
	);
	Ok(())
}

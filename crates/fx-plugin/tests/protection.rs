//! Single-file plugins and the protection against buggy ones (D-098): the
//! example in `plugins/AI-PROMPT.md` builds and runs; a file that does not
//! compile leaves its errors beside it; a plugin that panics, loops, eats
//! memory or returns NaN is stopped or ignored, never trusted; a manifest
//! with a bad or taken id is refused.
//!
//! Builds real `.rs` plugins with cargo (wasm32 target needed), in
//! `FOTOX_PLUGIN_BUILD` = a scratch folder; the first build takes a few
//! seconds.

use std::path::PathBuf;
use std::sync::Once;

const HEADER: fn() -> [f32; fx_plugin::HEADER_WORDS] = || fx_plugin::header(&[50.0], [1.0, 1.0, 1.0, 1.0], false);

fn folder() -> PathBuf {
	static SETUP: Once = Once::new();
	let base = std::env::temp_dir().join(format!("fx-plugin-protection-{}", std::process::id()));
	SETUP.call_once(|| {
		std::fs::create_dir_all(base.join("plugins")).unwrap();
		// SAFETY: set once, before any build reads it.
		unsafe { std::env::set_var("FOTOX_PLUGIN_BUILD", base.join("build")) };
	});
	base.join("plugins")
}

/// A one-file plugin `id` whose `rect` body is `body` (`ctx`, `pixels`, `k`
/// in scope).
fn script(id: &str, body: &str) -> PathBuf {
	let source = format!(
		r##"use fotox_plugin::{{Ctx, brush_plugin}};

const MANIFEST: &str = r#"{{ "id": "{id}", "name": "Test {id}", "slot": "eraser", "params": ["Strength"] }}"#;

brush_plugin! {{
	manifest: MANIFEST,
	rect: rect,
}}

#[allow(unused_variables, unused_mut)]
fn rect(ctx: &Ctx, pixels: &mut [[f32; 4]], k: &[f32]) {{
	{body}
}}
"##
	);
	let path = folder().join(format!("{id}.rs"));
	std::fs::write(&path, source).unwrap();
	path
}

fn grey(n: usize) -> (Vec<[f32; 4]>, Vec<f32>) {
	(vec![[0.4, 0.4, 0.4, 0.8]; n], vec![0.5; n])
}

#[test]
fn the_ai_prompt_example_builds_and_erases() {
	let prompt = include_str!("../../../plugins/AI-PROMPT.md");
	let start = prompt.find("### Complete example").unwrap();
	let code = &prompt[start..];
	let code = &code[code.find("```rust\n").unwrap() + 8..];
	let code = &code[..code.find("```").unwrap()];
	let path = folder().join("shadow-eraser.rs");
	std::fs::write(&path, code).unwrap();
	let plugin = fx_plugin::load_file(&path).unwrap_or_else(|e| panic!("the prompt's example must build: {e}"));
	assert_eq!(plugin.manifest.name, "Shadow Eraser Tool");
	assert!(plugin.has_gray);
	// A dark pixel at full build-up goes; a light one at a light touch stays.
	let mut pixels = vec![[0.05, 0.05, 0.05, 1.0], [0.95, 0.95, 0.95, 1.0]];
	let header = fx_plugin::header(&[40.0], [1.0, 1.0, 1.0, 1.0], false);
	fx_plugin::rect(plugin.key, &header, (0, 0), (2, 1), &mut pixels, &[1.0, 0.2]).unwrap();
	assert!(pixels[0][3] < 0.01, "dark erased: {:?}", pixels[0]);
	assert!(pixels[1][3] > 0.99, "light kept: {:?}", pixels[1]);
	// A rebuild of an unchanged file is a cache hit.
	assert!(fx_plugin::script::cached(&path).is_some());
}

#[test]
fn a_file_that_does_not_compile_leaves_its_errors_beside_it() {
	let path = script("broken-code", "let x: u32 = \"not a number\";");
	let error = match fx_plugin::load_file(&path) {
		Ok(_) => panic!("must not build"),
		Err(e) => e,
	};
	assert!(error.contains("does not compile"), "{error}");
	let errors = std::fs::read_to_string(fx_plugin::script::errors_file(&path)).unwrap();
	assert!(errors.contains("Paste this whole file to the AI"), "{errors}");
	assert!(errors.contains("mismatched types"), "the compiler's own message: {errors}");
	// Fixed: it builds, and the errors file goes.
	let path = script("broken-code", "for p in pixels.iter_mut() { p[3] *= 0.5; }");
	fx_plugin::load_file(&path).unwrap();
	assert!(!fx_plugin::script::errors_file(&path).exists());
}

#[test]
fn a_panicking_plugin_is_stopped_and_paints_nothing() {
	let plugin = fx_plugin::load_file(&script("panics", "let i = pixels.len() + 5; pixels[i][0] = 1.0;")).unwrap();
	let (mut pixels, k) = grey(16);
	let before = pixels.clone();
	assert!(fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (4, 4), &mut pixels, &k).is_err());
	assert_eq!(pixels, before, "nothing painted");
	assert!(plugin.stopped().is_some_and(|why| why.contains("crashed")), "{:?}", plugin.stopped());
	assert!(fx_plugin::take_errors().iter().any(|e| e.contains("Test panics") && e.contains("stopped")));
	// Stopped: later calls fail at once, without running it again.
	let started = std::time::Instant::now();
	assert!(fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (4, 4), &mut pixels, &k).is_err());
	assert!(started.elapsed() < std::time::Duration::from_millis(50));
}

#[test]
fn an_endless_loop_is_cut_off_and_stopped() {
	let plugin = fx_plugin::load_file(&script("loops", "loop { if pixels[0][0] > 2.0 { break; } pixels[0][0] *= 0.5; }")).unwrap();
	let (mut pixels, k) = grey(4);
	let started = std::time::Instant::now();
	assert!(fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (2, 2), &mut pixels, &k).is_err());
	assert!(
		started.elapsed() < std::time::Duration::from_secs(3),
		"the deadline cut it: {:?}",
		started.elapsed()
	);
	assert!(plugin.stopped().is_some_and(|why| why.contains("too long")), "{:?}", plugin.stopped());
}

#[test]
fn a_memory_hog_is_stopped() {
	let plugin = fx_plugin::load_file(&script(
		"hog",
		"let big = std::hint::black_box(vec![1u8; 1 << 30]); pixels[0][0] = big[12345] as f32;",
	))
	.unwrap();
	let (mut pixels, k) = grey(4);
	assert!(fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (2, 2), &mut pixels, &k).is_err());
	assert!(plugin.stopped().is_some(), "1 GB is over the {} MB cap", fx_plugin::MEMORY_LIMIT >> 20);
}

#[test]
fn nan_pixels_are_ignored_the_rest_is_kept() {
	let plugin = fx_plugin::load_file(&script(
		"nan",
		"for (i, p) in pixels.iter_mut().enumerate() { if i % 2 == 0 { p[0] = f32::NAN; } else { p[3] = 0.25; } }",
	))
	.unwrap();
	let (mut pixels, k) = grey(4);
	fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (2, 2), &mut pixels, &k).unwrap();
	assert_eq!(pixels[0], [0.4, 0.4, 0.4, 0.8], "a NaN pixel keeps its input");
	assert_eq!(pixels[1][3], 0.25, "a good pixel is taken");
	assert!(plugin.stopped().is_none());
	assert!(fx_plugin::take_errors().iter().any(|e| e.contains("invalid values")));
}

#[test]
fn a_taken_or_bad_id_is_refused() {
	fx_plugin::load_file(&script("twin", "")).unwrap();
	// Another file with the same id.
	let source = std::fs::read_to_string(folder().join("twin.rs")).unwrap();
	let other = folder().join("twin-copy.rs");
	std::fs::write(&other, source).unwrap();
	let error = fx_plugin::load_file(&other).err().unwrap();
	assert!(error.contains("already used"), "{error}");
	let error = fx_plugin::load_file(&script("Bad Id", "")).err().unwrap();
	assert!(error.contains("must be 1-64 characters"), "{error}");
}

#[test]
fn reload_restarts_a_stopped_plugin() {
	let dir = folder().join("reload");
	std::fs::create_dir_all(&dir).unwrap();
	let source = std::fs::read_to_string(script("crashy", "let i = pixels.len(); pixels[i][0] = 1.0;")).unwrap();
	let path = dir.join("crashy.rs");
	std::fs::write(&path, source).unwrap();
	let _ = std::fs::remove_file(folder().join("crashy.rs"));
	let plugin = fx_plugin::load_file(&path).unwrap();
	let (mut pixels, k) = grey(4);
	assert!(fx_plugin::rect(plugin.key, &HEADER(), (0, 0), (2, 2), &mut pixels, &k).is_err());
	assert!(plugin.stopped().is_some());
	let (tx, rx) = std::sync::mpsc::channel();
	fx_plugin::watch(dir, move |changes| {
		let _ = tx.send(changes);
	});
	fx_plugin::request_reload();
	let changes = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
	assert_eq!(
		changes,
		[fx_plugin::Change::Reloaded {
			names: vec!["Test crashy".into()]
		}]
	);
	let fresh = fx_plugin::get(plugin.key).unwrap();
	assert!(fresh.stopped().is_none(), "a reload starts it fresh");
}

# M3 (start) — flat TIFF / PNG export

Agent: Claude  ·  Branch: `task/M3-export-writers`  ·  Status: code done, not tried in the app. There are no M3 task cards yet; this is the uncontroversial part of the ROADMAP's "TIFF/PNG/JPEG export", written so the cards can build on it.

## What was done

* **`fx-io::tiff_write`** — the streaming TIFF writer of `fotox-cli gen` moved into `fx-io` (the CLI uses it from there) and learnt RGBA (ExtraSamples = unassociated alpha). Errors are `IoError` now.
* **`fx-io::export`** — `export_image(path, width, height, options, render, progress)`: the caller renders one band of 256 rows (straight RGBA16), the module converts (16 → 8 bits rounded; without alpha, flattened onto white) and encodes TIFF (strips = bands, BigTIFF when needed) or PNG (`png` stream writer, big-endian 16-bit, fast compression). Written to `<name>.part` and renamed at the end: a failed or cancelled export never destroys an existing file. Peak memory: one band.
* **`fx-engine::export`** — `export_document`: the CPU reference compositor renders each row of tiles (tiles in parallel with rayon), un-premultiplies, hands the band to `export_image`. Options: format from the extension, the document's bit depth, transparency kept.
* **Wiring** — *File ▸ Export ▸ PNG / TIFF* in the app: the shell shows the native save dialog (helper thread, like Open), `EngineInput::Export(path)` → a job on a worker thread with `progress` in the status bar and a toast "Exported …" (errors as `error`). The browser mock still shows its Export As dialog.

## Decisions for Rob (candidates for the M3 cards)

* **Transparency**: kept only when the document can have any — an opaque bottom layer (every canvas pixel at alpha 1, full opacity, no mask) means an opaque composite, so the file is written without alpha (`opaque_background`, reads that layer once). An Export dialog with a Transparency checkbox is still the proper control.
* **Bit depth = the document's.** No 16 → 8 choice yet.
* **The reference compositor is slow** (f64 per pixel): fine for photos, minutes for B1 (30 000²). A GPU readback path (the compositor already renders tiles) is the fast route; the file side does not change.
* **JPEG export** needs an encoder crate (`jpeg-encoder` is the usual pure-Rust choice) — not added without asking.
* No ICC profile yet (M4). The document ppi is written (TIFF resolution, PNG `pHYs`).

## Verification

```text
cargo fmt --check / clippy --workspace -D warnings / cargo test --workspace: clean
fx-io export tests: TIFF and PNG × 8/16 bit × alpha on/off, 300 × 530 (three bands, the last short),
  exported and re-imported with fx-io's own importers: every probed pixel exact (incl. flatten-on-white and 16→8 rounding);
  a cancelled export leaves the existing file untouched and no .part behind.
fx-engine export test: red background + 50 % blue layer over 300 of 400 columns (a Data tile across the tile edge),
  exported as 16-bit PNG and re-imported: (32767, 0, 32768, 65535) under the blue, pure red right of it.
```

Not tried in the app (Rob): File ▸ Export ▸ PNG / TIFF on an open document, progress in the status bar, the file opens in Photoshop.

## Addendum — `fotox-cli export`

`fotox-cli export <input> <output.tif|png> [--b3]` opens an image, optionally builds B3 on it, and exports exactly like File ▸ Export (same `fx_engine::export` code), printing the time. Measured (release, 6000 × 4000 16-bit test image from `gen`):

```text
single layer → PNG 16-bit (alpha dropped, opaque):  0.77 s, 31 MP/s
single layer → TIFF 16-bit:                          0.52 s, 46 MP/s — byte-identical pixels to the input
                                                      (only the resolution tag differs: 72/1 vs 7200/100)
B3 on top (220 layers) → PNG 16-bit:                 6.29 s, 3.8 MP/s (CPU reference compositor)
```

B1 (30 000²) with B3 would take ~4 min at that rate: the GPU readback path of M3-T07 is worth it.

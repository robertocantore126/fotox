# Snippets — the small pieces that are easy to get wrong

Companion to `HOWTO.md` and the cards M3–M6. Each snippet shows the
**correct** form of a detail that looks trivial and is not, with the usual
wrong version named. They are reference sketches written by Claude, **not
compiled**: adapt names and types to the code, keep the logic, and keep the
test the snippet mentions.

---

## 1. Conversions and rounding (every milestone)

```rust
/// 16 → 8 bit, rounded: the inverse of `v8 * 257`.
/// Wrong: `(v >> 8) as u8` (biased down by ½ step), `(v / 257) as u8` (truncates).
#[inline]
pub fn to_8(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32767) / 65535) as u8
}

/// 8 → 16 bit: 255 must become 65535. Wrong: `v << 8` (255 → 65280).
#[inline]
pub fn to_16(v: u8) -> u16 {
    u16::from(v) * 257
}

/// f32 in 0..=1 → u16, clamped and rounded. (`as` saturates and maps NaN to 0.)
/// Wrong: `(x * 65535.0) as u16` (truncates: 0.99999 → 65534).
#[inline]
pub fn f_to_16(x: f32) -> u16 {
    (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}
```

## 2. Premultiply before averaging (blur, resample, mips, dabs, heal)

```rust
#[inline]
fn premul([r, g, b, a]: [f32; 4]) -> [f32; 4] {
    [r * a, g * a, b * a, a]
}

/// Transparent pixels have no colour: return 0, never divide by ~0.
#[inline]
fn unpremul([r, g, b, a]: [f32; 4]) -> [f32; 4] {
    if a <= 1.0 / 65535.0 { [0.0; 4] } else { [r / a, g / a, b / a, a] }
}

// Any weighted sum of pixels: premul → sum → unpremul.
// Wrong: averaging straight colours → a dark fringe where opaque red meets
// transparent black. Test: half-transparent red next to transparent pixels,
// blurred: every output pixel with a > 0 is still pure red.
```

After a filter with negative lobes (bicubic, unsharp), clamp **premultiplied**
colour to `0..=a` before un-premultiplying, or colours above 1.0 appear.

## 3. Negative coordinates → tile and pixel (layers with offsets)

```rust
/// Document pixel → (tile, pixel inside the tile) of a layer at `offset`.
/// Layer-local coordinates are negative left/above the layer origin.
fn locate(x: i64, y: i64, offset: (i32, i32)) -> ((i64, i64), (u32, u32)) {
    let (lx, ly) = (x - i64::from(offset.0), y - i64::from(offset.1));
    (
        (lx.div_euclid(256), ly.div_euclid(256)),
        (lx.rem_euclid(256) as u32, ly.rem_euclid(256) as u32),
    )
}
// Wrong: `lx / 256` and `lx % 256` round toward zero: pixel −1 lands in tile 0
// at index −1 (or wraps to a huge u32). Test: offset (−10, −10), pixel (−1, −1).
```

## 4. Neighbourhood with apron and canvas edges (M4-T05, every filter)

```rust
// Output tile (tx, ty) of the *layer*; apron `a` px. Sample positions in
// document space:
//   x in (ox + tx·256 − a) .. (ox + tx·256 + 256 + a), same for y.
// For each position:
//   1. outside the canvas → clamp to the canvas (edge replicate, M4-T00-6):
let cx = x.clamp(0, i64::from(canvas_w) - 1);
let cy = y.clamp(0, i64::from(canvas_h) - 1);
//   2. then look the clamped position up in the layer: no tile / Empty →
//      transparent (no fetch), Solid → the value, Data → the pixel.
// Wrong order (look up first, clamp in layer space): layers smaller than the
// canvas get their own edge smeared across the canvas.
// Fetch each neighbour tile once per output tile (a 3×3 array of
// Option<Arc<TileBuffer>> when a ≤ 256), not once per pixel.
```

## 5. Gaussian kernel and the three-box approximation (M4-T06)

```rust
/// Normalised kernel, half-width ceil(3σ). σ < 0.3 → identity.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return vec![1.0];
    }
    let r = (3.0 * sigma).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum); // Wrong: skipping this darkens/brightens
    k
}

/// Widths (odd) of `n` successive box blurs ≈ Gaussian σ (Kovesi 2010).
fn boxes_for_gauss(sigma: f64, n: usize) -> Vec<usize> {
    let nf = n as f64;
    let w_ideal = (12.0 * sigma * sigma / nf + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let (wlf, s2) = (wl as f64, sigma * sigma);
    let m = ((12.0 * s2 - nf * wlf * wlf - 4.0 * nf * wlf - 3.0 * nf) / (-4.0 * wlf - 4.0)).round() as usize;
    (0..n).map(|i| if i < m { wl as usize } else { wu as usize }).collect()
}

/// One box pass over a line that carries `r` extra samples on each side;
/// writes `src.len() − 2r` values. f64 accumulator: an f32 running sum drifts.
fn box_line(src: &[[f32; 4]], r: usize, dst: &mut [[f32; 4]]) {
    let w = 2 * r + 1;
    debug_assert_eq!(dst.len(), src.len() - 2 * r);
    let mut acc = [0f64; 4];
    for p in &src[..w] {
        for c in 0..4 { acc[c] += f64::from(p[c]); }
    }
    let inv = 1.0 / w as f64;
    for i in 0..dst.len() {
        dst[i] = acc.map(|v| (v * inv) as f32);
        if i + w < src.len() {
            for c in 0..4 { acc[c] += f64::from(src[i + w][c]) - f64::from(src[i][c]); }
        }
    }
}
// Three passes shrink the line three times: the apron is the SUM of the three
// radii per side, not the largest. Horizontal passes run over the apron rows
// too (the vertical passes need them).
```

## 6. Pixel coverage of a rectangle (M5-T03 marquee, M6 crop)

```rust
/// Exact area of pixel (px, py) — the square [px, px+1) × [py, py+1) —
/// covered by the rectangle [x0, x1) × [y0, y1).
fn rect_coverage(px: i64, py: i64, (x0, y0, x1, y1): (f64, f64, f64, f64)) -> f32 {
    let ox = ((px + 1) as f64).min(x1) - (px as f64).max(x0);
    let oy = ((py + 1) as f64).min(y1) - (py as f64).max(y0);
    (ox.max(0.0) * oy.max(0.0)) as f32
}
// Wrong: testing only the pixel centre (no anti-aliasing) or the pixel corner
// (everything shifts by half a pixel). Pixel centres are at (px + 0.5, py + 0.5).
```

## 7. Combining selections (M5-T03)

```rust
let out = match mode {
    SelectMode::Replace => new,
    SelectMode::Add => old.max(new),
    SelectMode::Subtract => old.min(1.0 - new), // Wrong: `old - new` (goes negative, then wraps in u16)
    SelectMode::Intersect => old.min(new),      // Wrong: `old * new` (soft edges get darker)
};
```

## 8. Dab spacing that does not depend on event batching (M5-T06)

```rust
/// Walks the pen path and emits dabs every `spacing` px. The distance since
/// the last dab is carried across segments AND across event batches.
pub struct DabWalker {
    last: Option<Sample>,
    since_dab: f64,
}

impl DabWalker {
    pub fn push(&mut self, s: Sample, spacing_px: impl Fn(&Sample) -> f64, out: &mut Vec<Sample>) {
        let Some(a) = self.last.replace(s) else {
            out.push(s); // pen-down: one dab right away
            self.since_dab = 0.0;
            return;
        };
        let len = ((s.x - a.x).powi(2) + (s.y - a.y).powi(2)).sqrt();
        if len == 0.0 {
            return;
        }
        let mut pos = 0.0;
        loop {
            // Spacing at the current point (pressure changes the diameter).
            let step = spacing_px(&a.lerp(&s, pos / len)).max(0.5);
            let need = (step - self.since_dab).max(0.0);
            if pos + need > len {
                self.since_dab += len - pos;
                break;
            }
            pos += need;
            self.since_dab = 0.0;
            out.push(a.lerp(&s, pos / len));
        }
    }
}
// Wrong: resetting `since_dab` for every batch of pointer events, or measuring
// spacing from the start of each segment: the result then depends on how
// Windows batched the events. Test: the same samples fed 1 at a time vs all
// at once → identical dab lists.
```

## 9. Stroke build-up with an opacity ceiling (M5-T06)

```rust
// Per pixel, per dab (d = dab coverage × selection, 0..1):
s = s + flow * d * (1.0 - s);

// Per pixel, when a batch of dabs is done — ALWAYS from the layer as it was
// at pen-down, never from the current layer:
out_px = blend::composite(mode, before_px, colour, opacity * s);

// Wrong: compositing every dab onto the current layer. Opacity then stops
// being a ceiling (overlapping passes go above it) and Multiply darkens on
// every dab instead of once per stroke.
```

## 10. Picking the source mip before resampling (M6-T01)

```rust
// Local scale of the mapping (destination px per source px), from the Jacobian
// at the destination tile's centre; for affine maps it is constant.
let scale = jacobian_min_singular_value; // < 1 when reducing
let level = if scale < 0.5 { (1.0 / scale).log2().floor() as usize } else { 0 };
let level = level.min(image.level_count() - 1);
// Sample from `level`: source coordinates / 2^level; kernel on that level.
// Wrong: bicubic straight from level 0 at 10 %: 4 taps out of 100 pixels →
// aliasing and moiré. Test: a 1-px checkerboard reduced to 25 % is flat grey.
```

## 11. Bicubic sampling with the right half-pixel (M6-T01)

```rust
/// Keys' cubic, a = −0.5 ("Bicubic"), −0.75 ("Bicubic Sharper").
fn keys(x: f64, a: f64) -> f64 {
    let x = x.abs();
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a
    } else {
        0.0
    }
}

// Source position u (continuous, pixel CENTRES at i + 0.5) from the inverse
// mapping of the destination pixel centre (dx + 0.5, dy + 0.5):
let base = (u - 0.5).floor();
let t = u - 0.5 - base;
let w = [keys(t + 1.0, a), keys(t, a), keys(1.0 - t, a), keys(2.0 - t, a)];
// taps: base − 1, base, base + 1, base + 2 (premultiplied), then clamp 0..=a.
// Wrong: `u.floor()` without the −0.5 → the whole image shifts by half a
// pixel on every transform (visible after a few transforms).
// Always map DESTINATION pixels back to the source (inverse mapping); never
// "splat" source pixels forward (holes and double hits).
```

## 12. `.fxd`: positional reads on Windows (M3-T01)

```rust
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}
// Two traps: (1) `seek_read` may return FEWER bytes than asked — loop.
// (2) On Windows `seek_read` MOVES the file cursor. The writer appending to
// the same file must not rely on the cursor: give it its own handle, or
// `seek(SeekFrom::Start(end))` before every write.
```

## 13. `.fxd`: finding the newest valid footer (M3-T01)

```rust
/// Newest valid footer ending at or before `end`. 1 MiB blocks read
/// backwards, overlapping by 63 bytes so a footer that crosses a block
/// boundary is not missed.
fn find_footer(file: &File, end: u64) -> io::Result<Option<(u64, Footer)>> {
    const BLOCK: u64 = 1 << 20;
    let mut hi = end;
    while hi >= 64 {
        let lo = hi.saturating_sub(BLOCK);
        let mut buf = vec![0u8; (hi - lo) as usize];
        read_exact_at(file, &mut buf, lo)?;
        // footer starts i with i + 64 <= buf.len(), newest first
        for i in (0..=buf.len() - 64).rev() {
            if &buf[i..i + 8] == b"FXDEND01" {
                let at = lo + i as u64;
                if let Some(f) = Footer::parse(&buf[i..i + 64]) {
                    if f.checksum_ok() && f.end_offset == at + 64 {
                        return Ok(Some((at, f)));
                    }
                }
            }
        }
        if lo == 0 {
            break;
        }
        hi = lo + 63; // overlap
    }
    Ok(None)
}
// Wrong: non-overlapping blocks (a footer across the boundary is invisible),
// or accepting a magic without checking `end_offset` (the bytes "FXDEND01"
// can occur inside compressed tile data).
```

Save order for crash safety: append chunks → `sync_data()` → write the footer
→ `sync_data()`. Wrong: one `sync` at the end (the footer may reach the disk
before the chunks it points to).

## 14. 3D LUT coordinates (M4-T02)

```wgsl
// 33 nodes per axis: node i sits at the centre of texel i, i.e. (i + 0.5) / 33.
let uvw = rgb * (32.0 / 33.0) + vec3<f32>(0.5 / 33.0);
let out = textureSampleLevel(lut, lut_sampler, uvw, 0.0);
// Wrong: `textureSample(lut, s, rgb)` — black and white land on texel EDGES,
// half a texel off: the identity LUT is then not the identity (test C1 fails).
```

Apply the LUT to **straight** colour: un-premultiply, LUT, re-premultiply.

## 15. 1-D Euclidean distance transform (M5-T03 expand/contract, M6-T08 stroke)

```rust
/// Felzenszwalb–Huttenlocher, squared distances. `f[i]` = 0 on the shape,
/// FAR elsewhere. Run on rows, then on columns of the row result.
const FAR: f64 = 1e20; // Wrong: f64::INFINITY → ∞ − ∞ = NaN in `s`
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    let mut k = 0;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2 * q - 2 * p) as f64;
            if s > z[k] {
                break;
            }
            k -= 1; // never below 0: z[0] = −∞
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        d[q] = (q as f64 - p as f64).powi(2) + f[p];
    }
}
// Distances are only right if the tile's apron is ≥ the radius: pixels
// farther than the apron are "unknown", not "far". v and z need n and n + 1 entries.
```

## 15b. Flood fill without a document-sized bitmap (M5-T04)

```rust
// Work list of tiles with seed spans; the OUTPUT selection tiles double as
// the "visited" set. No recursion (stack overflow on big regions).
let mut work: VecDeque<((u32, u32), Vec<(u32, u32)>)> = VecDeque::from([(seed_tile, vec![seed_px])]);
while let Some((tile, seeds)) = work.pop_front() {
    let out = selection_tile_mut(tile);          // created on first visit
    let spill = scanline_fill(tile_pixels(tile), out, &seeds, tolerance);
    // `spill`: pixels on the tile border that were filled → seeds for the
    // neighbour tiles (only if that neighbour pixel is not already selected).
    for (neighbour, px) in spill { push_or_merge(&mut work, neighbour, px); }
}
// Tolerance in 8-bit units also for 16-bit documents:
//   |a − b| <= tolerance * 257   (Wrong: `<= tolerance` → 16-bit wand selects nothing)
```

## 16. Protocol and UI details (all milestones)

```rust
// New field on an existing message: old UIs must still parse.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub styles: Option<LayerStyles>,
// Wrong: a plain new field → "missing field" errors for every older sender,
// including the browser mock.
```

```js
// dialogs.js reads values BY LABEL, colon included, and only from rng()/sel/chk.
rng("Radius:", 4, { min: 1, max: 10000 })    // values["Radius:"]
num("Radius:", 4)                            // NOT read by readValues
// Sliders are integers: store tenths/hundredths and divide in fromValues
// (see the Exposure mapping in layers-panel.js).
```

```js
// Anything that changes the active tool must end in emit("tool", id) — the
// handler in main.js tells the engine. Calling state.tool = … directly skips it.
```

## 17. Brush size keys (M5-T09) — VERIFY the steps against Photoshop

```rust
fn step_up(size: f32) -> f32 {
    match size {
        s if s < 10.0 => 1.0,
        s if s < 100.0 => 10.0,
        s if s < 200.0 => 25.0,
        s if s < 300.0 => 50.0,
        _ => 100.0,
    }
}
fn bigger(size: f32) -> f32 { (size + step_up(size)).min(5000.0) }
/// Going down uses the step of the range BELOW: 100 → 90, not 100 → 0.
fn smaller(size: f32) -> f32 { (size - step_up(size - 0.5)).max(1.0) }
```

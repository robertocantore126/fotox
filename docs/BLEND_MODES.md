# Blend modes and compositing rules

Target: match Photoshop's results in its **default** configuration (8/16-bit
RGB, blending on encoded values, "Blend clipped layers as group" on).
Implemented in M2-T02 as WGSL (`fx-render`) with an f64 CPU twin
(`fx-render::reference`). Items marked **VERIFY** must be checked against
Photoshop renders in M7 (PSD import gives us test files); until then this
document is the spec.

## 1. Notation

All values normalised to 0..1, **straight** colour, encoded in the document
profile (not linearised).

* `Cb`, `αb` — backdrop (what is below), `Cs`, `αs` — source (the layer)
* `B(Cb, Cs)` — the blend function of the mode (per channel for separable modes)

Effective source alpha of a pixel layer:

```
αs = content_alpha × mask × opacity × fill
```

(`fill` and `opacity` differ only once layer styles exist; until then they
multiply.)

## 2. General compositing formula (source-over with blending)

```
αo = αs + αb·(1 − αs)
Co·αo = (1 − αb)·αs·Cs  +  (1 − αs)·αb·Cb  +  αs·αb·B(Cb, Cs)
```

With premultiplied accumulators (`cb = Cb·αb`, `cs = Cs·αs`) this is:

```
co = (1 − αb)·cs + (1 − αs)·cb + αs·αb·B(Cb, Cs)
```

`B` always receives straight colours: unpremultiply the backdrop
(`Cb = cb / αb`, `Cb = 0` when `αb = 0`) before calling it.

## 3. Separable modes

| Mode | B(Cb, Cs) |
| --- | --- |
| Normal | Cs |
| Darken | min(Cb, Cs) |
| Multiply | Cb·Cs |
| Color Burn | Cb = 1 → 1; Cs = 0 → 0; else 1 − min(1, (1 − Cb)/Cs) |
| Linear Burn | max(0, Cb + Cs − 1) |
| Lighten | max(Cb, Cs) |
| Screen | Cb + Cs − Cb·Cs |
| Color Dodge | Cb = 0 → 0; Cs = 1 → 1; else min(1, Cb/(1 − Cs)) |
| Linear Dodge (Add) | min(1, Cb + Cs) |
| Overlay | HardLight(Cs = Cb, Cb = Cs) — i.e. Hard Light with arguments swapped |
| Soft Light (Photoshop) | Cs ≤ 0.5: 2·Cb·Cs + Cb²·(1 − 2·Cs); else: 2·Cb·(1 − Cs) + √Cb·(2·Cs − 1) |
| Hard Light | Cs ≤ 0.5: Multiply(Cb, 2·Cs); else Screen(Cb, 2·Cs − 1) |
| Vivid Light | Cs ≤ 0.5: ColorBurn(Cb, 2·Cs); else ColorDodge(Cb, 2·(Cs − 0.5)) |
| Linear Light | clamp(Cb + 2·Cs − 1, 0, 1) |
| Pin Light | Cs ≤ 0.5: min(Cb, 2·Cs); else max(Cb, 2·Cs − 1) |
| Hard Mix | Cb + Cs ≥ 1 → 1, else 0. **VERIFY**: Photoshop changes Hard Mix behaviour when fill < 100 %. |
| Difference | abs(Cb − Cs) |
| Exclusion | Cb + Cs − 2·Cb·Cs |
| Subtract | max(0, Cb − Cs) |
| Divide | Cs = 0 → (Cb = 0 ? 0 : 1); else min(1, Cb/Cs) |

Note: Photoshop's Soft Light is **not** the W3C/PDF formula. Use the one above.

## 4. Non-separable modes

```
Lum(C)  = 0.30·R + 0.59·G + 0.11·B
Sat(C)  = max(R,G,B) − min(R,G,B)
ClipColor, SetLum, SetSat — exactly as in the W3C Compositing and Blending
Level 1 specification, §10.
```

| Mode | B(Cb, Cs) |
| --- | --- |
| Hue | SetLum(SetSat(Cs, Sat(Cb)), Lum(Cb)) |
| Saturation | SetLum(SetSat(Cb, Sat(Cs)), Lum(Cb)) |
| Color | SetLum(Cs, Lum(Cb)) |
| Luminosity | SetLum(Cb, Lum(Cs)) |
| Darker Color | whole colour with the smaller R+G+B sum (Cs on ties) |
| Lighter Color | whole colour with the larger R+G+B sum (Cs on ties) |

## 5. Dissolve

Pixel is drawn with alpha 1 if `hash(doc_x, doc_y, layer_id) < αs`, else not
drawn. The hash uses **document** coordinates so the pattern does not shimmer
while panning. At mip levels > 0: evaluate with the level's pixel centres
(pattern changes with zoom, as in Photoshop).

## 6. Layer stack rules

* Order: bottom → top. The document backdrop starts **transparent**.
* **Hidden** layers and layers with `αs = 0` everywhere in a tile contribute
  nothing (skip them).
* **Groups, not pass-through:** composite the children in isolation onto a
  transparent backdrop, then blend the result onto the real backdrop with the
  group's mode, opacity, fill and mask.
* **Groups, pass-through:** composite the children directly onto the real
  backdrop → `R`. If the group has opacity < 1 or a mask:
  `result = lerp(backdrop, R, opacity × mask)`.
* **Adjustment layers:** `Cs = f(Cb)` with `f` the adjustment, `αs = mask ×
  opacity × fill`, then the normal formula with the layer's blend mode.
  Inside a pass-through group, `Cb` is the running backdrop; inside an
  isolated group, it is the group's own composite so far.
* **Solid fill layers:** `Cs = the colour`, content alpha 1.
* **Clipping masks:** a base layer followed by one or more `clipped` layers
  above it form a clipping group. Composite, in isolation: the base (Normal
  mode, its own opacity/fill/mask), then each clipped layer with its mode,
  with its alpha multiplied by the **base content alpha at that pixel**
  (content alpha × base mask). Blend the result onto the backdrop with the
  base's blend mode. **VERIFY** against Photoshop.
* **Knockout, blend-if, layer styles:** not supported yet (M6+).

## 7. Precision and tests (M2-T02)

* Display path (Rgba16Float): GPU vs CPU reference, max abs error ≤ 1/1024
  per channel on 10 000 random pixel pairs per mode, including edge values
  (0, 1, 0.5, αs/αb ∈ {0, 1}).
* Commit path (f32 compute → u16): exact match with the CPU reference after
  rounding.

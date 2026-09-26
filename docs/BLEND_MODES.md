# Blend modes and compositing rules

Target: match Photoshop's results in its **default** configuration (8/16-bit
RGB, blending on encoded values, "Blend clipped layers as group" on).
Implemented in `fx-render/src/gpu/composite.wgsl` with an f64 CPU twin
(`fx-render::reference`). Items marked **VERIFY** must be checked against
Photoshop renders in M14 (PSD import gives us test files); until then this
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

## 2. General compositing formula

First mix the source with the blend result, weighted by backdrop alpha:

```
Cs' = (1 − αb)·Cs + αb·B(Cb, Cs)
```

Then one of two Porter-Duff operators (premultiplied `cb = Cb·αb`):

```
source-over (normal layers):   co = αs·Cs' + (1 − αs)·cb        αo = αs + αb·(1 − αs)
source-atop (clipped layers,   co = αs·αb·Cs' + (1 − αs)·cb     αo = αb
             adjustment layers)
```

Source-over expands to the familiar
`Co·αo = (1 − αb)·αs·Cs + (1 − αs)·αb·Cb + αs·αb·B(Cb, Cs)`.
`B` always receives straight colours: unpremultiply the backdrop
(`Cb = cb / αb`, `Cb = 0` when `αb = 0`) before calling it.

Implemented in `fx-render/src/blend.rs::composite` and `composite.wgsl::composite`.

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
  opacity × fill`, composited **source-atop** with the layer's blend mode:
  an adjustment never creates pixels where the backdrop is transparent.
  Inside a pass-through group, `Cb` is the running backdrop; inside an
  isolated group, it is the group's own composite so far.
* **Solid fill layers:** `Cs = the colour`, content alpha = its alpha.
* **Clipping masks:** a base layer followed by one or more `clipped` layers
  above it form a clipping group, composited **in isolation**:
  1. the base with **Normal** mode, its **fill** and its mask (not its opacity);
  2. each clipped layer with its own mode/opacity/mask, **source-atop**
     (so it only shows where the base has alpha);
  3. the result blended onto the backdrop with the **base's blend mode and
     opacity** (Photoshop: "clipped layers take on the opacity and mode of
     the base").
  A hidden base hides the whole group. A base that is a group acts as an
  isolated group. **VERIFY:** an adjustment layer as base is treated as "no
  base" (clipped layers composite normally); a clipped pass-through group is
  treated as isolated Normal.
* **Layer offsets** at mip levels > 0 are rounded to whole pixels of that
  level (exact at level 0): up to half a level pixel of shift in zoomed-out
  previews.
* **Group nesting** is limited to 11 levels (clipping groups count).
* **Knockout, blend-if, layer styles:** not supported yet (M6+).

## 7. Precision and tests

* Display path (Rgba16Float inputs and outputs): GPU vs CPU reference on
  noisy test stacks. Measured (lavapipe, 2026-09-24): max error ≈ 0.001 for
  continuous modes; modes with discontinuities (Color Burn/Dodge, Vivid
  Light, Hard Mix, Darker/Lighter Color, Dissolve, Hue/Saturation) flip a
  few pixels where f16 rounding crosses a threshold — < 0.01 % of channels.
  Test threshold: < 0.2 % of channels off by more than 2/1024.
* Commit path (f32 compute → u16): exact match with the CPU reference after
  rounding.

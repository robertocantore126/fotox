# M2 — Levels and Curves dialogs edit adjustment layers live

Agent: Claude  ·  Branch: `task/M2-levels-curves-live`  ·  Status: done (open point from `STATUS-2026-09-25.md`)

## What was done

* **Curves editor** (`ui/js/dialogs.js`, `curveField`): a real widget instead of the mock canvas. Click to add a point, drag to move it (it stays between its neighbours), drag a middle point out of the box to remove it; *Reset* goes back to the straight line; *Input* / *Output* show the active point (0–255). The drawn curve is the engine's: the natural cubic spline of `fx-render/src/adjust.rs` ported to JS (`curveSpline`), so what you see is what is applied.
* **Levels dialog**: five sliders (input black / gamma ×100 / input white, output black / white) replace the mock number boxes. Input white is kept at least 2 above input black.
* **Channel menu**: both dialogs keep one setting per channel (RGB composite, Red, Green, Blue — the engine's order); switching the menu shows that channel's setting.
* **Live editing**: double-clicking a Levels or Curves layer's icon opens its dialog and every change is sent as `set_adjustment` (merged into one History step, like the other dialogs); Cancel restores the original, *Preview* off shows the original while the dialog is open (now also for Brightness/Contrast, Hue/Saturation, Exposure).
* **New adjustment layer opens its dialog**, like Photoshop (not for Invert, which has no settings).
* Dialog engine: live dialogs get `dialog.set(values)` to write values back into their fields; menus and buttons inside a live dialog no longer show the mock toast; typing in a slider's number box moves the slider.

## Not done

* The Levels / Curves *Preset* menus and the Curves *Smooth* / *Linear* buttons stay mocks; the histogram is still the mock one (a real histogram needs an engine message).
* Photoshop's exact Levels order (channel then composite) and Curves interpolation are still marked VERIFY in `adjust.rs` (M7).

## Verification

```text
node --check / node ui/tools/check-data.mjs: ok
Browser preview (live dialog opened from the console with an onChange hook):
  click+drag on the curve adds (63, 103), the Input/Output boxes follow, onChange gets the 4 points;
  Channel ▸ Red: dialog.set shows Red's curve; Reset → [[0,0],[1,1]]; no toast, no console error.
  Levels: typing 300 in "Input White" clamps to 255 and moves the slider; onChange gets all five values.
JS spline through (0,0) (0.25,0.4) (0.6,0.55) (1,1) passes through every point.
```

Not tested in the app (Rob): double-click a Levels / Curves layer from B3, drag the curve, switch channels, Cancel.

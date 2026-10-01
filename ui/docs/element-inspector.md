# Element inspector

Hold Ctrl and right-click a UI element in the desktop app. A blue outline marks
the selected control. **Copy for AI** copies its label, selector, JavaScript
creation file/function/line, ancestor identifiers and matching CSS rules. Paste
the reference into your AI chat and fill in the requested improvement. Close
with Escape; you can inspect another element while the card is open.

Icons inside controls resolve to the owning control. Menu actions and dialog
IDs are included. The native viewport identifies its UI surface and engine
entry paths; pixels and document layers are not DOM elements. Creation frames
are provenance, not a claim that the behavior handler lives on that line.
Nodes created outside the DOM helper report their nearest recorded ancestor.
Source lines belong to the currently loaded version; restart after edits.

If automatic copying is unavailable in CEF, select the text and press Ctrl+C.
Ordinary right-click retains its normal behavior when the inspector is closed.
The inspector disables direct engine pointer input while its card is open.

The DOM helper retains development creation traces in a WeakMap, released with
removed nodes; it formats only the selected trace. To turn off trace capture
for profiling, set localStorage["fotox.inspector.sources"] = "off" in developer
tools and restart. Remove that key and restart to restore source locations.

The native input routing change requires rebuilding/bundling the desktop app.
Debug builds read UI files from ./ui; release builds embedding UI need a rebuild.

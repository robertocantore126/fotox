// Fotox — colour management in native mode (M4-T03/T04).
//
// The engine sends the CMYK profiles it found (`cmyk_profiles`) and the proof
// state of a document (`proof_state`); this module keeps them, puts the check
// marks on View ▸ Proof Colors / Gamut Warning, and runs the Assign Profile,
// Convert to Profile and Proof Setup dialogs.

import { openDialog } from "../dialogs.js";
import { dialogDef } from "../data/dialogs.js";
import { setFlag } from "../state.js";
import { status, toast } from "../tooltip.js";
import * as bridge from "./bridge.js";
import { UI, ENGINE } from "./protocol.js";
import { activeDocument } from "./documents.js";

let cmyk = []; // [{ name, path }]

// Dialog names ↔ the engine's values.
const PROFILES = { "sRGB IEC61966-2.1": "Srgb", "Adobe RGB (1998)": "AdobeRgb1998", "Display P3": "DisplayP3", "ProPhoto RGB": "ProPhotoRgb" };
const INTENTS = { Perceptual: "perceptual", "Relative Colorimetric": "relative_colorimetric", Saturation: "saturation", "Absolute Colorimetric": "absolute_colorimetric" };

/** Start listening to the engine. Call once, in native mode. */
export function initColor() {
  bridge.on(ENGINE.CMYK_PROFILES, ({ profiles }) => { cmyk = profiles || []; });
  bridge.on(ENGINE.PROOF_STATE, ({ proof_colors, gamut_warning, profile }) => {
    setFlag("view:proof-colors", proof_colors);
    setFlag("view:gamut-warning", gamut_warning);
    if (proof_colors && profile) status(`Proof: ${profile}`);
  });
}

/** The CMYK profiles the engine found. */
export function cmykProfiles() {
  return cmyk;
}

/** True for the dialogs this module runs. */
export function isColorDialog(id) {
  return id === "assign-profile" || id === "convert-profile" || id === "proof-setup";
}

/** Open Assign Profile, Convert to Profile or Proof Setup. */
export function openColorDialog(id) {
  const doc = activeDocument();
  if (doc == null) { toast("Open a document first"); return; }
  const command = (command) => bridge.send({ type: UI.COMMAND, doc, command });
  if (id === "assign-profile") {
    openDialog(id, { onOk: (v) => command({ op: "assign_profile", profile: PROFILES[v["Profile:"]] || "Srgb" }) });
  } else if (id === "convert-profile") {
    openDialog(id, {
      onOk: (v) => command({
        op: "convert_profile",
        profile: PROFILES[v["Destination:"]] || "Srgb",
        intent: INTENTS[v["Intent:"]] || "relative_colorimetric",
        bpc: !!v["Use Black Point Compensation"],
      }),
    });
  } else if (id === "proof-setup") {
    if (!cmyk.length) { toast("No CMYK profile found: add one to %APPDATA%\\Fotox\\profiles"); return; }
    const names = cmyk.map((p) => p.name);
    const fields = dialogDef(id).fields.map((f) => (f.label === "Device to Simulate:" ? { ...f, options: names, value: names[0] } : f));
    openDialog(id, {
      fields,
      onOk: (v) => {
        const profile = cmyk.find((p) => p.name === v["Device to Simulate:"]) || cmyk[0];
        bridge.send({
          type: UI.PROOF_SETUP, doc, path: profile.path,
          intent: INTENTS[v["Rendering Intent:"]] || "relative_colorimetric",
          bpc: !!v["Black Point Compensation"],
          simulate_paper: !!v["Simulate Paper Color"],
        });
      },
    });
  }
}

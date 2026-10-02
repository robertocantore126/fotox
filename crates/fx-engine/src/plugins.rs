//! Brush plugins in the engine (D-096): load them at start, reload them when
//! their `.wasm` changes, describe their tools to the UI, and turn a tool's
//! option bar into the plugin's params.
//!
//! The folder is `%APPDATA%\Fotox\plugins` (`FOTOX_PLUGINS` overrides it);
//! `cargo xtask plugins --watch` builds into it on every save.

use std::path::PathBuf;

use crossbeam_channel::Sender;
use fx_plugin::{Change, Manifest, PaintColor};

use super::engine::Internal;
use crate::tools::ToolSettings;

/// The UI tool id prefix of a plugin tool.
pub const TOOL_PREFIX: &str = "plugin:";

/// The plugin folder, if there is one to look in.
pub fn dir() -> Option<PathBuf> {
	if let Some(dir) = std::env::var_os("FOTOX_PLUGINS") {
		return Some(PathBuf::from(dir));
	}
	std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Fotox").join("plugins"))
}

/// Load the plugins and watch the folder; changes arrive as
/// `Internal::PluginsChanged`.
pub(crate) fn start(internal: Sender<Internal>) {
	let Some(dir) = dir() else { return };
	for plugin in fx_plugin::load_dir(&dir) {
		fx_core::stroke::register_plugin_label(plugin.key, &plugin.manifest.name);
	}
	fx_plugin::watch(dir, move |changes| {
		let _ = internal.send(Internal::PluginsChanged(changes));
	});
}

/// Keep History labels in step after a change.
pub(crate) fn note(changes: &[Change]) {
	for change in changes {
		if let Change::Loaded { id, name } = change {
			fx_core::stroke::register_plugin_label(fx_plugin::key_of(id), name);
		}
	}
}

/// What the toast says about a change.
pub(crate) fn describe(change: &Change) -> String {
	match change {
		Change::Loaded { name, .. } => format!("Plugin loaded: {name}"),
		Change::Unloaded { name, .. } => format!("Plugin removed: {name}"),
		Change::Failed(message) => message.clone(),
	}
}

/// The loaded plugins' tools, for `EngineToUi::Plugins`.
pub(crate) fn tools() -> Vec<fx_protocol::PluginTool> {
	fx_plugin::plugins()
		.iter()
		.map(|p| fx_protocol::PluginTool {
			id: format!("{TOOL_PREFIX}{}", p.manifest.id),
			name: p.manifest.name.clone(),
			slot: p.manifest.slot.clone(),
			icon: p.manifest.icon.clone(),
			options: p.manifest.options.clone(),
		})
		.collect()
}

/// The plugin behind UI tool id `tool`.
pub fn plugin_of(tool: &str) -> Option<std::sync::Arc<fx_plugin::Plugin>> {
	fx_plugin::by_id(tool.strip_prefix(TOOL_PREFIX)?)
}

/// Whether the plugin paints with the background swatch.
pub fn uses_background(manifest: &Manifest) -> bool {
	manifest.color == PaintColor::Background
}

/// The brush with the plugin's overrides (`manifest.brush`): profile,
/// accumulation, spacing.
pub fn brush(manifest: &Manifest, mut brush: fx_core::stroke::BrushParams, settings: &ToolSettings, tool: &str) -> fx_core::stroke::BrushParams {
	fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
		serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
	}
	let o = &manifest.brush;
	if let Some(profile) = o.profile.as_deref().and_then(parse) {
		brush.profile = profile;
		brush.tip = 0;
	}
	if let Some(accumulate) = o.accumulate.as_deref().and_then(parse) {
		brush.accumulate = accumulate;
	}
	if let Some(spacing) = o.spacing
		&& settings.number(tool, "Spacing").is_none()
	{
		brush.spacing = spacing.clamp(0.01, 10.0);
	}
	if let Some(key) = &o.feather_from {
		let default = field(manifest, key)
			.and_then(|f| f.get("value"))
			.and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
			.unwrap_or(0.0);
		let feather = settings.number(tool, key).unwrap_or(default).clamp(0.0, 2500.0) as f32;
		let core = brush.diameter;
		brush.diameter = (core + 2.0 * feather).min(5000.0);
		brush.hardness = (core / brush.diameter).clamp(0.0, 1.0);
	}
	brush
}

/// The option-bar field whose key (text without the colon, or `key`) is `key`.
fn field<'a>(manifest: &'a Manifest, key: &str) -> Option<&'a serde_json::Value> {
	manifest.options.as_array()?.iter().find(|f| {
		f.get("key")
			.and_then(|k| k.as_str())
			.or_else(|| f.get("text").and_then(|t| t.as_str()).map(|t| t.trim_end_matches(':')))
			== Some(key)
	})
}

/// A drop-down value as the index of its entry (or a number it spells).
fn choice(field: Option<&serde_json::Value>, value: &str) -> f32 {
	field
		.and_then(|f| f.get("options")?.as_array()?.iter().position(|o| o.as_str() == Some(value)))
		.map(|i| i as f32)
		.unwrap_or_else(|| value.parse().unwrap_or(0.0))
}

/// The plugin's params from tool `tool`'s option bar: numbers as shown,
/// toggles 0/1, drop-downs the entry's index; a field the bar has not sent
/// yet takes the manifest's default.
pub fn params(manifest: &Manifest, settings: &ToolSettings, tool: &str) -> [f32; 16] {
	let mut out = [0.0f32; 16];
	for (slot, key) in out.iter_mut().zip(&manifest.params) {
		let field = field(manifest, key);
		*slot = if let Some(v) = settings.number(tool, key) {
			v as f32
		} else if let Some(b) = settings.bool(tool, key) {
			f32::from(u8::from(b))
		} else if let Some(s) = settings.string(tool, key) {
			choice(field, &s)
		} else {
			match field.and_then(|f| f.get("value").or_else(|| f.get("on"))) {
				Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) as f32,
				Some(serde_json::Value::Bool(b)) => f32::from(u8::from(*b)),
				Some(serde_json::Value::String(s)) => s.parse().unwrap_or_else(|_| choice(field, s)),
				_ => 0.0,
			}
		};
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn manifest() -> Manifest {
		serde_json::from_value(serde_json::json!({
			"id": "f",
			"name": "F",
			"brush": { "profile": "feather", "accumulate": "max", "spacing": 0.2, "feather_from": "Feather" },
			"options": [
				{ "type": "num", "text": "Feather:", "value": "300" },
				{ "type": "select", "text": "Tone:", "options": ["A", "B", "C"], "value": "B" }
			],
			"params": ["Tone", "Feather"]
		}))
		.unwrap()
	}

	#[test]
	fn a_feather_widens_the_tip_around_a_solid_core() {
		let mut settings = ToolSettings::default();
		let base = fx_core::stroke::BrushParams {
			diameter: 100.0,
			tip: 7,
			..Default::default()
		};
		// Not sent yet: the manifest's default feather.
		let brush = super::brush(&manifest(), base, &settings, "plugin:f");
		assert_eq!(brush.diameter, 700.0);
		assert!((brush.hardness - 100.0 / 700.0).abs() < 1e-6);
		assert_eq!(brush.profile, fx_core::stroke::TipProfile::Feather);
		assert_eq!(brush.accumulate, fx_core::stroke::Accumulate::Max);
		assert_eq!(brush.spacing, 0.2);
		assert_eq!(brush.tip, 0, "a profile forces the round tip");
		settings.options.insert("plugin:f".into(), serde_json::json!({ "Feather": 50, "Tone": "C" }));
		assert_eq!(super::brush(&manifest(), base, &settings, "plugin:f").diameter, 200.0);
		assert_eq!(params(&manifest(), &settings, "plugin:f")[..2], [2.0, 50.0]);
	}
}

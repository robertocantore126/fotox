//! The engine side of M13: the AI jobs and the model manager.
//!
//! * T01: `ai:download {id}` (progress, Escape / `ai:cancel` cancels),
//!   `ai:delete {id}`, `ai:status`; the Preferences page reads `_ai` in the
//!   preferences message.
//! * T02: `ai:subject` (Select ▸ Subject) and `ai:remove-bg` (Remove
//!   Background).
//! * T04: the Object Selection tool's requests (`AiRequest::Object`), with
//!   the image embedding cached per document content.
//! * T06: `ai:generative-fill {prompt}` (Generative Fill) and the Crop tool's
//!   Generative Expand (`AiRequest::Expand`), through ComfyUI; `comfy:test`.
//!
//! Every model run is a job (recipe R2): the working image is read on the
//! engine thread (bounded, `ai::working_composite`), the model runs on a
//! worker, and its output comes back as an ordinary command (`SelectBy`,
//! `MaskFromModel`, `GenerativeLayer`), one history step. The document is
//! busy meanwhile. Nothing is faked without a model or ComfyUI (D-092).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fx_core::select_ops::SelectOp;
use fx_core::{Command, LayerRef, SelectMode};
use fx_protocol::{DocId, EngineToUi};
use serde_json::{Value, json};

use super::{Engine, Internal};
use crate::ai;
use crate::tools::AiRequest;

/// The working resolution the models read (long side).
const WORKING: u32 = 1024;

#[derive(Default)]
pub(super) struct State {
	/// The running AI job's task and cancel flag.
	cancel: Option<(u64, Arc<AtomicBool>)>,
	/// EfficientSAM's embedding: document, content generation, embedding.
	embedding: Option<(DocId, u64, Arc<ai::Embedding>)>,
}

/// What a finished AI job hands back to the engine thread.
pub(crate) struct AiDone {
	task: u64,
	doc: Option<DocId>,
	result: Result<AiResult, String>,
}

pub(crate) enum AiResult {
	Command(Command),
	/// An Object Selection whose embedding was computed on the way.
	Object {
		generation: u64,
		embedding: Arc<ai::Embedding>,
		command: Command,
	},
	/// A download or a connection test finished.
	Message(String),
}

type Progress<'a> = &'a dyn Fn(f32) -> bool;

/// The `_ai` block of the preferences message (the Preferences ▸ AI page).
pub(super) fn ai_info() -> Value {
	let models: Vec<Value> = fx_ai::models::ALL
		.iter()
		.map(|m| {
			json!({
				"id": m.id, "name": m.name, "licence": m.licence, "purpose": m.purpose,
				"mb": m.bytes() / 1_000_000, "installed": m.installed(),
			})
		})
		.collect();
	json!({
		"runtime": fx_ai::runtime::find_library().map(|p| p.display().to_string()),
		"folder": fx_ai::models::models_dir().display().to_string(),
		"models": models,
	})
}

fn mode_of(args: &Value) -> SelectMode {
	match args.get("mode").and_then(Value::as_str) {
		Some("add") => SelectMode::Add,
		Some("subtract") => SelectMode::Subtract,
		Some("intersect") => SelectMode::Intersect,
		_ => SelectMode::Replace,
	}
}

impl Engine {
	pub(super) fn m13_action(&mut self, id: &str, args: &Value) -> bool {
		let model = || args.get("id").and_then(Value::as_str).and_then(fx_ai::models::by_id);
		match id {
			"ai:status" => {
				let runtime = match fx_ai::runtime::find_library() {
					Some(p) => format!("ONNX Runtime: {}", p.display()),
					None => "ONNX Runtime missing (set FOTOX_ORT_DYLIB or put onnxruntime.dll next to Fotox)".into(),
				};
				let models: Vec<String> = fx_ai::models::ALL
					.iter()
					.map(|m| {
						format!(
							"{}: {} ({} MB)",
							m.name,
							if m.installed() { "installed" } else { "not installed" },
							m.bytes() / 1_000_000
						)
					})
					.collect();
				self.to_ui(&EngineToUi::Toast {
					text: format!("{runtime} · {} · folder {}", models.join(" · "), fx_ai::models::models_dir().display()),
				});
				self.send_prefs();
			}
			"ai:download" => {
				let Some(spec) = model() else { return true };
				let label = format!("Downloading {} ({} MB)", spec.name, spec.bytes() / 1_000_000);
				self.start_ai(None, label, move |progress| {
					fx_ai::models::download(&spec, &mut |done, total| progress(done as f32 / total.max(1) as f32)).map_err(|e| e.to_string())?;
					Ok(AiResult::Message(format!("{} installed", spec.name)))
				});
			}
			"ai:delete" => {
				let Some(spec) = model() else { return true };
				let text = match fx_ai::models::delete(&spec) {
					Ok(()) => format!("{} deleted", spec.name),
					Err(error) => error.to_string(),
				};
				self.to_ui(&EngineToUi::Toast { text });
				self.send_prefs();
			}
			"ai:cancel" => self.cancel_ai(),
			// Select ▸ Subject (T02); `mode` from an option bar's button.
			"ai:subject" | "sel:subject" => {
				let mode = mode_of(args);
				self.run_subject(move |mask| Command::SelectBy {
					select: SelectOp::Model(mask),
					mode,
				});
			}
			// Remove Background (T02): the subject as the active layer's mask.
			"ai:remove-bg" => {
				self.run_subject(|mask| Command::MaskFromModel { layer: LayerRef::Active, mask });
			}
			// Generative Fill (T06): the prompt dialog's value.
			"ai:generative-fill" => {
				let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("").trim().to_owned();
				self.generative_fill(prompt);
			}
			"comfy:test" => {
				let address = args
					.get("address")
					.and_then(Value::as_str)
					.filter(|a| !a.trim().is_empty())
					.map(str::to_owned)
					.or_else(|| self.prefs.string("comfy_address"));
				self.start_ai(None, "Testing ComfyUI".into(), move |_| {
					let address = fx_ai::comfy::find(address.as_deref()).map_err(|e| e.to_string())?;
					let version = fx_ai::comfy::ping(&address).map_err(|e| e.to_string())?;
					let list = fx_ai::comfy::checkpoints(&address).unwrap_or_default();
					let pick = fx_ai::comfy::pick_checkpoint(&list).unwrap_or_else(|| "none".into());
					Ok(AiResult::Message(format!(
						"{version} at {address} · {} checkpoints (default: {pick})",
						list.len()
					)))
				});
			}
			_ => return false,
		}
		true
	}

	/// Escape while an AI job runs cancels it (before the tool sees the key).
	pub(super) fn ai_escape(&mut self) -> bool {
		if self.m13.cancel.is_some() {
			self.cancel_ai();
			return true;
		}
		false
	}

	fn cancel_ai(&mut self) {
		if let Some((_, flag)) = &self.m13.cancel {
			flag.store(true, Ordering::Relaxed);
			self.to_ui(&EngineToUi::Toast { text: "Cancelling…".into() });
		}
	}

	/// Whether `spec` can run now; if not, say what is missing (D-092).
	fn ai_ready(&self, spec: &fx_ai::models::ModelSpec, what: &str) -> bool {
		if !fx_ai::runtime::available() {
			self.to_ui(&EngineToUi::Error {
				text: format!("{what} needs ONNX Runtime: put onnxruntime.dll next to Fotox or set FOTOX_ORT_DYLIB (Preferences ▸ AI)"),
			});
			return false;
		}
		if !spec.installed() {
			self.to_ui(&EngineToUi::Error {
				text: format!(
					"{what} needs the {} model ({} MB, {}): download it in Edit ▸ Preferences ▸ AI",
					spec.name,
					spec.bytes() / 1_000_000,
					spec.licence
				),
			});
			return false;
		}
		true
	}

	/// Run `work` on a worker thread as a job with progress and cancel;
	/// `doc` is busy meanwhile.
	fn start_ai(&mut self, doc: Option<DocId>, label: String, work: impl FnOnce(Progress<'_>) -> Result<AiResult, String> + Send + 'static) {
		if self.m13.cancel.is_some() {
			self.to_ui(&EngineToUi::Toast {
				text: "Another AI job is running (Esc cancels it)".into(),
			});
			return;
		}
		if let Some(id) = doc {
			let Some(open) = self.docs.get_mut(id) else { return };
			if let Some(job) = &open.busy {
				let text = format!("Wait until {job} is finished");
				self.to_ui(&EngineToUi::Toast { text });
				return;
			}
			open.busy = Some(label.clone());
		}
		self.next_task += 1;
		let task = self.next_task;
		let cancel = Arc::new(AtomicBool::new(false));
		self.m13.cancel = Some((task, cancel.clone()));
		self.to_ui(&EngineToUi::Progress {
			task,
			label: label.clone(),
			fraction: 0.0,
		});
		let internal = self.internal.clone();
		let spawned = std::thread::Builder::new().name(format!("ai-{task}")).spawn(move || {
			let progress_internal = internal.clone();
			let last = std::sync::Mutex::new(-1.0f32);
			let progress = move |fraction: f32| {
				let mut last = last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
				if fraction - *last >= 0.01 {
					*last = fraction;
					let _ = progress_internal.send(Internal::Progress {
						task,
						label: label.clone(),
						fraction: fraction.clamp(0.0, 1.0),
					});
				}
				!cancel.load(Ordering::Relaxed)
			};
			let result = work(&progress);
			let _ = internal.send(Internal::Ai(Box::new(AiDone { task, doc, result })));
		});
		if let Err(error) = spawned {
			self.m13.cancel = None;
			if let Some(open) = doc.and_then(|id| self.docs.get_mut(id)) {
				open.busy = None;
			}
			self.to_ui(&EngineToUi::ProgressDone { task });
			self.to_ui(&EngineToUi::Error {
				text: format!("Cannot start the job: {error}"),
			});
		}
	}

	/// An AI job finished (`Internal::Ai`).
	pub(super) fn ai_done(&mut self, done: AiDone) {
		self.to_ui(&EngineToUi::ProgressDone { task: done.task });
		if self.m13.cancel.as_ref().is_some_and(|(t, _)| *t == done.task) {
			self.m13.cancel = None;
		}
		if let Some(open) = done.doc.and_then(|id| self.docs.get_mut(id)) {
			open.busy = None;
		}
		match done.result {
			Ok(AiResult::Command(command)) => {
				if let Some(doc) = done.doc {
					self.command(doc, command);
				}
			}
			Ok(AiResult::Object {
				generation,
				embedding,
				command,
			}) => {
				if let Some(doc) = done.doc {
					self.m13.embedding = Some((doc, generation, embedding));
					self.command(doc, command);
				}
			}
			Ok(AiResult::Message(text)) => {
				self.to_ui(&EngineToUi::Toast { text });
				self.send_prefs();
			}
			Err(text) if text == fx_ai::AiError::Cancelled.to_string() => {
				self.to_ui(&EngineToUi::Toast { text: "Cancelled".into() });
				self.send_prefs();
			}
			Err(text) => {
				self.to_ui(&EngineToUi::Error { text });
				self.send_prefs();
			}
		}
	}

	/// BiRefNet on the active document, its mask turned into a command.
	fn run_subject(&mut self, then: impl FnOnce(fx_core::select_ops::ModelMask) -> Command + Send + 'static) {
		let Some(doc_id) = self.docs.active_id() else { return };
		if !self.ai_ready(&fx_ai::models::BIREFNET, "Select Subject") {
			return;
		}
		self.end_stroke();
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		if open.busy.is_some() {
			self.to_ui(&EngineToUi::Toast {
				text: "Wait until the running job is finished".into(),
			});
			return;
		}
		// FAST: read on the engine thread (bounded by 1024², mips served inline).
		let work = match ai::working_composite(&mut open.doc, &store, WORKING) {
			Ok(w) => w,
			Err(text) => {
				self.to_ui(&EngineToUi::Error { text });
				return;
			}
		};
		self.start_ai(Some(doc_id), "Select Subject".into(), move |progress| {
			progress(0.1);
			let mask = ai::subject_mask(&work).map_err(|e| e.to_string())?;
			if !progress(1.0) {
				return Err(fx_ai::AiError::Cancelled.to_string());
			}
			Ok(AiResult::Command(then(mask)))
		});
	}

	/// A request a tool made (the Object Selection tool, Generative Expand).
	pub(super) fn ai_request(&mut self, doc_id: DocId, request: AiRequest) {
		match request {
			AiRequest::Object { boxed, points, mode } => self.object_select(doc_id, boxed, points, mode),
			AiRequest::Expand { old, prompt } => self.generative_expand(doc_id, old, prompt),
		}
	}

	/// The Object Selection tool (T04): EfficientSAM's decoder on the cached
	/// embedding, or the encoder first when the content changed.
	fn object_select(&mut self, doc_id: DocId, boxed: Option<[f64; 4]>, points: Vec<((f64, f64), bool)>, mode: SelectMode) {
		if !self.ai_ready(&fx_ai::models::EFFICIENT_SAM, "The Object Selection tool") {
			return;
		}
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let generation = open.generation;
		let cached = match &self.m13.embedding {
			Some((d, g, e)) if *d == doc_id && *g == generation => Some(e.clone()),
			_ => None,
		};
		let make = move |mask| Command::SelectBy {
			select: SelectOp::Model(mask),
			mode,
		};
		match cached {
			Some(embedding) => self.start_ai(Some(doc_id), "Object Selection".into(), move |_| {
				let mask = ai::sam_mask(&embedding, boxed, &points).map_err(|e| e.to_string())?;
				Ok(AiResult::Command(make(mask)))
			}),
			None => {
				let work = match ai::working_composite(&mut open.doc, &store, WORKING) {
					Ok(w) => w,
					Err(text) => {
						self.to_ui(&EngineToUi::Error { text });
						return;
					}
				};
				self.start_ai(Some(doc_id), "Object Selection".into(), move |progress| {
					progress(0.1);
					let embedding = Arc::new(ai::sam_embedding(&work).map_err(|e| e.to_string())?);
					if !progress(0.8) {
						return Err(fx_ai::AiError::Cancelled.to_string());
					}
					let mask = ai::sam_mask(&embedding, boxed, &points).map_err(|e| e.to_string())?;
					Ok(AiResult::Object {
						generation,
						embedding,
						command: make(mask),
					})
				});
			}
		}
	}

	/// The ComfyUI settings from the preferences (T06); the address is found
	/// on the worker.
	fn comfy_job(&self, prompt: &str, expand: bool) -> (Option<String>, fx_ai::comfy::Job) {
		let p = &self.prefs;
		let workflow_file = p.string(if expand { "comfy_workflow_expand" } else { "comfy_workflow_fill" });
		let workflow = workflow_file
			.filter(|f| !f.trim().is_empty())
			.and_then(|f| match std::fs::read_to_string(&f) {
				Ok(text) => Some(text),
				Err(error) => {
					tracing::warn!("workflow {f}: {error}; using the bundled one");
					None
				}
			})
			.unwrap_or_else(|| {
				if expand {
					fx_ai::comfy::OUTPAINT.to_owned()
				} else {
					fx_ai::comfy::INPAINT.to_owned()
				}
			});
		let seed = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_or(0, |d| d.as_nanos() as u64 & 0xffff_ffff_ffff);
		(
			p.string("comfy_address").filter(|a| !a.trim().is_empty()),
			fx_ai::comfy::Job {
				address: String::new(),
				checkpoint: p.string("comfy_checkpoint").unwrap_or_default(),
				prompt: prompt.to_owned(),
				negative: p
					.string("comfy_negative")
					.unwrap_or_else(|| "blurry, low quality, watermark, text, frame, border".into()),
				seed,
				count: p.number("comfy_variations").unwrap_or(3.0).clamp(1.0, 8.0) as u32,
				steps: p.number("comfy_steps").unwrap_or(25.0).clamp(1.0, 150.0) as u32,
				cfg: p.number("comfy_cfg").unwrap_or(6.0).clamp(1.0, 30.0) as f32,
				workflow,
			},
		)
	}

	/// Send `input` to ComfyUI on a worker; the images become a
	/// `GenerativeLayer`.
	fn run_comfy(&mut self, doc_id: DocId, label: &str, name: String, input: ai::GenInput, expand: bool, prompt: &str) {
		let (preferred, mut job) = self.comfy_job(prompt, expand);
		self.start_ai(Some(doc_id), label.into(), move |progress| {
			job.address = fx_ai::comfy::find(preferred.as_deref()).map_err(|e| e.to_string())?;
			if job.checkpoint.is_empty() {
				let list = fx_ai::comfy::checkpoints(&job.address).map_err(|e| e.to_string())?;
				job.checkpoint = fx_ai::comfy::pick_checkpoint(&list)
					.ok_or_else(|| "ComfyUI has no checkpoint to use (put one in its models/checkpoints folder)".to_owned())?;
			}
			let images = fx_ai::comfy::run(&job, &input.image, &mut |f| progress(f)).map_err(|e| e.to_string())?;
			let variations = images
				.into_iter()
				.map(|i| fx_core::command::m13::Variation {
					width: i.width,
					height: i.height,
					rgba: i.rgba,
				})
				.collect();
			Ok(AiResult::Command(Command::GenerativeLayer {
				name,
				rect: input.rect,
				variations,
				mask: input.mask,
			}))
		});
	}

	/// Edit ▸ Generative Fill (T06): the selection and the area around it.
	fn generative_fill(&mut self, prompt: String) {
		let Some(doc_id) = self.docs.active_id() else { return };
		self.end_stroke();
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let canvas = (open.doc.width, open.doc.height);
		let Some(selection) = open.doc.selection.clone() else {
			self.to_ui(&EngineToUi::Toast {
				text: "Generative Fill needs a selection".into(),
			});
			return;
		};
		let Some((x0, y0, x1, y1)) = selection.canvas_bounds(canvas) else {
			self.to_ui(&EngineToUi::Toast {
				text: "The selection is empty".into(),
			});
			return;
		};
		// The area around the selection gives the model its context.
		// VERIFY: Photoshop's context margin.
		let (x0, y0, x1, y1) = (i64::from(x0), i64::from(y0), i64::from(x1), i64::from(y1));
		let margin = ((x1 - x0).max(y1 - y0) / 4).max(32);
		let rect = (
			(x0 - margin).max(0),
			(y0 - margin).max(0),
			(x1 + margin).min(i64::from(canvas.0)),
			(y1 + margin).min(i64::from(canvas.1)),
		);
		let input = {
			let mut reader = ai::CoverageReader::new(&selection, &store, canvas);
			ai::generative_input(&mut open.doc, &store, rect, WORKING, &mut |x, y| reader.at(x, y), false)
		};
		let input = match input {
			Ok(i) => i,
			Err(text) => {
				self.to_ui(&EngineToUi::Error { text });
				return;
			}
		};
		let name = if prompt.is_empty() {
			"Generative Fill".to_owned()
		} else {
			prompt.chars().take(60).collect()
		};
		self.run_comfy(doc_id, "Generative Fill", name, input, false, &prompt);
	}

	/// The Crop tool's Generative Expand (T06): after the crop grew the
	/// canvas, the area outside the old canvas (`old`, new canvas pixels).
	fn generative_expand(&mut self, doc_id: DocId, old: (i64, i64, i64, i64), prompt: String) {
		let store = self.store.clone();
		let Some(open) = self.docs.get_mut(doc_id) else { return };
		let (w, h) = (i64::from(open.doc.width), i64::from(open.doc.height));
		// FAST: the whole new canvas goes to the model (a narrow strip on a
		// big image comes back soft).
		let rect = (0, 0, w, h);
		let outside = |x: i64, y: i64| if x >= old.0 && y >= old.1 && x < old.2 && y < old.3 { 0.0 } else { 1.0 };
		let input = match ai::generative_input(&mut open.doc, &store, rect, WORKING, &mut |x, y| outside(x, y), true) {
			Ok(i) => i,
			Err(text) => {
				self.to_ui(&EngineToUi::Error { text });
				return;
			}
		};
		let name = if prompt.is_empty() {
			"Generative Expand".to_owned()
		} else {
			prompt.chars().take(60).collect()
		};
		self.run_comfy(doc_id, "Generative Expand", name, input, true, &prompt);
	}
}

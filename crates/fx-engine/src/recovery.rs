//! Periodic incremental recovery, outside the scratch lifecycle.
use super::engine::Internal;
use crossbeam_channel::{Receiver, Sender};
use fx_core::Document;
use fx_io::fxd::{self, SaveRequest, SaveTarget};
use fx_protocol::DocId;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// AUDIT-FIX(D2): emergency requests originate on a surviving shell thread, never a render wait.
static INPUT: Mutex<Option<Sender<crate::EngineInput>>> = Mutex::new(None);
static ACK: OnceLock<(Sender<()>, Receiver<()>)> = OnceLock::new();
pub(crate) fn register(input: Sender<crate::EngineInput>) {
	if let Ok(mut slot) = INPUT.lock() {
		*slot = Some(input);
	}
}
pub fn emergency(timeout: Duration) {
	let ack = ACK.get_or_init(|| crossbeam_channel::bounded(1));
	while ack.1.try_recv().is_ok() {}
	if let Some(input) = INPUT.lock().ok().and_then(|slot| slot.clone()) {
		if input.send(crate::EngineInput::EmergencyRecovery).is_ok() {
			let _ = ack.1.recv_timeout(timeout);
		}
	}
}

// AUDIT-FIX(D2): one worker owns each session's recovery handles; at most one queued snapshot per open doc.
enum Work {
	Snapshot {
		id: DocId,
		generation: u64,
		name: String,
		doc: Box<Document>,
	},
	Remove(DocId),
	Barrier,
}
pub(crate) struct Recovery {
	sender: Sender<Work>,
}
impl Recovery {
	pub fn start(store: Arc<fx_tiles::TileStore>, internal: Sender<Internal>) -> std::io::Result<Self> {
		let root = root().ok_or_else(|| std::io::Error::other("LOCALAPPDATA is unavailable; recovery disabled"))?;
		std::fs::create_dir_all(&root)?;
		let session = root.join(format!(
			"{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
		));
		std::fs::create_dir_all(&session)?;
		let lease = lock_session(&session)?;
		let (sender, receiver) = crossbeam_channel::unbounded();
		std::thread::Builder::new().name("recovery".into()).spawn(move || {
			let _lease = lease;
			let available = discover(&root, &session);
			let _ = internal.send(Internal::RecoveryAvailable { paths: available });
			let mut files = std::collections::HashMap::<DocId, Arc<fxd::FxdFile>>::new();
			for work in receiver {
				match work {
					Work::Snapshot { id, generation, name, doc } => {
						// AUDIT-FIX(P1): reset timeout reporting per recovery save job.
						let _pressure = fx_tiles::ProducerScope::enter();
						let path = session.join(format!("document-{}.fxd", id.0));
						let target = files
							.get(&id)
							.cloned()
							.map(SaveTarget::Incremental)
							.unwrap_or_else(|| SaveTarget::Fresh(path.clone()));
						let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
							fxd::save(
								SaveRequest {
									doc: &doc,
									store: &store,
									preview: None,
								},
								target,
								&mut |_| true,
							)
						}))
						.map_err(|panic| format!("recovery panicked: {}", super::engine::panic_text(&*panic)))
						.and_then(|r| r.map_err(|e| e.to_string()));
						let result = result.map(|saved| {
							files.insert(id, saved.file);
							let _ = fx_io::fs_util::write_json(
								&path.with_extension("json"),
								serde_json::json!({"name":name,"generation":generation}).to_string().as_bytes(),
							);
						});
						let _ = internal.send(Internal::RecoverySaved { doc: id, generation, result });
					}
					Work::Remove(id) => {
						files.remove(&id);
						let path = session.join(format!("document-{}.fxd", id.0));
						let _ = std::fs::remove_file(&path);
						let _ = std::fs::remove_file(path.with_extension("json"));
					}
					Work::Barrier => {
						let _ = ACK.get_or_init(|| crossbeam_channel::bounded(1)).0.try_send(());
					}
				}
			}
		})?;
		Ok(Self { sender })
	}
	pub fn snapshot(&self, id: DocId, generation: u64, name: String, doc: Document) -> bool {
		self.sender
			.send(Work::Snapshot {
				id,
				generation,
				name,
				doc: Box::new(doc),
			})
			.is_ok()
	}
	pub fn remove(&self, id: DocId) {
		let _ = self.sender.send(Work::Remove(id));
	}
	pub fn barrier(&self) {
		let _ = self.sender.send(Work::Barrier);
	}
}
fn root() -> Option<PathBuf> {
	std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Fotox/recovery"))
}
// AUDIT-FIX(D2): share-denied session lock prevents recovering files still owned by a live Windows session.
fn lock_session(session: &Path) -> std::io::Result<File> {
	let mut options = OpenOptions::new();
	options.read(true).write(true).create(true).truncate(false);
	#[cfg(windows)]
	{
		use std::os::windows::fs::OpenOptionsExt;
		options.share_mode(0);
	}
	options.open(session.join("session.lock"))
}
fn discover(root: &Path, own: &Path) -> Vec<String> {
	let mut paths = Vec::new();
	let Ok(sessions) = std::fs::read_dir(root) else { return paths };
	for session in sessions.flatten() {
		let dir = session.path();
		if dir == own || !dir.is_dir() {
			continue;
		}
		let Ok(_lease) = lock_session(&dir) else { continue };
		let Ok(entries) = std::fs::read_dir(&dir) else { continue };
		for entry in entries.flatten() {
			let path = entry.path();
			if path.extension().is_some_and(|ext| ext == "fxd") {
				paths.push(path.to_string_lossy().into_owned());
			}
		}
	}
	paths.sort();
	paths
}

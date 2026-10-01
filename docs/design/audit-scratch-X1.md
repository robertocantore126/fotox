# X1: scratch health and integrity (untested)

Current: fx-tiles/src/scratch.rs:126 creates delete-on-close file; :151 allocates best-fit4KiB extents or extends; :168 reads LZ4 block without checksum. store.rs demote_warm keeps RAM on failure but only logs; stats scratch_full never reaches protocol. engine/lib.rs:333 silently filters missing preference folder. canvas.js:145 displays only usage.

B1: ScratchHealth cached counters and sticky error in store; query available/total bytes on startup/trim worker, never engine/render disk I/O. MemoryStats adds full/error/free/reserve/path fields with defaults; status red full/error, amber near reserve, actionable tooltip. Preserve existing TileStoreStats Copy API so audit code untouched.

B2: check real-volume reserve max5GiB or5% total before each scratch write, including physical growth into a freed logical tail. Conservative implementation may refuse extent reuse near reserve too: allocator free/end bookkeeping is not physical EOF, so checking only append would incorrectly grow after logical-tail free. Keep warm source if reserve rejects. Not a disk reservation; other processes can still consume space after check.

B3: Extent in-memory crc32 over compressed bytes, no on-disk format/header change. Verify immediately after positioned read, before decompress; mismatch TileError::Corrupt, sticky visible error. Uses existing workspace crc32fast dependency. Never drop authoritative warm/hot copies on failed writes.

B4: Preferences patch validation worker probes create_new + delete own temporary file, checks folder existence/free reserve, then commits entire preference patch only on success. Startup invalid setting falls back to provided default and reports warning via cached health/Hello. Folders are not silently created from invalid user preference. Default folder still created as before. Applied scratch folder remains next-start setting.

Switch: additional FOTOX_NO_SCRATCH_GUARDS=1 sampled once at TileStore startup restores old scratch writes/read/no-validation/status path. Prompt supplied no dedicated X1 switch; use this independent switch rather than disabling memory changes too.

Invariants: render never calls free-space/filesystem probe; scratch allocator owns all writes and cleans failed extents; CRC on compressed exact length; no user file cleanup; own unique probe only. Risks: synchronous free-space queries per write slow trim, quotas vs volume total may overreserve, startup fallback may also fail; sticky error persists until restart. Unknown non-Windows free space remains unsupported, preserving existing writes. Compile-only check provides no disk failure or corruption proof.

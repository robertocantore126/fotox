# COMPCACHE: reuse existing below cache, conservative above cache (untested)

## E1: source investigation

program.rs:268 builds a tile program by walking layer tree; :286 hashes every contributing op/tile/parameters. Builder:list :404 omits hidden/empty layers but visits sibling lists. hash_ops :1015 includes layer identity, source tile IDs/solid values, blend, opacity/fill alpha, mask, clipping and Blend If; groups/adjustments hashed too.

engine/render.rs:350 clears cached programs on every new generation, retains previous ready tiles for visual fallback; :390 rebuilds requested/touched viewport/coarse programs, up to256 newly requested/frame. Dirty mips/effects cause engine worker requests. :430 composites programs; GPU cache compositor.rs:406 keys full output by (level,tx,ty)+program.key, so a brush stroke recomposites changed viewport tiles, unchanged program keys hit output cache.

Existing prefix cache compositor.rs:548 splits before the root stack segment containing hot layer, minimum2ops; prefix key hashes tile coordinate + op content, not doc/active namespace. :490 reuses cached prefix (one atlas slot), blending remaining ops; miss builds prefix separately and still does full composite that frame. But :415 gathers/uploads ALL program inputs before prefix lookup: unchanged below source tiles can be evicted then reloaded unnecessarily even with prefix cached. Warm below cache avoids below blends but does not avoid those uploads/loads. Above stack is always blended per tile. TileOutcome Ready/Deferred/Empty preserve current nonblocking fallback.

Thus implement missing context/keying, input omission on warm below hit and conservative above cache, reusing existing prefix machinery, not another output texture. Builder/hash CPU cost still visits layers; this patch does not make total input-to-present constant in layer count and cannot bypass missing derived inputs safely without a later planner change.

## E2?E5 design

At frame start pass document ID/current active layer plus immutable doc metadata. New cache context hashes identity of all layers outside active layer + global size/light/patterns; changed context bumps epoch and clears cached prefixes/suffixes. Key includes namespace(document,active,epoch), mip/tile, side tag and actual op content (more conservative than separate below generation; above edits also invalidate below). Active uses selected active rather than timeout hot state. Comparison switch preserves original hot-prefix path.

Root prefix segment already includes active clipping cluster/styles/masks in remainder. Skip input gathering for cached prefix only after lookup pins slot for current frame; encoding consumes exactly that pinned snapshot. Missing cache gathers original full inputs and composites original full program for current frame while producing reusable cache. No waits.

Above cache permitted only when suffix contains >=2 root Op::Layer of documented visible Normal pixel/SolidFill/FillLayer roots, no styles, clipping, Blend If or groups/adjustments/channels operations. Masks and opacity are baked per static layer and associative Normal over is safe. Precompose into premultiplied atlas slot; new shader op does premultiplied over, not source unpremultiply/reblend. Expected floating-point regrouping/f16 quantization can differ from old sequential path; old/new pixel comparison mandatory, no promise of bit equality.

Groups: simplest initial rule is root-active only. Active inside groups (isolated OR pass-through) falls back old full program on new path; do not reuse whole ancestor prefix as if it were inside-group cache. Isolated local-stack optimisation remains explicit limitation, rather than risking backdrop/clipping stack mistakes. Above groups always fallback.

Cached prefixes/suffixes use AtlasKey::Prefix with side-tagged hash, same shared P1 slot ceiling; evict stale cached slots before source tiles, never current frame slot. Switch active/document or outside-active edit clears caches. Use FOTOX_NO_COMPOSITE_CACHE=1 sampled compositor startup: disables all new E logic, retains exact original compositor including its older hot-prefix cache. Disabling that original cache would not be an exact old path.

Risks: existing GPU hash collision assumptions; derived cache identity changes cause excess clears. f16 cache regrouping may cause quantization differences. Full op scans/build still costly. Group cases deliberately deferred to full fallback. Slot admission snapshot must remain consistent through uploads; cache misses cannot read same-dispatch cache before texture copy. Unavailable inputs never drawn as valid zeros. Budget pressure must not evict slots already pinned this frame. No GPU/test/app executions.

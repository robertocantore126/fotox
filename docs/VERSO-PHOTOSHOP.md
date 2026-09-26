# Verso Photoshop — cosa manca a Fotox

> **Analisi di divario (gap analysis), non una roadmap approvata.**
> Revisione di riferimento: ramo `m6`, `913dc07` (M6-T00 decisioni D-050…D-056, M6-T01 core di ricampionamento fatto).
> Metodo: il documento confronta **Photoshop (versione corrente, uso generico di fotoritocco, grafica e stampa)** con quello che il codice di Fotox fa *davvero*, non con quello che i menu promettono. Ogni affermazione è ancorata a file/righe o a decisioni registrate in `docs/DECISIONS.md`. Le stime di sforzo sono mie e sono dichiarate come tali.
> Nessun file di codice è stato modificato.
>
> **Lavoro in corso nel checkout.** Al momento della stesura il working tree conteneva anche modifiche **non committate** di M6-T02 (rotazioni/flip esatti): `crates/fx-ops/src/permute.rs` (nuovo), `crates/fx-core/src/transform.rs` (+413 righe: `Permutation`, `Anchor9`, `dest_rect`), `fx-ops/src/{lib,neighbourhood,resample/mod}.rs`. Non le ho toccate e non sono incluse in questa analisi: le voci «trasformazioni» e «Image ▸ Rotation» sono valutate come sono a `913dc07`, quindi **diventeranno più verdi di così**.

---

## 0. Come leggere questo documento

Ogni area ha tre colonne di stato:

| Stato | Significato |
|---|---|
| ✅ **Alla pari** | La funzione esiste e per come è costruita regge il confronto con Photoshop. |
| 🟡 **Parziale** | Esiste la «spina dorsale» (modello, framework, comandi) ma manca la maggior parte delle funzioni visibili. |
| 🔴 **Assente** | Non esiste né il modello né la funzione. |

Classi di sforzo (stima mia, in unità del progetto: una **carta** = un task `M*-T**` come quelli esistenti):

* **carta** — 1–2 carte, riusa framework esistenti.
* **milestone** — una milestone completa (come M6): decisioni T00 + 8–10 carte.
* **struttura** — cambia il modello di documento o il percorso dei pixel: tocca 4+ crate e le decisioni D-xxx.

---

## 1. Cosa c'è già (per calibrare il resto)

Fotox non parte da zero: diverse cose sono già **alla pari o migliori** di Photoshop, ed è importante dirlo perché alcune di esse sono esattamente ciò che a Photoshop si può rimproverare.

| Capacità | Stato | Evidenza |
|---|---|---|
| **Modello a tessere fuori-core**: nessuna operazione è proporzionale al documento (con le due eccezioni difettose di §9.1) | ✅ Alla pari / avanti | `fx-tiles` (256², hot/warm/cold/backed), `docs/ARCHITECTURE.md`; apertura pigra e salvataggio incrementale di un 30k² — cosa che Photoshop non fa (lui carica tutto in scratch e risalva tutto) |
| **Aprire un `.fxd` da 30 000² è immediato**, i pixel si leggono quando servono | ✅ Avanti | `fx-io/src/fxd/open.rs`, `D-027` |
| **Compositing con tutti i 27 blend mode + Pass Through**, con gemello CPU di riferimento | ✅ Alla pari | `fx-core/src/blend.rs` (28 varianti), `docs/BLEND_MODES.md` |
| **14 livelli di regolazione** (Brightness/Contrast, Levels, Curves, Exposure, Hue/Saturation con 6 gamme, Invert, Posterize, Threshold, Gradient Map, Channel Mixer, Photo Filter, Color Balance, Vibrance, Black & White) | ✅ Alla pari su quelle 14 | `fx-core/src/layer.rs` (`enum Adjustment`) |
| **Selezioni con copertura sub-pixel** (marquee, 4 forme, lasso libero e poligonale, bacchetta magica, Modify, formiche, spostamento del contorno) | ✅ Alla pari (manca il *refine* avanzato, §5) | M5, `fx-core/src/selection.rs` |
| **Motore pennello** con dinamica di pressione, flow/opacity con buffer di tratto, clone, healing e spot healing | ✅ Alla pari sulle funzioni presenti | M5-T06/T07, `D-041`/`D-042`/`D-045` |
| **Gestione del colore** ICC, soft proof, Gamut Warning, conversione ed export CMYK (via lcms2) | ✅ Alla pari (su RGB; nessuna modalità CMYK nativa, `D-012`) | M4, `fx-color`, `D-031`…`D-033` |
| **Storia come istantanee** illimitate in profondità di ciò che cambia (limite 50 come Photoshop) | ✅ Alla pari | `fx-core/src/history.rs`, `D-009` |
| **Comandi serializzabili** (`Command`, 30 operazioni) come unico vocabolario di modifica | ✅ Fondamento sano per macro/scripting (M8) | `fx-core/src/command.rs`, `D-010` |

**Cosa significa «clone definitivo».** Nella pratica Photoshop è tre prodotti in uno: (a) un fotoritocco raster con poche decine di strumenti, (b) un compositing/vettoriale da grafica con livelli vettoriali, testo, effetti e stampa, (c) un piccolo sistema operativo grafico per file multi-formato, automazione e plugin. *«Definitivo»* è una soglia di giudizio, non una funzione: qui la traduco in tre soglie verificabili (§10).

---

## 2. Il divario in una tabella

| # | Area | Fotox oggi | Photoshop | Stato | Sforzo |
|---|---|---|---|---|---|
| 1 | Modello colore (Lab/Gray/CMYK/Bitmap/Indexed/Duotone) | solo RGB 8/16 | 8 modalità × 8/16/32 bit | 🔴 | struttura |
| 2 | Canali (colore, alpha, spot, maschere veloci) | non esiste il concetto | pannello Canali completo | 🔴 | struttura |
| 3 | Oggetti avanzati / filtri avanzati / link a file | esclusi per M6 (`D-055`) | pilastro del workflow pro | 🔴 | milestone |
| 4 | Percorsi vettoriali / penna / maschere vettoriali | solo forme parametriche (M6, parziale) | penna, editing Bézier, vettoriale completo | 🟡 | milestone |
| 5 | Testo tipografico | point/box text previsti in M6-T07 | tipografia completa (OpenType, stili, su tracciato) | 🟡 | milestone |
| 6 | Effetti livello | 5 di 10 previsti (M6-T08) | 10 + opzioni avanzate | 🟡 | carta+ |
| 7 | Filtri distruttivi | **2** (Gaussian Blur, Unsharp Mask) | ~100 | 🔴 (framework ✅) | 2–4 carte per famiglia |
| 8 | Strumenti di pittura/ritocco | 14 id reali, 3 «not yet», **Move assente** | ~70 | 🔴 | milestone |
| 9 | Selezioni avanzate (Refine, Color Range, soggetto/cielo, canali di selezione) | base completa | avanzato | 🟡 | 2–4 carte |
| 10 | Trasformazioni (libere, warp, prospettiva, contenuto) | M6-T01–T05 in corso | completa | 🟡 | in corso |
| 11 | Import PSD/PSB | previsto in M7 | nativo | 🔴 | milestone (M7) |
| 12 | Altri formati (PDF, SVG, WebP, GIF, BMP, TGA, DDS, ICO, EXR, RAW) | TIFF/PNG/JPEG | decine | 🔴 | 1 carta per formato |
| 13 | Stampa | nessuna (dialogo mock) | pipeline di stampa completo | 🔴 | milestone |
| 14 | Automazione (Azioni, Batch, Droplet, script) | nessuna (M8 previsto) | Azioni + 3 linguaggi di script | 🔴 | milestone (M8) |
| 15 | Plugin / ecosistema | nessuno | UXP + C++ + Marketplace | 🔴 | struttura |
| 16 | AI (Generative Fill, Remove Background, Subject/Sky, Super Resolution) | voci di menu finte | integrata | 🔴 | fuori portata «clonabile» |
| 17 | Video/Timeline e 3D | nessuno | Timeline + 3D | 🔴 | fuori obiettivo (§11) |
| 18 | UI/UX pro (workspace, preset pennelli, preferenze, guide/snap reali) | pannelli presenti, quasi tutti mock | completo e personalizzabile | 🟡 | milestone |
| 19 | Affidabilità (i 3 difetti critici) | 3 critici aperti | — | 🔴 | vedi `docs/reviews/2026-09-25-deep-code-review.md` |
| 20 | Verifica di conformità con Photoshop | nessuna | — | 🔴 | prerequisito, §10 |
| 21 | Piattaforme | solo Windows (`D-014`) | Win/macOS/iPad/Web | 🟡 | milestone |

---

## 3. Modello di documento: il divario più profondo

Oggi un documento è:

```rust
DocumentColor { depth: BitDepth(U8|U16), profile: ColorProfile }
LayerKind: Pixel | Group | Adjustment | SolidFill      // + Shape | Text in M6
```

Photoshop su questo strato offre molto di più, e quasi tutto **non è un'aggiunta locale**:

### 3.1 Modalità di colore e profondità
* Mancano: **Grayscale, Lab, CMYK nativo, Bitmap, Indexed, Duotone** e **32 bit/canale**.
* `D-007` (no 32f) e `D-012` (CMYK solo all'export) sono **decisioni consapevoli**, non dimenticanze: hanno senso per un editor RGB da stampa. Ma «clone definitivo» significa anche questo, e il costo non è il supporto a una modalità: è che *tutto* il codice di compositing, i 28 blend mode, le 14 regolazioni e le conversioni assumono «RGBA encoded, profilo documento». `BLEND_MODES.md` §1 lo dichiara esplicitamente («encoded, non linearizzato»): è la scelta giusta per *uguagliare* Photoshop, ma è anche quella che rende costoso aggiungere Lab/Luminosità e 32f in seguito, perché servirebbe un doppio percorso di compositing.
* **32 bit in particolare è la soglia che separa il fotoritocco dal resto** (HDR, stacking, filtri concatenati senza posterizzazione): senza, restano fuori interi flussi di lavoro fotografici.

### 3.2 Canali
Photoshop ha un modello a canali di prima classe: canali di colore, **canali alpha** (le selezioni salvate *sono* canali), canali spot per la stampa, canali di maschera veloce. In Fotox:
* l'unico «canale» è la **maschera di livello** (`Layer::mask`), legata a un livello;
* la **selezione non è salvabile** in nessuna forma: `D-028` dice correttamente che Photoshop non salva la selezione *attiva*, ma Photoshop permette di **salvarla come canale** (e di ricaricarla dopo, anche in un altro documento). Qui *Select ▸ Save/Load Selection* è un dialogo mock, e `Select ▸ New Mask` / `Mask from Selection` sono le uniche due uscite;
* il pannello **Canali** esiste solo come disegno (`ui/js/data/panels.js`, `kind: "channels"`).
* Conseguenza pratica: mancano maschere veloci (Quick Mask), maschere vettoriali, *Save Selection*, e l'intero flusso di lavoro con canali spot.

### 3.3 Che cosa manca nel modello dei livelli
| Funzione Photoshop | Stato | Nota |
|---|---|---|
| Livelli forma/testo | 🟡 M6-T06/T07 | con limiti dichiarati: niente penna/direct selection, niente gradiente/pattern nei riempimenti, warp di testo assente |
| Effetti livello | 🟡 5 su 10 (`D-054`) | mancano Bevel & Emboss, Satin, Inner Glow, Gradient Overlay, Pattern Overlay, più tutte le opzioni avanzate (Blend If, Knockout, Contour, Texture, Use Global Light come dialogo) |
| Oggetti avanzati (+ filtri avanzati, contenuto collegato) | 🔴 `D-055` | sono *un modello di documento a sé*: questo è il singolo elemento più costoso della lista dopo PSD |
| Maschere vettoriali / di ritaglio | 🟡 | clipping raster ✅, vettoriale 🔴 |
| Layer comps | 🔴 | pannello con nota «No layer comps yet» |
| Gruppi annidati oltre 11 livelli | 🔴 | limite dichiarato in `BLEND_MODES.md` §6: superarlo è un errore, non una degradazione |
| Blend If / Knockout | 🔴 | esplicitamente «not supported yet» |
| Riempimenti gradiente/pattern | 🟡 solo tinta unita | `LayerKind::SolidFill` |
| Video, 3D, Note, Slices, Campioni colore | 🔴 | fuori obiettivo (§11) |

### 3.4 La mossa mancante: nessun "Move"
Questo merita una riga a sé perché è il primo strumento che chiunque usa.

* `Command::OffsetLayer` esiste in `fx-core` con i suoi test (`command.rs:117,565`), e spostare un livello **non riscrive pixel** (`D-015`) — l'architettura è pronta.
* **Nessuno lo emette**: una ricerca su tutto il workspace trova `OffsetLayer` solo in `fx-core` (implementazione + test) e nel nome di un test in `fx-engine/src/thumbs.rs`. Non c'è azione, non c'è strumento, non c'è scorciatoia.
* Il **Move Tool (V) è lo strumento predefinito** (`fx-engine/src/view.rs:68`, `ui/js/state.js:6`) ed è definito nell'elenco della toolbar (`ui/js/data/tools.js:7`), ma `new_tool("move")` **non lo implementa** e non rientra nemmeno fra i «Non ancora» che mostrano un avviso: `tool_pointer` esce in silenzio (`engine.rs:420`, ramo `_ => None` in `tools/mod.rs`).
* Risultato: **oggi il contenuto di un livello non si può spostare in alcun modo** (solo riordinare lo *stack* trascinando nel pannello). Arriverà con M6-T04 (Free Transform), ma il «Move» con frecce, allineamento a griglia/guide e spostamento della selezione resterà da fare.

---

## 4. Strumenti: 14 su ~70

Implementati (`fx-engine/src/tools/mod.rs::new_tool`): **14 id di strumento**, cioè contagocce; 4 selezioni rettangolare/ellittica/riga/colonna; lasso libero e poligonale; bacchetta magica; pennello; matita; gomma; timbro clone; pennello correttivo; toppa. (Dietro, pennello/matita/gomma/timbro/correzione/toppa sono lo stesso motore di pittura con `Kind` diverso.)

Placeholder che *dicono* di non esistere (avviso al clic): selezione rapida, selezione oggetto, lasso magnetico. Qualunque altro id — incluso il predefinito `move` — viene ignorato **in silenzio** (`_ => None`).

Assenti (elenco Photoshop, raggruppato per importanza d'uso quotidiano):

| Gruppo | Strumenti mancanti | Coperto da |
|---|---|---|
| Base | **Move (V)**, Crop (C), Type (T), Shape/Pen (U/P) | M6-T03/T04/T06/T07 (parziale) |
| Pittura | **Paint Bucket (G)**, Gradient (G), Mixer Brush, Pattern Stamp, History Brush, Art History Brush | non previsto |
| Cancellazione | **Magic Eraser**, Background Eraser | non previsto |
| Nitidezza/sfocatura | **Blur, Sharpen, Smudge** | non previsto |
| Tono | **Dodge, Burn, Sponge** | non previsto |
| Ritocco avanzato | **Patch, Content-Aware Move, Red Eye, Liquify, Puppet Warp, Perspective Warp** | non previsto |
| Costruzione | Tracciato/Penna, selezione diretta/percorso, Path Selection | M6 lo dichiara «out of scope» |
| Misura | **Righello, Conteggio, Nota, Sostituzione colore** | non previsto |
| Sviluppo | **Camera Raw** (come filtro e come import) | non previsto |
| Selezione avanzata | **Quick Selection, Object Selection, Magnetic Lasso** (oggi placeholder) | non previsto |
| Altro | Zoom (c'è la vista), Hand (c'è), Slices, Frame, 3D | — |

Nota di metodo: la toolbar nell'interfaccia **disegna già** gli strumenti mancanti (`ui/js/data/tools.js`), quindi il divario è visibile all'utente al primo avvio.

---

## 5. Filtri: 2 su ~100 (il framework c'è)

`FilterParams` in `fx-core/src/ops.rs` ha **due varianti**: `GaussianBlur` e `UnsharpMask`. L'infrastruttura è però quella giusta e completa:

* filtro a tessere con *apron* e replicazione del bordo (`D-036`),
* anteprima live solo sulle tessere visibili al livello di vista, «vince l'ultima richiesta» (`fx-engine/src/filters.rs`),
* applicazione come job con barra di avanzamento e una voce di storia (`D-034`, `Last Filter`),
* percorso veloce a livello grossolano per raggi grandi.

Quindi aggiungere filtri è in gran parte **lavoro di kernel, non di architettura**: è la parte più «meccanica» del divario. Mancano, fra i più usati: Motion/Radial/Box/Surface/Lens Blur, Add Noise/Despeckle/Dust & Scratches/Median, Sharpen/Sharpen Edges/Smart Sharpen, High Pass, Minimum/Maximum/Offset/Custom, tutti i Distort, Stylize, Pixelate, Render (Clouds, Lens Flare, Lighting Effects), **Filter Gallery**, **Fade**, e i filtri *interattivi* (Liquify, Vanishing Point) che richiedono un percorso di anteprima interattiva diverso da quello attuale.

---

## 6. Selezioni: base completa, manca la parte "intelligente"

✅ Presenti (M5): tutte le forme, i tre modi con modificatori, Feather/Anti-alias, Border/Smooth/Expand/Contract, Invert/Reselect, spostamento del contorno, bacchetta con soglia e anti-alias, maschere dalla selezione, Fill/Clear/Layer via Copy/Cut, clipboard interna + di sistema sotto 8192² (`D-048`).

🔴/🟡 Mancano:
* **Refine Edge / Refine Hair** (selezione morbida con decontaminazione del bordo) — è la funzione che separa una maschera amatoriale da una professionale;
* **Color Range** e **Focus Area** (dialoghi mock);
* **Select Subject / Sky** (AI, vedi §11);
* **Grow** / **Similar** (nel menu, marcati non disponibili);
* **Transform Selection** (marcato non disponibile);
* **Save Selection / Load Selection** come canali (§3.2) — e quindi anche Quick Mask;
* **Select All Layers / Select Similar Layers** nel pannello.

---

## 7. File e formati

| Direzione | Supportati | Mancano |
|---|---|---|
| Aprire | TIFF, PNG, JPEG (per *sniffing* del contenuto, `fx-io/src/lib.rs:70`), `.fxd` pigro | **PSD/PSB (M7)**, PDF, EPS, SVG, WebP, GIF, BMP, HEIC/HEIF, RAW di fotocamera (CR2/NEF/ARW…), OpenEXR, DNG |
| Salvare | `.fxd` (incrementale!) | PSD/PSB in uscita, PDF multipagina |
| Esportare | TIFF, PNG, JPEG (`ExportFormat::from_path`) | WebP, AVIF, GIF, BMP, TGA, DDS, ICO, MP4, SVG reale, PDF |
| Livelli in uscita | — | «Export Layers» con nomi/pattern, «Export As» completo, Save for Web |
| TIFF | non compresso | Deflate/LZW/ZIP (`D-030` rimanda a M8), TIFF multilivello, 32f |

Il punto meno ovvio ma più importante: **`.fxd` è un formato chiuso e senza lettori esterni**. Photoshop, come strumento «definitivo», ha un formato che tutti gli altri programmi aprono. `M7` (PSD/PSB in ingresso) è la carta che risolve il 90 % del problema pratico; l'uscita PSD resterebbe da fare.

---

## 8. UI/UX: pannelli disegnati, pochi vivi

`ui/js/data/panels.js` definisce **23 pannelli**; in modalità nativa sono realmente alimentati dal motore: **Livelli**, **Storia**, **Regolazioni**, **Colore**, **Info**, più le schede documento e la barra di stato. Tutti gli altri sono rendering del mock: Canali, Tracciati, Azioni, Layer Comps, Proprietà, Istogramma, Navigator, Misure, Carattere, Paragrafo, Pennello, Impostazioni pennello, Preset strumento, Librerie, Timeline, Note.

Altri divari UX rilevanti:
* **Guide, griglia, snap**: le guide sono disegnate come percentuali statiche (`ui/js/canvas.js`), i dialoghi «New Guide» sono mock, e i comandi `order:*` (Disponi), `align:*` (Allinea), `dist:*` (Distribuisci) **non hanno implementazione nel motore** pur non essendo marcati come non disponibili: danno un avviso «not implemented yet» (§9.2). Lo stesso vale per `mask:reveal-all`/`hide-all`/`delete`/`apply`/`disable` e per `layer:new-from-bg`. (Invece `img:crop`, `img:rot90*`, `img:flip-*`, `sel:grow`, `sel:similar`, `sel:transform`, `layer:ungroup`, `doc:revert` sono correttamente **grigie** nel menu: sono le voci che arriveranno con M6 o che non sono previste.)
* **Preferenze**: dialoghi mock; non c'è nulla da configurare (memoria del magazzino, disco di lavoro, unità, griglia, colori di interfaccia).
* **Scorciatoie personalizzabili**: nessuna (mappa fissa in `ui/js/shortcuts.js`).
* **Preset**: pennelli, campioni, stili, pattern, forme — pannelli non funzionanti; non c'è import/export di preset né file `.abr`/`.aco`/`.asl`.
* **Workspace**: voci presenti, layout fisso.
* **Tablet**: solo Windows Ink (`D-046`), nessun WinTab; touch/pinch non gestiti.
* **Localizzazione e accessibilità**: interfaccia solo inglese, nessun supporto tastiera completa/lettori di schermo.

---

## 9. Affidabilità e performance (prerequisito, non un'area)

### 9.1 I tre difetti critici già documentati
Vanno chiusi **prima** di aggiungere funzioni, perché colpiscono una per uno le tre promesse centrali del prodotto (documenti enormi, salvataggio incrementale, lavori pesanti che non bloccano):

1. memoria proporzionale al documento in `composite_layers` e `EngineOps::filter` (viola la regola d'oro del progetto: ~6,7 GB su un 30 000²);
2. due salvataggi concorrenti sullo stesso `.fxd` (basta tenere premuto Ctrl+S) possono corrompere il file;
3. un errore dentro un lavoro pesante lascia il documento «occupato» per sempre.

Dettaglio, prove e come si riproducono: `docs/reviews/2026-09-25-deep-code-review.md`.

### 9.2 Il menu promette più di quanto il motore faccia
`ui/js/data/menus.js` contiene **467 voci** (di cui 75 esplicitamente disabilitate) e 53 sottomenu. Il commento in `actions.js` dice che ogni azione viene comunque inviata al motore, che risponde con un avviso per ciò che non possiede. Non è un difetto *di sé* — è onesto — ma per una valutazione del divario significa che **la copertura reale non si legge dai menu**. Le voci **non disabilitate** ma senza implementazione nel motore sono le più insidiose, perché l'utente le clicca e scopre un avviso. Verificate a questa revisione (nessun riscontro in `crates/fx-engine/src`, e nessun `dis: true` nel menu):

* `order:*` (Disponi: porta in primo piano / avanti / indietro / in fondo);
* `align:*` e `dist:*` (Allinea e Distribuisci, 6 voci ciascuno);
* `mask:reveal-all`, `mask:hide-all`, `mask:delete`, `mask:apply`, `mask:disable` (funzionano solo `mask:add`, `mask:reveal-sel`, `mask:hide-sel` e `layer:edit-mask`);
* `layer:new-from-bg`, `layer:content-options`, `layer:track`, `layer:rasterize-clip`, `layer:copy-style`, `layer:paste-style`, `layer:clear-style`;
* `sel:all-layers`, `smart:convert`, `raster:*`, `type:*`, `doc:save-psd`;
* l'intera famiglia `Render` dei filtri (Clouds, Lens Flare…) e in generale tutte le voci `dlg:*` dei filtri che non siano Gaussian Blur o Unsharp Mask;
* tutti i dialoghi `dlg:*` non cablati (Image Size, Canvas Size, Preferenze, Print, Fill non-`edit:fill`…): si aprono e non fanno nulla.

### 9.3 Performance e architettura
* **I filtri sono CPU-only** (`D-034`): giusto come riferimento, ma per l'interattività «Photoshop-like» (Liquify, Camera Raw, sfocature a raggio grande su 30k) serve un percorso GPU o incrementale.
* **Il budget di memoria è fisso a compile time** (`TileStoreConfig`: 5 GiB caldi + 3 GiB tiepidi, 6 GiB di atlas GPU) e non c'è un pannello Performance come in Photoshop (percentuale RAM, numero di dischi di lavoro, cache delle cronologie).
* **Nessuna cache persistente fra sessioni** (Photoshop tiene la cache delle anteprime su disco).
* **Nessun percorso a 32 bit** (§3.1), **nessuno zero-copy PSD**.
* Il modello a tessere è però il fondamento giusto: le performance di apertura/navigazione sono già dichiarate nei criteri S1, S2, S8 (`docs/PERFORMANCE.md`).

---

## 10. Il prerequisito che manca del tutto: una suite di conformità con Photoshop

Oggi **non esiste alcun test che confronti Fotox con Photoshop**. `BLEND_MODES.md` contiene già sei «**VERIFY**» da controllare *in M7*, quando i file PSD daranno dei casi di prova. Senza questo, «clone definitivo» resta un'opinione.

Serve, come primo passo di qualsiasi piano di parità:

1. **Corpus di riferimento**: 30–50 PSD reali (livelli, gruppi pass-through, clipping, maschere, 27 blend mode, effetti, testo, forme, regolazioni) + i PNG di riferimento esportati da Photoshop.
2. **Test di confronto pixel-per-pixel** con tolleranze dichiarate (oggi: `< 0.2 %` dei canali oltre 2/1024 è la soglia dei test GPU — riusabile).
3. **Criteri nuovi** da aggiungere a `docs/PERFORMANCE.md §4` per le aree che arriveranno: un S21 per il testo (keystroke → pixel), un S22 per il PSD (apertura di un 500 MB con 200 livelli), un S23 per la stampa/esportazione PDF, un S24 per le azioni/batch.
4. **Un fuzzer del contenitore `.fxd`** (i tre difetti critici mostrano che il percorso «file danneggiato» non è mai stato esercitato).

---

## 11. Cosa *non* conviene inseguire (e perché)

Un clone «definitivo» inteso come *parità di elenco funzioni* è un obiettivo economicamente irraggiungibile e tecnologicamente in ritirata. Meglio dichiarare i non-obiettivi:

* **AI generativa** (Generative Fill, Remove Background, Super Resolution): richiede modelli e infrastruttura cloud; le voci di menu esistono già come finzione e andrebbero rimosse o marchiate come «non in Fotox» per onestà.
* **Video/Timeline e 3D**: domini separati con formati, codec e GPU pipeline propri; nessuna sinergia con il modello a tessere.
* **Ecosistema di plugin (UXP/C++) e Marketplace, cloud, collaborazione**: è un investimento di piattaforma, non di editor.
* **Piattaforme multiple** (macOS/iPad/Web): `D-014` dice Windows-only testato; portare BGE/CEF+wgpu su macOS è una milestone a sé e non aggiunge capacità di editing.
* **PSD in *uscita* fedele al 100 %** (smart object, effetti, testo editabile): è un formato proprietario e non documentato in modo autorevole; conviene puntare a un *import* eccellente (M7) e a un export «appiattito ma corretto».

Quello che invece **non** è negoziabile se si vuole il titolo: modalità colore (almeno Grayscale/Lab + 32f), canali, PSD in ingresso, penna/vettoriale, testo serio, Move e strumenti di base, filtri più usati, stampa, automazione, formati di scambio comuni.

---

## 12. Piano proposto, in tre soglie

### Soglia A — «clone usabile» (fotoritocco e compositing quotidiani)
Obiettivo: un professionista può fare una giornata di lavoro senza aprire Photoshop.

| Priorità | Cosa | Sforzo |
|---|---|---|
| 1 | Chiudere i 3 difetti critici + fuzzare `.fxd` | 3 carte |
| 2 | **Move Tool** (drag, frecce, Shift, allineamento) + Arrange/Align/Distribute | 1–2 carte |
| 3 | M6 come previsto (trasformazioni, crop, forme, testo, 5 effetti) | resto di M6 |
| 4 | **M7: import PSD/PSB** + suite di conformità (§10) | milestone M7 |
| 5 | Filtri: le 5 famiglie più usate (Blur/Noise/Sharpen/Stylize/Other) + **Fade** | 4–6 carte |
| 6 | Strumenti base mancanti: Paint Bucket, Gradient, Dodge/Burn/Sponge, Blur/Sharpen/Smudge, Magic Eraser | 4–5 carte |
| 7 | Selezioni: **Save/Load Selection come canali**, Grow/Similar, Transform Selection, Refine Edge | 3 carte |
| 8 | Effetti livello: le altre 5 + opzioni avanzate (Blend If, Knockout) | 2–3 carte |
| 9 | Formati di scambio: PDF (export), WebP, GIF, BMP, SVG export | 3–4 carte |
| 10 | UI pro: preferenze reali, scorciatoie personalizzabili, guide/griglia/snap veri, preset pennelli/swatch/stili | 4–6 carte |

### Soglia B — «fotografo professionista»
Obiettivo: il flusso di lavoro fotografico non richiede più Photoshop.

* **Camera Raw** (sviluppo RAW + come filtro), stacking/HDR/panorama (Photomerge, Merge to HDR), 
* **32 bit/canale** e spazi Lab/HDR,
* **Oggetti avanzati** + filtri avanzati,
* **Liquify / Puppet Warp / Content-Aware Fill&Move / Patch**,
* **Color Range / Refine Hair / Select Subject** (anche solo algoritmico, non AI),
* scripting (JS o Python) sopra i `Command` già serializzabili (`D-010`),
* **M8: Azioni e Batch**, Droplet, Image Processor.

Sforzo: 4–6 milestone, di cui due strutturali (32f, oggetti avanzati).

### Soglia C — «clone definitivo»
Obiettivo: chi apre Fotox per la prima volta non trova nulla che gli manchi.

* Tutte le **modalità di colore** (Gray/Lab/CMYK nativo/Indexed/Bitmap/Duotone) con conversioni e profili,
* **pannello Canali** completo (colore, alpha, spot, Quick Mask),
* **penna/vettoriale** completo (editing Bézier, maschere vettoriali, import/export SVG, pattern e gradienti nei riempimenti, testo su tracciato),
* **tipografia professionale** (OpenType, stili di paragrafo e carattere, giustificazione, verticale),
* **stampa** reale (impostazione pagina, profili di uscita, segni di taglio, PDF passthrough),
* **plugin host** o API di scripting esterna,
* **Layer comps, Data Sets/Variabili, History Log, Note, Misure, Slices**,
* verifica sistematica sui **30–50 PSD** di riferimento con soglie pubbliche.

Sforzo: 6–10 milestone, con almeno tre aree «struttura» (§2, righe 1, 2, 3).

---

## 13. Definizione operativa di «fatto»

Proposta di criteri verificabili, da aggiungere al progetto (non a `PERFORMANCE.md` finché non approvati):

1. **Parità di risultato**: per ogni funzione dichiarata, un test di confronto con Photoshop sullo stesso PSD, con tolleranza dichiarata e **zero** «VERIFY» aperti in `BLEND_MODES.md`.
2. **Parità di formato**: un PSD reale di un cliente si apre, si modifica e si risalva senza perdita di contenuto *supportato*, con un rapporto scritto su cosa è stato appiattito.
3. **Nessun vicolo cieco nell'interfaccia**: nessuna voce di menu non disabilitata che risponda «not implemented yet» (oggi ce ne sono molte, §9.2). Le voci non realizzate vanno spente o rimosse.
4. **Robustezza**: nessun file danneggiato può bloccare il documento; i tre difetti critici chiusi e coperti da test.
5. **Scala**: tutti i criteri S1–S24 passano su B1 (10k²) e B3 (30k²) alla macchina di riferimento, con la RAM entro il budget dichiarato.

---

## 14. Riassunto in cinque righe

1. Fotox ha un **fondamento migliore** di Photoshop su ciò che conta per i documenti enormi (tessere, apertura pigra, salvataggio incrementale) e già copre compositing, regolazioni, selezioni e pittura.
2. Il divario **più profondo** è nel modello di documento: modalità di colore, canali, oggetti avanzati, vettoriale e testo serio.
3. Il divario **più visibile** è nell'elenco degli strumenti (12 su ~70, incluso **Move**, che manca del tutto) e nei filtri (**2** su ~100, ma il framework c'è).
4. Il divario **più costoso da rimandare** è l'ingresso PSD (M7) e la suite di conformità con Photoshop: senza, ogni altra parità resta non verificata.
5. Prima di tutto: i tre difetti critici, perché riguardano esattamente le tre promesse su cui Fotox batte Photoshop (documenti enormi, salvataggio incrementale, lavori pesanti senza bloccare).

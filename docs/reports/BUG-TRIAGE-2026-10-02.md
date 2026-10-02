# Correzioni del triage bug hunting — 2 ottobre 2026

Base: `main` locale `bfbe5d90ce8b390493f10bbb590acd285f4a3ba3`. Riferimento: «Fotox — triage delle segnalazioni di bug hunting.md». Intervento richiesto dall'utente sui bug e difetti, escludendo le scelte architetturali.

## Ambito realizzato

I 26 difetti confermati ancora aperti nella verifica iniziale hanno ora modifiche correttive nel codice. R01 e R05 erano già corretti nella base e sono stati conservati. Questo è un riepilogo di implementazione e verifiche, non la certificazione di tutte le sequenze native del documento.

| Segnalazioni | Correzione |
| --- | --- |
| R02, R03 | Gli Smart Filter aggiornano sincronicamente i parametri; le anteprime non entrano nella coda dei lavori pixel. Annullamento di anteprime e Place basato sull'identità stabile dello stato, anche con 50 passi; ripristino del precedente stato di modifica del documento. |
| R04 | `map_layer` e `fill_with` consegnano allo store gruppi di otto tile, senza trattenere tutti i buffer del livello. |
| R06 | Token di sessione nella preparazione asincrona di Trasformazione libera; risultati di sessioni vecchie scartati. |
| R07, R08 | Annulla Stile livello ripristina Luce globale; il valore UI in attesa è associato al documento. |
| R09 | Il caricatore memorizza le tile illeggibili, evitando il ciclo continuo di lettura/errore/ridisegno. |
| R10 | Escludi usa la differenza simmetrica; i sotto-tracciati vengono combinati prima di applicare la modalità rispetto alla selezione esistente. |
| R12 | Il completamento AI riesamina la chiusura della finestra anche in caso di errore o annullamento. |
| R13, R14 | La cronologia distingue le modifiche al contenuto dalle selezioni. Pennello storia usa identità stabili e la UI aggiorna la riga della sorgente o segnala che non è più disponibile. |
| R15, L12 | Le maschere vettoriali rasterizzano le operazioni booleane reali; la sfumatura usa il filtro a tile con i margini necessari. |
| Forme personalizzate | Normalizzazione rispetto all'origine e all'estensione effettiva. |
| Clipboard | Lettura prioritaria del formato PNG per conservare la trasparenza; limiti e aritmetica controllata nella lettura DIB. |
| Miniature canali | Firma calcolata su tutte le tile, oltre le prime 16. |
| Lazo magnetico | Il segmento completo viene campionato con una risoluzione ridotta quando supera 2.048 pixel, senza tagliare l'estremità. |
| Tracciati importati | Tolleranza geometrica per gli ancoraggi terminali quasi coincidenti. |
| L02 | New Smart Object via Copy duplica integralmente le proprietà del livello, assegnando una sorgente indipendente. |
| L03, L15 | Unisci compone nel contesto interno del gruppo superstite; i gruppi con figli esclusi perdono le cache effetti del contenuto originale. |
| L05 | Spostamenti e trasformazioni aggiornano le maschere collegate di forme, testi e Smart Object e i tracciati vettoriali. L'origine delle maschere non pixel viene conservata nel formato FXD. |
| L07, L09 | Blend If e Riempimento dei gruppi entrano nei programmi CPU e GPU; Blend If viene applicato anche alle regolazioni. |
| L10 | Copia stile conserva i pattern del documento sorgente per l'incollaggio fra documenti. |
| L11 | Converti testo in forma conserva i colori delle parti; un successivo cambio di riempimento li sostituisce. I colori partecipano alla conversione del profilo. |

Correzioni aggiuntive di lacune concrete: R16 rifiuta i domini LUT non supportati; R11 controlla l'annullamento anche dopo un risultato AI con embedding in cache; Rimuovi sfondo interseca la maschera preesistente; L06 applica alla maschera collegata la stessa mappa del ridimensionamento in base al contenuto; L14 applica il metodo di fusione degli Smart Filter. Seleziona e maschera conserva documento/livello destinatari, produce una maschera nascosta per un risultato vuoto e scarta l'output dopo un lavoro fallito. La cache dei layout testo è limitata e condivisa per contenuto/PPI. La selezione immutata di un ancoraggio non crea un passo vuoto.

## Verifiche eseguite

- Compilazione dell'intero workspace e di tutti i target: superata.
- Build dell'app e preparazione del bundle Windows: superate.
- Test di `fx-core`, `fx-render`, `fx-io`: superati. Sono inclusi i confronti GPU/CPU esistenti.
- Test del motore: libreria, `edit_flow`, `style_options`, `review_2026_09_27`: superati.
- Nuove regressioni su cronologia piena, selezioni, colori delle forme, Interseca/Escludi, Blend If e Riempimento; confronto GPU/CPU per Blend If di gruppi/regolazioni; spostamento maschere, sfumatura attraverso il confine fra tile, Unisci nel gruppo, intersezione dello sfondo e mappa di ridimensionamento della maschera: superate.
- Tre test della clipboard, inclusi PNG con alpha parziale/nullo e DIB con dimensioni estreme: superati.
- Sintassi JavaScript di `styles.js` e controllo del diff: superati. Formattazione Rust eseguita; i file estranei all'intervento sono stati mantenuti invariati.
- Avvio nativo: NVIDIA RTX 3060/DX12 inizializzata, primo fotogramma UI ricevuto, UI pronta e collegata al motore. Istanza chiusa normalmente al termine.

## Limiti e punti esclusi

L'avvio non sostituisce una prova manuale di ogni gesto o finestra di dialogo. Restano da provare dal vivo le sequenze temporali di anteprima/Annulla, AI/chiusura, Trasformazione libera e i passaggi di Luce globale fra documenti. I test della clipboard verificano i decoder; non sostituiscono l'incollaggio da ogni programma esterno. Il Lazo magnetico sui segmenti grandi usa una risoluzione ridotta: il limite di memoria resta quello preesistente.

Non sono stati introdotti budget adattivi, mip sulla GPU, riprogettazioni di COMPCACHE, caricamenti raggruppati o nuovi benchmark di migliaia di livelli. L04 resta la scelta di prodotto esclusa dal documento. Le segnalazioni non dimostrate su selezione di ancoraggi cancellati, riuso degli ID e messaggi delle deformazioni non sono state dichiarate risolte. I percorsi prestazionali di miniature canali e campionatori composti richiedono misure e rimangono fuori da questo intervento.

Nessun push remoto. I file locali preesistenti `.freebuff/` e `crates/fx-engine/tests/tmp_bughunt_effects.rs` sono esclusi dalle modifiche.

# Audit: commit 5e97a6d — Rust-side reranker shipping fix

**Scope:** `crates/oracle-core/src/model_download.rs`, `crates/oracle-core/src/query/reranker.rs`, `crates/oracle-core/src/lib.rs`, `src-tauri/src/oracle/mod.rs`
**Reviewer:** Auditor (read-only, no cargo/npm)

---

## REPERTO 1 — La parametrizzazione ha indebolito le protezioni del downloader?

**SMENTITO.** Le protezioni precedenti (`.part` + rinomina atomica, HEAD/size verification, cancellazione, timeout) erano tutte nel corpo di `ensure_bundle_onnx_async` / `download_file`. La parametrizzazione ha spostato i dati (URL base, lista file, model_id) nel `ModelBundleDescriptor` e ha aggiunto `write_model_config_if_missing` + `is_complete` alla fine del loop, ma il corpo del download è identico.

Verifica specifica per entrambi i pacchetti:

| Protezione | BGE (34 MB) | TinyBERT (5 MB) |
|---|---|---|
| `.part` + rename atomica | ✅ `download_file` usa `dest.with_extension(...)` + `fs::rename` (riga ~196-198 mod.rs download) | ✅ stessa funzione condivisa |
| HEAD size check pre-download | ✅ `remote_len` → `bytes_total` → `effective_download_cap` | ✅ identico |
| Size mismatch post-download | ✅ `got != expected` → rimuove `.part` e bail (riga ~230) | ✅ identico |
| Cap 8 GiB su file singolo | ✅ `MAX_DOWNLOAD_BYTES` usato in `effective_download_cap` | ✅ identico |
| Cancel prima di ogni file | ✅ `if cancel.is_cancelled()` all'inizio del loop | ✅ identico |
| Request timeout 10 min | ✅ `reqwest::Client::builder().timeout(MODEL_REQUEST_TIMEOUT)` | ✅ stesso client |
| Read timeout 30s idle | ✅ `MODEL_READ_TIMEOUT` | ✅ identico |
| HTTPS-only redirect | ✅ `redirect::Policy::custom` refuses non-https | ✅ identico |
| Symlink clear pre-download | ✅ `clear_broken_model_dir_symlink(dir)` prima del loop | ✅ identico |

**Nota sull'ordine:** il `write_model_config_if_missing` e il check `is_complete` sono ora DENTRO `ensure_model_onnx_async` (riga ~336-343 model_download.rs) invece che nel wrapper. Questo è corretto: nel codice originale `ensure_bge_small_onnx_with_cancel` li faceva dopo il ritorno di `ensure_bundle_onnx_with_cancel`. Ora sono nello stesso punto logico, solo spostati nella funzione async. Il risultato è identico: il config viene scritto dopo tutti i download e prima della validazione.

---

## REPERTO 2 — Due scaricamenti insieme: interferiscono?

**SMENTITO.** I due download sono completamente isolati.

- **Stato separato:** `self.model_download: Arc<Mutex<ModelDownloadState>>` e `self.reranker_download: Arc<Mutex<ModelDownloadState>>` sono due `Arc` indipendenti (mod.rs righe 48-49).
- **Thread separati:** `start_bundle_download` (riga 564) riceve `slot: Arc<Mutex<ModelDownloadState>>` e clona l'Arc per il thread. Ogni download ha il proprio thread con nome diverso: `oracle-embedding-model-download` e `oracle-reranker-model-download`.
- **Directory diverse:** BGE va in `models/bge-small-en-v1.5/`, TinyBERT va in `models/ms-marco-TinyBERT-L-2-v2/`. Nessuna sovrapposizione.
- **Cancellazione:** `cancel_model_download` (riga 767-778) itera su entrambi gli slot e chiama `.cancel()` su entrambi. Questo è corretto: un utente che annulla "il download" intende annullare tutto.
- **Progresso:** Il progresso del reranker va nello slot `reranker_download`, quello dell'embedder in `model_download`. Il frontend riceve entrambi come campi separati: `status.model` e `status.reranker` (mod.rs riga 1456-1457). Il frontend mostra il reranker come badge separato (`RerankerStatus` in OracleSearch.tsx), non come barra di progresso unica.
- **Retry:** `onRetryReranker` (OraclePanel.tsx riga 312) chiama `handleModelDownload` che chiama `oracleModelDownloadStart` che chiama `start_model_download(true)`. Questo avvia **entrambi** i download con `force=true`. Questo è ragionevole: l'utente che premi "Retry" intende riprovare tutto.

**PLAUSIBILE (ma minore):** Se l'utente premi "Cancel" mentre sta scaricando solo il reranker (l'embedder è già Ready), l'embedder non viene toccato (il suo slot è già in stato Ready, e `start_bundle_download` esce con `Ok(())` a riga 593-595). Questo è corretto.

---

## REPERTO 3 — Lo stato del riordinatore: l'enum copre tutti i casi?

**CONFERMATO (con un caveat minore).**

L'enum `OracleModelState` ha: `NotApplicable`, `Missing`, `Downloading`, `Ready`, `Failed`, `Cancelled`.

Flussi di stato per il reranker:
1. **Primo avvio, assente:** `Missing` → download parte → `Downloading` → `Ready` o `Failed` o `Cancelled`
2. **Presente e completo:** `Ready`
3. **Non-BGE backend (Candle):** L'embedder è `NotApplicable`, ma il reranker segue comunque il flusso normale (manca solo l'embedder). Il reranker non ha una ragione per essere `NotApplicable` dato che usa ORT indipendentemente dal backend dell'embedder.
4. **Cartella vuota:** `configured_reranker_present` (reranker.rs riga 226-233) richiede: (a) `model_config.json` parsabile, (b) `onnx/model_quantized.onnx` esistente, (c) `tokenizer.json` esistente. Una cartella vuota → `RerankerConfig::load` fallisce → `false`. ✅
5. **Cartella con config ma file mancanti:** `graph_path` o `tokenizer_path` fallisce → `false`. ✅
6. **Download interrotto (file parziali):** I file parziali hanno estensione `.part` e non vengono rinominati. `configured_reranker_present` cerca `onnx/model_quantized.onnx` (senza `.part`), quindi non lo trova → `false`. ✅

**Caveat minore:** `reranker_status()` (mod.rs riga 529-539) legge lo stato dal mutex e poi controlla `configured_reranker_present` su disco. Se lo stato è `Downloading`, non rilegge il disco (la condizione `state.status.state != OracleModelState::Downloading` è `false`). Questo è corretto: durante il download non ha senso rileggere il disco.

**Cosa NON copre:** Lo stato non distingue tra "non scaricato" e "scaricato ma corrotto" — entrambi risultano `Missing` o `Failed` a seconda del momento. Questo è accettabile: `configured_reranker_present` rileva l'incompletezza e il download viene ritentato.

---

## REPERTO 4 — Il contratto IPC: `"dense+reranked"` attraversa tutto il giro?

**CONFERMATO.**

Il giro completo:

1. **Produzione:** `engine.rs` riga 404 imposta `row.retrieval = "dense+reranked".to_string()` sui candidati rerankati.
2. **Mapping Rust:** `result_from_context` (mod.rs riga 1471) fa match su `"dense+reranked"` → `Some(OracleMatchType::DenseReranked)`.
3. **Serializzazione:** `OracleMatchType::DenseReranked` ha `#[serde(rename = "dense+reranked")]` (mod.rs riga 920).
4. **Test:** `result_mapping_preserves_the_reranked_match_type` (mod.rs riga 1630-1656) verifica sia il Rust enum che la serializzazione JSON.
5. **TypeScript:** `OracleMatchType` in `ipc.ts` include `"dense+reranked"` nella union (ipc.ts riga ~190-195).
6. **Frontend:** `OracleResultRow` (OracleSearch.tsx riga ~178) mostra `result.match_type` direttamente.

**Verifica dei valori mancanti:** L'engine produce esattamente quattro stringhe di retrieval:
- `"lexical"` — scorso da solo (riga ~446)
- `"dense"` — scored ma non fuso con lexical né rerankato (riga ~370)
- `"dense+reranked"` — rerankato (riga 404)
- `"dense+lexical"` — fuso lexical+dense (riga 439)

La funzione `result_from_context` ha un `_ => None` per valori sconosciuti. Se l'engine producesse un valore non mappato, il frontend mostrerebbe `match_type: null` — silenzioso ma non crasha. Questo è lo stesso comportamento pre-fix. Il `_ => None` è ragionevole come fallback.

**Verifica raggiungibilità:** `oracle_ask_inner` (mod.rs riga 1147) chiama `runtime.reranker()` e passa il risultato a `open_engine`. Se `reranker()` ritorna `Some(...)`, il reranker viene iniettato nel `QueryEngine`. Se `None`, il ramo reranker in `engine.rs` (riga 399: `if let Some(ref reranker) = self.reranker`) non viene eseguito e le righe restano `"dense"`. Questo è corretto: l'assenza del reranker degrada in modo trasparente.

---

## REPERTO 5 — Il costo a riposo: `reranker_status()` tocca il disco?

**CONFERMATO (ma irrilevante).**

`reranker_status()` (mod.rs riga 529-539) chiama `configured_reranker_present(Path::new(&state.status.directory))` che legge `model_config.json` dal disco, poi verifica che `onnx/model_quantized.onnx` e `tokenizer.json` esistano. Questo succede:

1. A ogni `oracle_status` chiamata dal frontend (polling ogni 1.5 secondi, OraclePanel.tsx riga ~231).
2. A ogni `oracle_ask` (via `oracle_ask_inner` → `start_model_download` → ... → `status_from_snapshot`).
3. A ogni `oracle_doctor`.

Il costo è: `open("model_config.json")` + `read` + JSON parse + `stat("onnx/model_quantized.onnx")` + `stat("tokenizer.json")`. Tre syscall stat + una read + un parse JSON. Su SSD: < 1ms. Su un file system di rete: potrebbe essere più lento, ma il reranker non cambia durante l'esecuzione. L'unica variazione è dopo un download completato: il stato passa da `Downloading` a `Ready` nel momento in cui `reranker_status()` legge il disco e trova i file completi.

**PLAUSIBILE (ottimizzazione):** Si potrebbe cachare il risultato di `configured_reranker_present` e invalidarlo solo al completamento del download. Ma il gain è < 1ms per chiamata, quindi non giustifica la complessità.

---

## REPERTO 6 — `reranker()`: nested lock con `reranker_download`

**CONFERMATO come funzionante, ma la forma merita attenzione.**

`reranker()` (mod.rs riga 497-518):
```
fn reranker(&self) -> Option<SharedReranker> {
    let mut slot = self.reranker.lock();       // LOCK 1: self.reranker
    if slot.is_none() {
        let directory = self.reranker_download.lock()  // LOCK 2: self.reranker_download (nested)
            .status.directory.clone();
        // LOCK 2 released here
        if !directory.is_empty() {
            if let Some(handle) = RerankerHandle::if_present(...) {
                *slot = Some(Arc::new(handle));
            }
        }
    }
    slot.clone()
}
```

**Analisi deadlock:** L'ordine di lock è sempre `reranker` → `reranker_download`. Verificato su tutti i call path:
- `oracle_status_inner` → `status_from_snapshot` → `reranker_status()` → lock solo `reranker_download` (no `reranker`). OK.
- `oracle_ask_inner` → `runtime.reranker()` → lock `reranker` → (nested) lock `reranker_download`. OK.
- `oracle_ask_inner` → `runtime.start_model_download(false)` → lock `reranker_download` (solo). OK.
- `start_bundle_download` → spawned thread → lock sul proprio `slot` (clonato). OK.
- `cancel_model_download` → lock `reranker_download`. OK.

Nessun path acquisisce `reranker_download` prima di `reranker`. Nessun deadlock.

**PLAUSIBILE (robustness):** La forma è corretta ma fragile: se un futuro developer aggiunge un path che acquisisce `reranker_download` prima di `reranker`, crea un deadlock. Un commento `// Lock order: reranker → reranker_download` nella struttura `OracleRuntime` sarebbe prudente.

---

## REPERTO 7 — `start_model_download`: il reranker parte sempre, anche con un modello non-BGE custom?

**CONFERMATO.** Questo è intenzionale e corretto.

Quando `self.model_id != BGE_SMALL_MODEL_ID`:
- Il download dell'embedder non parte (stato diventa `Failed` con messaggio informativo).
- Il download del reranker parte comunque (il reranker è un componente separato, indipendente dal modello di embedding).

Scenario: un developer ha `DEVBOULE_ORACLE_MODEL=custom-model-v3`. L'embedder non può essere scaricato automaticamente (messaggio: "has no automatic installer"). Ma il reranker TinyBERT (5 MB) viene scaricato normalmente. Quando fa una query con dense paths, il reranker funziona; l'embedder deve essere fornito manualmente.

Questo è corretto: il reranker non dipende dal modello di embedding.

---

## REPERTO 8 — `reranker_status()` con `Downloading` non aggiorna lo stato su disco

**SMENTITO come bug, PLAUSIBILE come design choice.**

`reranker_status()`:
```rust
if state.status.state != OracleModelState::Downloading
    && configured_reranker_present(Path::new(&state.status.directory))
{
    state.status.state = OracleModelState::Ready;
    ...
}
```

La condizione `!= Downloading` impedisce che lo stato venga sovrascritto durante il download. Questo è corretto: il download thread è l'unico che dovrebbe cambiare lo stato da `Downloading` a `Ready/Failed/Cancelled`.

Il caso in cui il download è appena terminato ma lo stato non è ancora aggiornato dal thread: il download thread scrive lo stato finale (mod.rs riga ~630-690) all'interno del lock su `cancel_slot`. Se `reranker_status()` viene chiamato prima che il thread scriva, legge ancora `Downloading` (o `Missing`). Non c'è race condition: il lock è lo stesso (`reranker_download`).

---

## REPERTO 9 — `if_present` prima: `is_dir()` bastava. Ora: `configured_reranker_present` è più restrittivo.

**CONFERMATO.** Questa è la correzione del difetto originale.

Prima: `model_dir.is_dir()` → una cartella vuota bastava a dire "c'è".
Ora: `configured_reranker_present` → richiede `model_config.json` parsabile + `onnx/model_quantized.onnx` + `tokenizer.json`.

Test`configured_bundle_requires_the_sidecar_and_declared_files` (reranker.rs riga 563-583) verifica esplicitamente:
1. Solo `model_config.json` → `false`
2. + `tokenizer.json` → `false`
3. + `onnx/model_quantized.onnx` → `true`

Questo copre esattamente lo scenario del difetto originale: una cartella vuota o parzialmente popolata non viene più scambiata per un bundle completo.

---

## REPERTO 10 — `reranker()` legge `reranker_download.status.directory` senza lock su `reranker_download`

**PLAUSIBILE (stale read).** La directory viene letta e clonata dentro il lock su `self.reranker`, ma NON dentro il lock su `self.reranker_download`:

```rust
let mut slot = self.reranker.lock();       // lock 1
if slot.is_none() {
    let directory = self
        .reranker_download               // ← NO LOCK QUI
        .lock()                          // lock 2
        .unwrap_or_else(|error| error.into_inner())
        .status
        .directory
        .clone();
```

Ah no, in realtà il lock SU `reranker_download` È preso (riga 504: `self.reranker_download.lock()`). La directory viene letta dentro entrambi i lock. Questo è corretto.

**SMENTITO.** Non c'è stale read: la directory viene letta dentro il lock su `reranker_download`.

---

## REPERTO 11 — Il test `status_exposes_a_missing_optional_reranker` validato?

**CONFERMATO.** Il test (mod.rs riga 1662-1677):
1. Crea un runtime con backend "candle" (usa `TestEnvironment::new("candle")`).
2. Configura un root temporaneo.
3. Chiama `reranker_status()` e verifica che lo stato sia `Missing`.
4. Verifica che il `model_id` sia `ms-marco-TinyBERT-L-2-v2`.
5. Verifica che il messaggio contenga "reranker".

Questo test non richiede un modello reale né un download. Verifica che il reranker venga rilevato come mancante quando non c'è. Il test pulisce `ORACLE_RERANKER_MODEL_DIR` nel `TestEnvironment`, quindi la directory è quella di default (sotto il root temporaneo), che è vuota.

---

## RIEPILOGO

| # | Reperto | Verdetto | Rischio |
|---|---|---|---|
| 1 | Parametrizzazione indebolisce protezioni | **SMENTITO** | Nessuno |
| 2 | Due scaricamenti insieme interferiscono | **SMENTITO** | Nessuno |
| 3 | Stato enum copre tutti i casi | **CONFERMATO** | Nessuno |
| 4 | Contratto IPC `dense+reranked` completo | **CONFERMATO** | Nessuno |
| 5 | `reranker_status()` tocca il disco | **CONFERMATO** (irrilevante) | Trascurabile |
| 6 | Nested lock `reranker` → `reranker_download` | **CONFERMATO** funzionante | Fragile (commento consigliato) |
| 7 | Reranker parte sempre, anche con modello non-BGE | **CONFERMATO** (intenzionale) | Nessuno |
| 8 | Stato `Downloading` non sovrascritto | **SMENTITO** come bug | Nessuno |
| 9 | `if_present` ora usa check completo | **CONFERMATO** | Nessuno |
| 10 | Stale read su directory | **SMENTITO** | Nessuno |
| 11 | Test stato missing | **CONFERMATO** | Nessuno |

**Nessun must-fix trovato.** Un solo nice-to-have: commento sull'ordine di lock nella struttura `OracleRuntime`.

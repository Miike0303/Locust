# NMT-SPIKE — RTX 5070 Ti / Windows 11

Measured on 2026-10-28 in `C:/Projects/Locust-nmt-spike`. The Locust checkout was read only. No real game files, live Google requests, or Locust code changes were used.

## Recommendation

**Use an optional CT2 provider for an EN→ES pilot, with Google fallback and strict validation. Keep Google as the default for Japanese dialogue.** Bulk speed is sufficient on this GPU. The quality evidence supports a limited, opt-in EN→ES deployment; it does not establish an unrestricted primary translator for every game string.

Use OPUS EN→ES first for ordinary visible text. Preserve Locust's existing placeholder/glossary restoration and pending-entry fallback: reject changed, missing, added or duplicated controls instead of accepting partial repairs. The `{PL_i}` probe still fails for this model, so masking alone does not make it safe. Engine-aware translation of protected spans is a useful follow-up experiment, with its effect on grammar measured separately.

For JA→EN, OPUS produces an undetectable meaning error on a plain dialogue probe: `先輩、私も一緒に行っていいですか？` (roughly “Senpai, may I come with you?”) becomes `I'm going to go to the hospital.` It also turns the UI command `冒険を続ける` into `We're on a quest.` Google fallback triggered only by exceptions or control validation cannot catch these errors. Retaining Google here is a deployment recommendation; this spike did not measure Google's relative quality or speed.

Treat NLLB 600M/1.3B and direct JA→ES as research options. Their weights are CC-BY-NC-4.0, which prevents assuming they are suitable for a commercial default. Choose a noncommercial NLLB experiment explicitly, with its terms visible; use the measured quality table below to compare directions. A separate game-domain evaluation is still needed before changing the Japanese default.

Keep one persistent translator per loaded model and one GPU worker. Batch 256 gives high bulk throughput in this experiment; batch 64 is a practical starting point for integration with bounded jobs. Validate the current CLI/pipeline batch limits before exposing 256. Prewarm the worker: loading weights and producing the first translation have substantially different costs.

NLLB 1.3B has the best tested JA→EN reference score (55.88 chrF), but its game UI probes still contain domain errors: `セーブデータがありません` (“No save data”) becomes `We don't have any data on the savings.`. It also drops Kenta from the named dialogue probe. Higher general-domain scores therefore support further noncommercial evaluation rather than automatically changing the Japanese default.

## Measurements

Median of three warm passes over 1,000 unique input strings per direction. Greedy decoding (beam 1). Main throughput includes tokenizer, synchronous CT2 inference, and target decoding. chrF is on all 1,012 aligned FLORES-200 devtest rows, scale 0–100.

| Model | Device / compute | Direction | Model load (s) | Strings/s B16 | B64 | B256 | Peak board VRAM / model-load baseline delta (MiB) | chrF |
|---|---|---|---:|---:|---:|---:|---:|---:|
| OPUS JA→EN | cuda / int8_float16 | JA→EN | 0.24 | 71.5 | 181.9 | 486.0 | 3344 / +1058 | 47.93 |
| OPUS EN→ES | cuda / int8_float16 | EN→ES | 0.32 | 68.2 | 209.2 | 559.9 | 3320 / +1018 | 54.77 |
| NLLB 600M | cuda / int8_float16 | JA→EN | 1.98 | 28.0 | 75.0 | 146.1 | 5461 / +3084 | 52.04 |
| NLLB 600M | cuda / int8_float16 | EN→ES | 1.98 | 23.8 | 75.5 | 176.8 | 5723 / +3346 | 54.19 |
| NLLB 600M | cuda / int8_float16 | JA→ES | 1.98 | 28.3 | 67.6 | 165.9 | 5704 / +3327 | 41.91 |
| NLLB 1.3B | cuda / int8_float16 | JA→EN | 2.98 | 15.7 | 47.1 | 126.0 | 8100 / +5864 | 55.88 |
| NLLB 1.3B | cuda / int8_float16 | EN→ES | 2.98 | 14.5 | 31.2 | 72.2 | 8238 / +6001 | 55.12 |
| NLLB 1.3B | cuda / int8_float16 | JA→ES | 2.98 | 11.2 | 26.6 | 53.5 | 8319 / +6082 | 45.18 |
| OPUS EN→ES | cpu / int8_float32 | EN→ES | 0.33 | 11.6 | 17.1 | 26.2 | — | 54.77 |

Model load is process-cold, with the Windows filesystem cache left intact. The same model load is repeated in each direction row for readability. Board VRAM includes the display and other processes; the delta uses the baseline before model load. CPU has no GPU memory allocation attribution.

## Sentence throughput and timing details

Sentence units use a documented punctuation/newline heuristic; every UI label counts as at least one. This is separate from input-string count and is not a linguistic sentence segmenter.

| Model | Device | Direction | Strings / sentence units | Sentences/s B16 | B64 | B256 | Core strings/s B16 / B64 / B256 | First batch 16 (s) | Tokenizer load (s) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| OPUS JA→EN | cuda | JA→EN | 1000 / 1182 | 84.5 | 215.0 | 574.5 | 74.7 / 204.8 / 685.6 | 14.660 | 0.684 |
| OPUS EN→ES | cuda | EN→ES | 1000 / 1241 | 84.6 | 259.6 | 694.9 | 71.2 / 239.9 / 819.8 | 15.935 | 0.863 |
| NLLB 600M | cuda | JA→EN | 1000 / 1182 | 33.1 | 88.6 | 172.7 | 28.1 / 75.7 / 148.7 | 22.260 | 2.209 |
| NLLB 600M | cuda | EN→ES | 1000 / 1241 | 29.5 | 93.8 | 219.5 | 23.9 / 76.4 / 184.9 | 0.165 | 2.209 |
| NLLB 600M | cuda | JA→ES | 1000 / 1182 | 33.5 | 79.9 | 196.0 | 28.4 / 68.2 / 169.2 | 0.273 | 2.209 |
| NLLB 1.3B | cuda | JA→EN | 1000 / 1182 | 18.6 | 55.7 | 148.9 | 15.7 / 47.4 / 127.8 | 17.805 | 2.188 |
| NLLB 1.3B | cuda | EN→ES | 1000 / 1241 | 18.0 | 38.7 | 89.6 | 14.6 / 31.4 / 73.4 | 0.241 | 2.188 |
| NLLB 1.3B | cuda | JA→ES | 1000 / 1182 | 13.3 | 31.4 | 63.3 | 11.3 / 26.7 / 53.9 | 5.131 | 2.188 |
| OPUS EN→ES | cpu | EN→ES | 1000 / 1241 | 14.5 | 21.2 | 32.5 | 11.7 / 17.3 / 26.6 | 0.963 | 3.025 |

## Placeholder probes

Raw-control probes are 20 Japanese and 20 English rows in samples.md, with 10 control-bearing rows in each source language. A failure means the exact multiset of protected tokens/newlines changed. This catches missing, altered, duplicated and added controls, but does not prove correct placement. The canonical-mask probe replaces each control occurrence with a distinct `{PL_i}` token before translation; it is an approximation of the production masking seam, not an execution of Locust.

| Model | Device | Direction | Raw controls broken / tested | Canonical masks broken / tested |
|---|---|---|---:|---:|
| OPUS JA→EN | cuda | JA→EN | 10 / 10 | 9 / 10 |
| OPUS EN→ES | cuda | EN→ES | 7 / 10 | 4 / 10 |
| NLLB 600M | cuda | JA→EN | 7 / 10 | 8 / 10 |
| NLLB 600M | cuda | EN→ES | 9 / 10 | 8 / 10 |
| NLLB 600M | cuda | JA→ES | 8 / 10 | 9 / 10 |
| NLLB 1.3B | cuda | JA→EN | 8 / 10 | 9 / 10 |
| NLLB 1.3B | cuda | EN→ES | 8 / 10 | 8 / 10 |
| NLLB 1.3B | cuda | JA→ES | 10 / 10 | 9 / 10 |
| OPUS EN→ES | cpu | EN→ES | 7 / 10 | 3 / 10 |

## Quality settings

- OPUS JA→EN cuda JA→EN: chrF **47.93**; chrF++ **45.28**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- OPUS EN→ES cuda EN→ES: chrF **54.77**; chrF++ **52.44**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 600M cuda JA→EN: chrF **52.04**; chrF++ **49.45**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 600M cuda EN→ES: chrF **54.19**; chrF++ **51.96**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 600M cuda JA→ES: chrF **41.91**; chrF++ **39.41**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 1.3B cuda JA→EN: chrF **55.88**; chrF++ **53.39**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 1.3B cuda EN→ES: chrF **55.12**; chrF++ **52.91**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- NLLB 1.3B cuda JA→ES: chrF **45.18**; chrF++ **42.62**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.
- OPUS EN→ES cpu EN→ES: chrF **54.77**; chrF++ **52.45**; 1012 rows; decoding-cap hits 0; `nrefs:1|case:mixed|eff:yes|nc:6|nw:0|space:no|version:2.6.0` / `nrefs:1|case:mixed|eff:yes|nc:6|nw:2|space:no|version:2.6.0`.

## Hardware and method

- NVIDIA RTX 5070 Ti, 16,303 MiB reported VRAM, driver 581.80, compute capability 12.0; Windows 11 build 26100.
- AMD Ryzen 7 7700X, 8 physical / 16 logical cores, 63.1 GiB RAM. The final CPU fallback uses 1 intra-thread / one inter-thread worker, INT8 (`int8_float32`) with the wheel's oneDNN backend and selected AVX2 ISA. Its OpenMP environment is recorded in the JSON.
- Python 3.12.14; CTranslate2 4.8.4; Transformers 4.57.6; sacrebleu 2.6.0. Full versions: [requirements.lock.txt](requirements.lock.txt).
- GPU inference actually ran with `int8_float16`. The Windows wheel plus local NVIDIA cuBLAS 12.9.2 worked on Blackwell. A standalone `cudart64_12.dll` probe was not loadable, but this did not prevent CT2 GPU execution. CPU fallback was measured intentionally, not substituted for failed CUDA.
- One process per model/device; tokenizer load and `Translator` construction timed separately. “Cold” means a fresh process/translator with filesystem caches intact. Python import time is excluded. First-batch timings capture lazy runtime initialization and matter for startup UX.
- Greedy beam 1, maximum input 512 tokens, maximum output 256 tokens, no source truncation, one input string per translation request. A complete untimed pass at each batch size precedes three timed full passes; the table uses medians. Output cap hits and every repeat are retained in the JSON. A shared compute lock serializes model conversion and benchmark processes.
- Main speed includes tokenization, synchronous inference and decoding. The detailed table also reports inference-only speed. Batch inputs retain corpus order; CT2 may sort within a submitted batch. No translation-result cache is used.
- NVML samples board memory every 20 ms. Display/other-process use is included; the table gives the total peak and its increase over the pre-load baseline, rather than claiming exact process-only VRAM. Short spikes between samples can be missed. No clock/power pinning or exclusive-machine guarantee was used; all three timings are available for inspecting variability.

## Data and limits

The benchmark has **1,000 unique Japanese and 1,000 unique English strings**. The synthetic subset is 47 Japanese and 29 English strings from Locust's RenPy fixtures, pipeline fixture triples, multilingual/ RGSS synthetic tables, and a manually filtered set of Rust test literals. Paths, diagnostics, real-copy helpers and game binaries were excluded. The remaining **953 Japanese / 971 English** strings are public FLORES-200 devtest text. This is predominantly general-domain prose, not a 1,000-row game dialogue corpus.

Mean source lengths: Japanese 54.4 characters (median 53); English 127.6 characters (median 124). These throughput figures should not be presented as a full-game benchmark. No context, speaker history or game glossary was provided to the models. EN→ES used native English source text rather than noisy JA→EN pivot output; composed pipeline quality was not measured.

FLORES quality uses all 1,012 aligned Japanese/English/Spanish devtest rows, downloaded from Meta's public archive. chrF uses character order 6, beta 2, no word-order component; chrF++ is also shown for comparison with FLORES conventions. Both use a 0–100 scale. This is an automatic reference metric, not a percentage of correct sentences. References and benchmark prose overlap intentionally; this is an inference spike, not training or model selection on held-out game text. [FLORES-200 documentation](https://github.com/facebookresearch/flores/blob/main/flores200/README.md)

[samples.md](samples.md) contains exactly 40 source rows: 20 Japanese and 20 English, mixing fixture UI, dialogue, names/honorifics and authored control probes. Each supported model/direction is shown side by side and failures are marked. The canonical-mask experiment is retained in the result JSON. Its literal-multiset check does not prove correct control position, tag nesting, honorific meaning or translation quality. The small, deliberately difficult probe set does not estimate a production fallback percentage.

Every referenced source fixture has a stored SHA-256 and path/line provenance. Verification rechecked 109 source files; observed changes since extraction: 1. Saved inputs remain the benchmark source of truth. See [data/summary.json](data/summary.json), [data/flores-provenance.json](data/flores-provenance.json), and [verification.json](verification.json).

The end-of-run hash differs for `crates/core/tests/translation_consistency.rs`. This candidate file contributed 0 rows to the retained workloads. The spike only read the Locust checkout and did not regenerate benchmark inputs after that drift. Both hashes are retained in verification.json.

Long-output flags in the timed mixed workloads (hypotheses of at least 255 tokens, maximum decoding length 256): nllb600 ja-en: 6 across the timed repeats; nllb1300 ja-es: 9 across the timed repeats. These are repeated observations, not unique-source counts. Every full FLORES quality pass had zero such flags. Treat length-limit/repetition cases as failures in a provider adapter.

## Licenses and model provenance

| Component | Declared license / provenance | Deployment implication |
|---|---|---|
| Helsinki-NLP/opus-mt-ja-en | Apache-2.0 in the current upstream card; converted locally | Candidate weights for an optional distributable provider; preserve applicable notices |
| Helsinki-NLP/opus-mt-en-es | Apache-2.0 in the current upstream card; converted locally | First candidate for the EN→ES pilot |
| facebook/nllb-200-distilled-600M and distilled-1.3B | CC-BY-NC-4.0; ready INT8 exports by JustFrederik, original Meta tokenizers | Research / noncommercial route; do not silently bundle as an unrestricted commercial model |
| CTranslate2 / ct2rs | MIT | Preserve notices for the runtime/bindings and selected native dependencies |
| NVIDIA cuBLAS / NVRTC packages | NVIDIA CUDA license terms and component notices | Package redistributable runtime components under their own terms; the MIT bindings license does not relicense these DLLs |
| Original FLORES-200 sample | CC-BY-SA-4.0 | Keep attribution and dataset terms with retained sample data |

The two OPUS weight-card declarations were checked directly: [JA→EN card](https://huggingface.co/Helsinki-NLP/opus-mt-ja-en), [EN→ES card](https://huggingface.co/Helsinki-NLP/opus-mt-en-es). Generic OPUS software licenses or a third-party export's metadata are not substituted for those model-card declarations.

NVIDIA lists cuBLAS and NVRTC among CUDA redistributable components, with the applicable terms and notices in the [CUDA EULA](https://docs.nvidia.com/cuda/eula/index.html). This packaging layer is separate from model and binding licenses.

The [NLLB model card](https://huggingface.co/facebook/nllb-200-distilled-600M) identifies its noncommercial terms and research focus, including limited domain evaluation and a 512-token training length. Both NLLB sizes here use pinned [600M](https://huggingface.co/JustFrederik/nllb-200-distilled-600M-ct2-int8) / [1.3B](https://huggingface.co/JustFrederik/nllb-200-distilled-1.3B-ct2-int8) CT2 exports. The original multi-GB FP32 NLLB checkpoints were not downloaded or independently reconverted; all downloaded weight files were checked against the Hub's SHA-256 metadata. Exact revisions and hashes are in [models.json](models.json) and [model-metadata.json](model-metadata.json).

## Proposed Rust integration

**Proceed behind an optional Cargo feature, initially exposing only the approved model/direction.** This spike validates Python CT2 inference; it does not validate a Windows Rust build or prove that different native build features reproduce the same performance. [ct2rs](https://docs.rs/ct2rs/latest/ct2rs/) exposes Translator bindings and lists Marian-MT and NLLB among tested architectures.

A feature layout to evaluate in a separate implementation change is:

```toml
[dependencies]
ct2rs = { version = "0.10.2", optional = true, default-features = false, features = ["all-tokenizers", "ruy"] }

[features]
nmt = ["dep:ct2rs"]
nmt-cuda = ["nmt", "ct2rs/cuda", "ct2rs/cuda-dynamic-loading"]
```

This is a proposal, not a compiled manifest. The CPU RUY feature above differs from this Python wheel's measured oneDNN backend; CPU performance needs a native recheck, or select a matching backend deliberately. The crate documents tokenizer/backend feature selection and MIT licensing in its [README](https://github.com/jkawamoto/ctranslate2-rs).

Integration needs:

1. **Model management:** explicit download/install/remove UI, pinned revisions, expected sizes and SHA-256, resumable downloads, atomic completion markers, visible weight licenses and model versions. Package CT2 weights and matching vocabulary/tokenizer artifacts together. The missing `.txt` vocabulary caught in this spike is an example of why model.bin alone is insufficient. Record the chosen model in translation provenance.
2. **Tokenization:** Marian needs the exact source and target SentencePiece models, vocabulary and EOS behavior. NLLB needs the matching tokenizer and source language code (`jpn_Jpan` / `eng_Latn`), the forced target prefix (`eng_Latn` / `spa_Latn`) and special-token removal. Match known token IDs and translations against this harness before integration. These rules are described in CT2's [Transformers guide](https://opennmt.net/CTranslate2/guides/transformers.html).
3. **Provider behavior:** implement the existing `TranslationProvider` contract in `crates/providers`; preserve entry IDs/order and group requests by supported pair. Forward the optional feature through the relevant CLI/server/desktop builds and register the provider conditionally. Run blocking inference on a bounded worker, keep translators alive and respect cancellation between batches. Enforce model input limits before dispatch. NMT cannot interpret freeform context, character notes or glossary instructions as an LLM prompt; use explicit protection/term logic or route unsupported cases to fallback. The current Python pipeline selects one primary for both phases; Google JA→EN plus CT2 EN→ES needs explicit phase-specific configuration, validation and manifest attribution.
4. **Existing validation/fallback:** retain `translation/manager.rs` and `translation/protection.rs` checks. `run_fallback_chain` already retries remaining pending entries rather than overwriting successful rows. Report missing models, unsupported pairs, CUDA/OOM errors and damaged protected tokens through that seam. A semantic mistranslation remains a successful-looking result, which is why language/domain routing and review are still required.
5. **Windows native build and packaging:** validate the selected ct2rs/CTranslate2 version with MSVC, CMake and a Blackwell-capable CUDA toolkit/compiler; target SM 12.0 and compatible PTX deliberately. `cuda-dynamic-loading` delays CUDA-library loading but does not remove the native CUDA build requirement. Ship or discover compatible cuBLAS DLLs with a controlled search path, verify driver/runtime requirements and clean-machine behavior, and test CRT/linkage choices. The Python wheel did not require a full toolkit installation, so its success does not settle the Rust build. See [CT2 installation/build options](https://opennmt.net/CTranslate2/installation.html) and the crate's [build script](https://docs.rs/crate/ct2rs/latest/source/build.rs).

## Installation, recoveries and reproducibility

Exact successful and failed installation commands, conversion commands and execution commands are in [INSTALL.md](INSTALL.md). Re-run with `env.ps1`, pinned package versions and the retained inputs. `CARGO_TARGET_DIR` remained `C:\Projects\Locust-nmt-spike\cargo-target`; no Cargo build or alternate target directory was created.

Recovered issues: uv initially tried to create a Python link outside the workspace; package launcher generation hit a denied default temp directory; one reinstall overlapped a loaded `.pyd`; standard large Hub downloads stalled; one tokenizer cache-directory operation was denied; the first NLLB artifact allow-list omitted `shared_vocabulary.txt`. Local uv/temp paths, a retry after the inspection exited, pinned HTTP range downloads, direct small-file downloads and the corrected vocabulary allow-list resolved these issues. Incomplete duplicate download files were removed after the final weight hashes were verified; their sizes/paths remain in [discarded-downloads.json](discarded-downloads.json).

The initial OPUS warm-up covered one batch. Their exploratory results are retained under `results/exploratory/`; only the full-pass warm-up runs appear in the main tables. No CUDA inference failure forced CPU substitution. Beam-search quality/speed, live Google comparison, actual game files, a Rust build and production fallback rates were outside this measurement.

The initial eight-thread CPU attempt was interrupted after several minutes without completing its first full pass, with host CPU utilization observed at 100%. A separate 16-sentence control probe with passive OpenMP waiting ran faster at one thread than four (about 9 versus 4.6 strings/s on the second pass). The full CPU table therefore uses one thread with `OMP_NUM_THREADS=1`, `MKL_NUM_THREADS=1`, `OPENBLAS_NUM_THREADS=1`, `KMP_BLOCKTIME=0`, and `OMP_WAIT_POLICY=PASSIVE`. This is the measured configuration, not a claim about the CPU's maximum possible performance. See `logs/cpu-8-threads-interrupted.log`, `cpu-probe-1.json`, and `cpu-probe-4.json`.

Raw timings, translated workloads, aligned quality outputs and probe results are under [results/](results/); console logs are under [logs/](logs/). [verification.json](verification.json) independently recomputes quality and checks all nine model/device/direction rows, three timed repeats per batch size, input/output alignment and control counters.

## Disk usage

**5.904 GiB (6,339,188,003 logical bytes)** for the entire scratch directory, including installed Python, venv, uv cache, original Marian checkpoints, converted/exported models, tokenizer files and results. This is logical file size, not allocated filesystem clusters. No automatic cleanup of the reusable environment/cache was performed.

| Directory | GiB |
|---|---:|
| `models` | 2.024 |
| `.venv` | 1.612 |
| `uv-cache` | 1.569 |
| `source-models` | 0.579 |
| `python` | 0.061 |
| `tokenizers` | 0.041 |
| `results` | 0.016 |

CT2 model binaries alone total 2.011 GiB. Verified downloaded weight binaries total 2.620 GB, plus tokenizer/configuration files. Interrupted duplicate partials consumed another 0.608 GB before removal, so model transfer exceeded the nominal 2–3 GB estimate through retries. Full network byte accounting was not recorded. The two original Marian checkpoints are retained for reproducible conversion. The detailed account is in [disk-usage.json](disk-usage.json).

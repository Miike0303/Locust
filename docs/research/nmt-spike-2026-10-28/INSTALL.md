# Commands used for this spike

Working directory: `C:\Projects\Locust-nmt-spike`. PowerShell commands below are the actual installation commands, including retries. `CARGO_TARGET_DIR` was inherited unchanged; no Cargo build was run.

## Initial attempt and directory correction

```powershell
$env:UV_CACHE_DIR = 'C:\Projects\Locust-nmt-spike\uv-cache'
$env:UV_PYTHON_INSTALL_DIR = 'C:\Projects\Locust-nmt-spike\python'
$env:HF_HOME = 'C:\Projects\Locust-nmt-spike\hf-cache'
uv venv --python 3.12 .venv
```

Python downloaded, but creation of a minor-version link outside the workspace was denied. Retried with:

```powershell
$env:UV_PYTHON_BIN_DIR = 'C:\Projects\Locust-nmt-spike\python-bin'
uv venv --python 3.12 .venv
uv pip install --python .venv\Scripts\python.exe ctranslate2 'transformers>=4.45,<5' sentencepiece huggingface_hub sacrebleu torch psutil nvidia-ml-py
```

The venv succeeded. Package installation partially completed, then Windows denied updating an executable launcher in the default sandbox temporary directory.

## Successful package installation

`env.ps1` sets the same local directories above, `UV_LINK_MODE=copy`, `TEMP` and `TMP` to the workspace's `tmp` directory, Python UTF-8/unbuffered output, and Hugging Face/tokenizer telemetry settings.

```powershell
. .\env.ps1
uv pip install --python .venv\Scripts\python.exe ctranslate2 'transformers>=4.45,<5' sentencepiece huggingface_hub sacrebleu torch psutil nvidia-ml-py nvidia-cublas-cu12
```

`nvidia-cublas-cu12` also installed `nvidia-cuda-nvrtc-cu12`. To restore console entry points left incomplete by the first attempt:

```powershell
uv pip install --python .venv\Scripts\python.exe --reinstall-package ctranslate2 --reinstall-package sacrebleu --reinstall-package tabulate ctranslate2 sacrebleu tabulate
```

The first execution of this last command overlapped an environment inspection and could not replace a loaded CT2 `.pyd`. The identical command succeeded after that inspection exited. No elevation or system CUDA installation was needed for the Python wheel.

Added Marian's recommended punctuation normalizer and recorded the environment:

```powershell
uv pip install --python .venv\Scripts\python.exe sacremoses
uv pip freeze --python .venv\Scripts\python.exe | Set-Content -LiteralPath requirements.lock.txt -Encoding utf8
uv pip check --python .venv\Scripts\python.exe
```

The final dependency check reported 39 compatible installed packages.

## Model provisioning and data

```powershell
. .\env.ps1
.venv\Scripts\python.exe inspect_environment.py
.venv\Scripts\python.exe provision.py metadata
.venv\Scripts\python.exe prepare_data.py
.venv\Scripts\python.exe provision.py nllb600
.venv\Scripts\python.exe provision.py opus-en-es
.venv\Scripts\python.exe provision.py opus-ja-en
.venv\Scripts\python.exe provision.py nllb1300
```

`provision.py` pins repositories to the commits in `model-metadata.json`. Standard Hugging Face downloads of large weights made little visible progress and were interrupted; their incomplete files were retained until the final copies were verified, then removed. The final downloader uses exact 16 MiB HTTP ranges, validates the response range and complete-file SHA-256 against the Hub's LFS metadata, and saves everything here. Original NLLB FP32 weights were not downloaded. Most configuration/tokenizer files initially used `snapshot_download`; a transient permission failure creating its local `.cache/huggingface` directory was recovered by switching the provisioner to direct pinned URLs for small files and ranged requests for LFS files at least 1 MB. Existing large files are hash-verified on resume.

The Marian conversions are launched by `provision.py` with these exact argument values (absolute paths resolved from this directory):

```powershell
.venv\Scripts\ct2-transformers-converter.exe --model C:\Projects\Locust-nmt-spike\source-models\opus-en-es --output_dir C:\Projects\Locust-nmt-spike\models\opus-en-es --quantization int8_float16 --copy_files source.spm target.spm vocab.json tokenizer_config.json
.venv\Scripts\ct2-transformers-converter.exe --model C:\Projects\Locust-nmt-spike\source-models\opus-ja-en --output_dir C:\Projects\Locust-nmt-spike\models\opus-ja-en --quantization int8_float16 --copy_files source.spm target.spm vocab.json tokenizer_config.json
```

The NLLB exports are stored INT8; the GPU loads them with `compute_type="int8_float16"`. The CPU benchmark explicitly uses `compute_type="int8"`.

## Benchmarks

```powershell
. .\env.ps1
.venv\Scripts\python.exe benchmark.py --model nllb600 --smoke
.\run_benchmarks.ps1
```

The runner's individual benchmark commands are:

```powershell
.venv\Scripts\python.exe benchmark.py --model opus-en-es --device cuda
.venv\Scripts\python.exe benchmark.py --model opus-ja-en --device cuda
.venv\Scripts\python.exe benchmark.py --model nllb600 --device cuda
.venv\Scripts\python.exe benchmark.py --model nllb1300 --device cuda
.venv\Scripts\python.exe benchmark.py --model opus-en-es --device cpu --threads 1
```

The first standalone OPUS runs warmed only one batch. Their results are preserved under `results/exploratory/`. The final runner uses a full untimed 1,000-string pass at every batch size before the three measured passes. Models and conversion processes share a compute lock; the final benchmark suite runs sequentially. The queued NLLB smoke command was interrupted before it ran because provisioning was not complete; the full NLLB benchmark performs the actual validation instead.

The initial CPU command used `--threads 8`; it was interrupted after several minutes without completing its first full pass. These diagnostic commands compared one and four threads on 16 FLORES sentences:

```powershell
.venv\Scripts\python.exe cpu_probe.py
.venv\Scripts\python.exe cpu_probe.py --threads 4
```

The final complete CPU benchmark uses the faster one-thread configuration from that probe. Its exact command sequence was:

```powershell
$env:OMP_NUM_THREADS = '1'
$env:MKL_NUM_THREADS = '1'
$env:OPENBLAS_NUM_THREADS = '1'
$env:KMP_BLOCKTIME = '0'
$env:OMP_WAIT_POLICY = 'PASSIVE'
.venv\Scripts\python.exe benchmark.py --model opus-en-es --device cpu --threads 1
```

The final runner applies those CPU settings automatically after the GPU runs. They are recorded in the CPU result JSON. The interrupted eight-thread console output remains in `logs/cpu-8-threads-interrupted.log`.

The first NLLB load reported a missing target vocabulary. The export uses `shared_vocabulary.txt`; the original allow-list selected only JSON vocabularies. After including both formats, these same provisioning commands restored the artifact and the runner resumed, retaining completed OPUS runs:

```powershell
.venv\Scripts\python.exe provision.py nllb600
.venv\Scripts\python.exe provision.py nllb1300
.\run_benchmarks.ps1
```

Report generation and independent verification:

```powershell
.venv\Scripts\python.exe summarize.py
.venv\Scripts\python.exe verify_results.py
.venv\Scripts\python.exe disk_usage.py
.venv\Scripts\python.exe build_report.py
```

Use `requirements.lock.txt` for the resolved package versions. `runtime.py` adds the venv's NVIDIA DLL directories to the current process DLL search path. `HF_HOME` is always local, even when scripts are run without sourcing `env.ps1`.

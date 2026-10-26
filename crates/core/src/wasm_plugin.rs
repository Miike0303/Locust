//! WASM plugin support for external format plugins.
//!
//! Requires the `wasm-plugins` feature flag.

#[cfg(any(feature = "wasm-plugins", test))]
fn log_range(ptr: i32, len: i32, mem_len: usize) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(ptr).ok()?;
    let len = usize::try_from(len).ok()?;
    let end = start.checked_add(len)?;
    (end <= mem_len).then_some(start..end)
}

#[cfg(feature = "wasm-plugins")]
mod inner {
    use std::path::Path;

    use serde::Deserialize;
    use wasmtime::*;

    use crate::error::{LocustError, Result};
    use crate::extraction::{FormatPlugin, InjectionReport};
    use crate::models::StringEntry;

    #[derive(Debug, Clone, Deserialize)]
    pub struct WasmPluginMetadata {
        pub id: String,
        pub name: String,
        #[serde(default)]
        pub description: String,
        #[serde(default)]
        pub version: String,
        pub extensions: Vec<String>,
        #[serde(default)]
        pub author: String,
    }

    pub struct WasmPlugin {
        engine: Engine,
        module: Module,
        pub metadata: WasmPluginMetadata,
    }

    impl WasmPlugin {
        fn create_instance(&self) -> Result<(Store<()>, Instance)> {
            let mut store = Store::new(&self.engine, ());
            let mut linker = Linker::new(&self.engine);

            // Provide host import: locust_log
            linker
                .func_wrap(
                    "env",
                    "locust_log",
                    |mut caller: Caller<'_, ()>, ptr: i32, len: i32| {
                        if let Some(memory) =
                            caller.get_export("memory").and_then(|e| e.into_memory())
                        {
                            let data = memory.data(&caller);
                            if let Some(range) = super::log_range(ptr, len, data.len()) {
                                if let Ok(msg) = std::str::from_utf8(&data[range]) {
                                    tracing::info!("[wasm-plugin] {}", msg);
                                }
                            }
                        }
                    },
                )
                .map_err(LocustError::Other)?;

            let instance = linker
                .instantiate(&mut store, &self.module)
                .map_err(LocustError::Other)?;

            Ok((store, instance))
        }

        fn read_wasm_string(store: &Store<()>, memory: &Memory, ptr: i32) -> Result<String> {
            let data = memory.data(store);
            let start = ptr as usize;

            // Read length prefix (first 4 bytes, little-endian)
            if start + 4 > data.len() {
                return Err(LocustError::Other(anyhow::anyhow!(
                    "invalid WASM string pointer"
                )));
            }
            let len = u32::from_le_bytes([
                data[start],
                data[start + 1],
                data[start + 2],
                data[start + 3],
            ]) as usize;

            let str_start = start + 4;
            let str_end = str_start + len;
            if str_end > data.len() {
                return Err(LocustError::Other(anyhow::anyhow!(
                    "WASM string out of bounds"
                )));
            }

            String::from_utf8(data[str_start..str_end].to_vec())
                .map_err(|e| LocustError::Other(e.into()))
        }

        fn write_wasm_string(
            store: &mut Store<()>,
            instance: &Instance,
            s: &str,
        ) -> Result<(i32, i32)> {
            let alloc = instance
                .get_typed_func::<i32, i32>(&mut *store, "locust_alloc")
                .map_err(LocustError::Other)?;

            let bytes = s.as_bytes();
            let total_len = 4 + bytes.len();
            let ptr = alloc
                .call(&mut *store, total_len as i32)
                .map_err(LocustError::Other)?;

            let memory = instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| LocustError::Other(anyhow::anyhow!("no memory export")))?;

            let data = memory.data_mut(&mut *store);
            let start = ptr as usize;
            data[start..start + 4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            data[start + 4..start + 4 + bytes.len()].copy_from_slice(bytes);

            Ok((ptr, total_len as i32))
        }
    }

    impl FormatPlugin for WasmPlugin {
        fn id(&self) -> &str {
            &self.metadata.id
        }

        fn name(&self) -> &str {
            &self.metadata.name
        }

        fn description(&self) -> &str {
            &self.metadata.description
        }

        fn supported_extensions(&self) -> &[&str] {
            // This is a limitation — we can't return &[&str] from owned data easily
            // Use a static leak for the lifetime (acceptable for plugins loaded once)
            // Instead, we override detect() to avoid needing this
            &[]
        }

        fn detect(&self, path: &Path) -> bool {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_lowercase();
                self.metadata.extensions.iter().any(|supported| {
                    let s = supported.strip_prefix('.').unwrap_or(supported);
                    s.to_lowercase() == ext_lower
                })
            } else {
                false
            }
        }

        fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
            let (mut store, instance) = self.create_instance()?;

            let path_str = path.to_string_lossy().to_string();
            let (path_ptr, path_len) = Self::write_wasm_string(&mut store, &instance, &path_str)?;

            let extract_fn = instance
                .get_typed_func::<(i32, i32), i32>(&mut store, "locust_extract")
                .map_err(LocustError::Other)?;

            let result_ptr = extract_fn
                .call(&mut store, (path_ptr, path_len))
                .map_err(LocustError::Other)?;

            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| LocustError::Other(anyhow::anyhow!("no memory export")))?;

            let json_str = Self::read_wasm_string(&store, &memory, result_ptr)?;
            let entries: Vec<StringEntry> = serde_json::from_str(&json_str)?;

            Ok(entries)
        }

        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let (mut store, instance) = self.create_instance()?;

            let path_str = path.to_string_lossy().to_string();
            let entries_json = serde_json::to_string(entries)?;

            let (path_ptr, path_len) = Self::write_wasm_string(&mut store, &instance, &path_str)?;
            let (entries_ptr, entries_len) =
                Self::write_wasm_string(&mut store, &instance, &entries_json)?;

            let inject_fn = instance
                .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "locust_inject")
                .map_err(LocustError::Other)?;

            let result_ptr = inject_fn
                .call(&mut store, (path_ptr, path_len, entries_ptr, entries_len))
                .map_err(LocustError::Other)?;

            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| LocustError::Other(anyhow::anyhow!("no memory export")))?;

            let json_str = Self::read_wasm_string(&store, &memory, result_ptr)?;
            let report: InjectionReport = serde_json::from_str(&json_str)?;

            Ok(report)
        }
    }

    pub fn load_wasm_plugin(path: &Path) -> Result<WasmPlugin> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, path).map_err(|e| {
            LocustError::Other(anyhow::anyhow!("failed to load WASM module: {}", e))
        })?;

        // Create a temporary instance to read metadata
        let mut store = Store::new(&engine, ());
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap("env", "locust_log", |_: Caller<'_, ()>, _: i32, _: i32| {})
            .map_err(LocustError::Other)?;

        let instance = linker.instantiate(&mut store, &module).map_err(|e| {
            LocustError::Other(anyhow::anyhow!("failed to instantiate WASM: {}", e))
        })?;

        let metadata_fn = instance
            .get_typed_func::<(), i32>(&mut store, "locust_plugin_metadata")
            .map_err(|e| {
                LocustError::Other(anyhow::anyhow!("missing locust_plugin_metadata: {}", e))
            })?;

        let meta_ptr = metadata_fn
            .call(&mut store, ())
            .map_err(LocustError::Other)?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| LocustError::Other(anyhow::anyhow!("no memory export")))?;

        let meta_json = WasmPlugin::read_wasm_string(&store, &memory, meta_ptr)?;
        let metadata: WasmPluginMetadata = serde_json::from_str(&meta_json)?;

        tracing::info!(
            "Loaded WASM plugin: {} v{}",
            metadata.name,
            metadata.version
        );

        Ok(WasmPlugin {
            engine,
            module,
            metadata,
        })
    }

    pub fn scan_plugin_dir(dir: &Path) -> Result<Vec<WasmPlugin>> {
        let mut plugins = Vec::new();

        if !dir.exists() {
            return Ok(plugins);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wasm") {
                match load_wasm_plugin(&path) {
                    Ok(plugin) => {
                        tracing::info!("Registered WASM plugin: {}", plugin.metadata.id);
                        plugins.push(plugin);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load WASM plugin {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(plugins)
    }
}

#[cfg(feature = "wasm-plugins")]
pub use inner::*;

// Stubs when wasm-plugins feature is not enabled
#[cfg(not(feature = "wasm-plugins"))]
mod stubs {
    use crate::error::Result;
    use std::path::Path;

    pub fn scan_plugin_dir(_dir: &Path) -> Result<Vec<()>> {
        Ok(Vec::new())
    }
}

#[cfg(not(feature = "wasm-plugins"))]
pub use stubs::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn wasm_log_range_rejects_negative_values() {
        for (ptr, len) in [(-1, 1), (0, -1), (-1, -1), (1, -1)] {
            assert_eq!(log_range(ptr, len, 16), None, "({ptr}, {len})");
        }
    }

    #[test]
    fn wasm_log_range_bounds_large_sums() {
        assert_eq!(log_range(i32::MAX, i32::MAX, 16), None);
        // The sum overflows i32 but fits usize, even on a 32-bit host.
        let start = i32::MAX as usize;
        assert_eq!(
            log_range(i32::MAX, i32::MAX, usize::MAX),
            Some(start..start * 2)
        );
    }

    #[test]
    fn wasm_log_range_rejects_out_of_bounds() {
        for (ptr, len) in [(4, 13), (17, 0), (i32::MAX, 0)] {
            assert_eq!(log_range(ptr, len, 16), None, "({ptr}, {len})");
        }
    }

    #[test]
    fn wasm_log_range_accepts_valid_and_empty_ranges() {
        assert_eq!(log_range(2, 4, 16), Some(2..6));
        assert_eq!(log_range(0, 16, 16), Some(0..16));
        assert_eq!(log_range(16, 0, 16), Some(16..16));
        assert_eq!(log_range(0, 0, 0), Some(0..0));
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("locust_wasm_")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn test_scan_plugin_dir_finds_wasm() {
        // Without actual WASM files, this just tests the scanning logic
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        // Create a dummy .wasm file (invalid, but tests the scanning)
        fs::write(dir.join("test.wasm"), b"not a real wasm").unwrap();

        #[cfg(feature = "wasm-plugins")]
        {
            let result = scan_plugin_dir(&dir);
            // Should not crash, but the invalid WASM file will be skipped
            assert!(result.is_ok());
            assert!(result.unwrap().is_empty()); // invalid WASM skipped
        }

        #[cfg(not(feature = "wasm-plugins"))]
        {
            let result = scan_plugin_dir(&dir);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_scan_plugin_dir_skips_invalid() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        // Create files with wrong extensions
        fs::write(dir.join("readme.txt"), "not a plugin").unwrap();
        fs::write(dir.join("data.json"), "{}").unwrap();
        // Create an invalid .wasm file
        fs::write(dir.join("broken.wasm"), b"\x00\x61\x73\x6d").unwrap();

        #[cfg(feature = "wasm-plugins")]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }

        #[cfg(not(feature = "wasm-plugins"))]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        #[cfg(feature = "wasm-plugins")]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }
        #[cfg(not(feature = "wasm-plugins"))]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let dir = PathBuf::from("/nonexistent/plugin/dir");
        #[cfg(feature = "wasm-plugins")]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }
        #[cfg(not(feature = "wasm-plugins"))]
        {
            let result = scan_plugin_dir(&dir).unwrap();
            assert!(result.is_empty());
        }
    }

    #[cfg(not(feature = "wasm-plugins"))]
    #[test]
    fn feature_off_scan_is_inert_even_for_a_file() {
        let dir = tempdir();
        let path = dir.path().join("plugin.wasm");
        let bytes = b"\0asm\x01\0\0\0";
        fs::write(&path, bytes).unwrap();
        assert!(scan_plugin_dir(&path).unwrap().is_empty());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    // WAT is accepted by Wasmtime's existing default `wat` feature. These guests
    // have no loops, WASI, external SDK, filesystem access or timing assertions.
    #[cfg(feature = "wasm-plugins")]
    mod contracts {
        use super::*;
        use crate::extraction::{FormatPlugin, FormatStability};
        use crate::models::{OutputMode, StringEntry};

        const META: &str =
            r#"{"id":"fixture","name":"Fixture 日本語","extensions":[".故事","TXT"]}"#;

        fn escaped(bytes: &[u8]) -> String {
            bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
        }

        fn data(ptr: usize, bytes: &[u8]) -> String {
            let mut prefixed = (bytes.len() as u32).to_le_bytes().to_vec();
            prefixed.extend_from_slice(bytes);
            assert!(ptr + prefixed.len() <= 32768, "fixture data overlaps heap");
            format!("(data (i32.const {ptr}) \"{}\")", escaped(&prefixed))
        }

        fn fixture(metadata: &[u8], operations: &str) -> String {
            format!(
                r#"(module
                    (import "env" "locust_log" (func $log (param i32 i32)))
                    (memory (export "memory") 1)
                    (global $heap (mut i32) (i32.const 32768))
                    {}
                    (func (export "locust_plugin_metadata") (result i32) i32.const 0)
                    (func (export "locust_alloc") (param $size i32) (result i32)
                        (local $ptr i32)
                        global.get $heap local.tee $ptr
                        local.get $size i32.add global.set $heap
                        local.get $ptr)
                    {operations})"#,
                data(0, metadata)
            )
        }

        fn load(dir: &tempfile::TempDir, name: &str, wat: &str) -> WasmPlugin {
            let path = dir.path().join(name);
            fs::write(&path, wat).unwrap();
            load_wasm_plugin(&path).unwrap()
        }

        // The guest refuses any mismatch in the public ABI: byte length (not
        // character count), little-endian prefix, payload and total allocation.
        // Comparing both arguments also detects overlapping host allocations.
        fn check_string(ptr: &str, len: &str, value: &str) -> String {
            let mut wat = format!(
                "local.get ${len} i32.const {} i32.ne if unreachable end\n\
                 local.get ${ptr} i32.load i32.const {} i32.ne if unreachable end\n",
                value.len() + 4,
                value.len()
            );
            for (offset, byte) in value.bytes().enumerate() {
                wat.push_str(&format!(
                    "local.get ${ptr} i32.load8_u offset={} i32.const {byte} \
                     i32.ne if unreachable end\n",
                    offset + 4
                ));
            }
            wat
        }

        #[test]
        fn public_roundtrip_preserves_unicode_identity_and_reports() {
            let dir = tempdir();
            let path = dir.path().join("会話.故事");
            fs::write(&path, b"unchanged host input").unwrap();
            let mut expected: Vec<StringEntry> = serde_json::from_value(serde_json::json!([
                {
                    "id": "scene#1", "source": "同じ台詞 🦀\n{hero}",
                    "translation": null, "file_path": path, "context": "First speaker",
                    "tags": ["dialogue"], "metadata": {"slot": 1}, "status": "pending",
                    "provider_used": null, "char_limit": 80,
                    "created_at": "2026-01-01T00:00:00Z", "translated_at": null,
                    "reviewed_at": null
                },
                {
                    "id": "scene#2", "source": "同じ台詞 🦀\n{hero}",
                    "translation": null, "file_path": path, "context": "Second speaker",
                    "tags": [], "metadata": {}, "status": "pending",
                    "provider_used": null, "char_limit": null,
                    "created_at": "2026-01-01T00:00:00Z", "translated_at": null,
                    "reviewed_at": null
                }
            ]))
            .unwrap();
            let extracted_json = serde_json::to_string(&expected).unwrap();
            expected[0].translation = Some("¡Hola, héroe! 🦀\n{hero}".into());
            let injected_json = serde_json::to_string(&expected).unwrap();
            let report_json = serde_json::json!({
                "files_modified": 1, "strings_written": 1, "strings_skipped": 1,
                "warnings": ["Fixture report only; no host file was written."],
                "files_written": ["salida/会話.故事"], "skip_reasons": {"untranslated": 1}
            })
            .to_string();
            let operations = format!(
                r#"{} {}
                    (func (export "locust_extract") (param $path i32) (param $path_len i32)
                        (result i32)
                        {}
                        local.get $path i32.const 4 i32.add
                        local.get $path i32.load call $log
                        i32.const -1 i32.const 1 call $log
                        i32.const 0 i32.const -1 call $log
                        i32.const 65535 i32.const 2 call $log
                        i32.const 1024)
                    (func (export "locust_inject")
                        (param $path i32) (param $path_len i32)
                        (param $entries i32) (param $entries_len i32) (result i32)
                        {} {} i32.const 8192)"#,
                data(1024, extracted_json.as_bytes()),
                data(8192, report_json.as_bytes()),
                check_string("path", "path_len", path.to_str().unwrap()),
                check_string("path", "path_len", path.to_str().unwrap()),
                check_string("entries", "entries_len", &injected_json)
            );
            let wasm = load(
                &dir,
                "positive.wasm",
                &fixture(META.as_bytes(), &operations),
            );
            let plugin: &dyn FormatPlugin = &wasm;
            assert_eq!(plugin.id(), "fixture");
            assert_eq!(plugin.name(), "Fixture 日本語");
            assert_eq!(plugin.description(), "");
            assert!(plugin.supported_extensions().is_empty()); // detect is overridden
            assert_eq!(plugin.supported_modes(), vec![OutputMode::Replace]);
            assert_eq!(plugin.stability(), FormatStability::Stable);
            assert!(plugin.detect(&path));
            assert!(plugin.detect(std::path::Path::new("UPPER.tXt")));
            assert!(!plugin.detect(std::path::Path::new("other.bin")));
            assert!(!plugin.detect(std::path::Path::new("no_extension")));
            assert!(plugin.validate_source(&path).is_ok());
            assert!(plugin.inject_add(&path, "es", &[]).is_err());

            let extracted = plugin.extract_with_diagnostics(&path).unwrap();
            assert!(extracted.complete);
            assert!(extracted.warnings.is_empty());
            assert_eq!(
                serde_json::to_string(&extracted.entries).unwrap(),
                extracted_json
            );
            let mut entries = extracted.entries;
            entries[0].translation = expected[0].translation.clone();
            let report = plugin.inject(&path, &entries).unwrap();
            assert_eq!(
                serde_json::to_value(report).unwrap(),
                serde_json::from_str::<serde_json::Value>(&report_json).unwrap()
            );
            assert_eq!(
                serde_json::to_string(&plugin.extract(&path).unwrap()).unwrap(),
                extracted_json
            );
            assert_eq!(fs::read(&path).unwrap(), b"unchanged host input");
            // The scanner uses the same loader, skips invalid .wasm and ignores
            // non-.wasm files; it must still return the one valid guest.
            fs::write(dir.path().join("broken.wasm"), b"invalid").unwrap();
            fs::write(dir.path().join("ignored.txt"), fixture(META.as_bytes(), "")).unwrap();
            let plugins = scan_plugin_dir(dir.path()).unwrap();
            assert_eq!(plugins.len(), 1);
            assert_eq!(FormatPlugin::id(&plugins[0]), "fixture");
        }

        #[test]
        fn public_empty_extract_and_legacy_report_defaults() {
            let dir = tempdir();
            let report = br#"{"files_modified":0,"strings_written":0,"strings_skipped":2,"warnings":["legacy"]}"#;
            let operations = format!(
                r#"{} {}
                    (func (export "locust_extract") (param i32 i32) (result i32) i32.const 1024)
                    (func (export "locust_inject") (param i32 i32 i32 i32) (result i32) i32.const 8192)"#,
                data(1024, b"[]"),
                data(8192, report)
            );
            let wasm = load(&dir, "legacy.wasm", &fixture(META.as_bytes(), &operations));
            let plugin: &dyn FormatPlugin = &wasm;
            let path = std::path::Path::new("empty.txt");
            let extracted = plugin.extract_with_diagnostics(path).unwrap();
            assert!(extracted.complete);
            assert!(extracted.entries.is_empty());
            assert!(extracted.warnings.is_empty());
            let report = plugin.inject(path, &[]).unwrap();
            assert_eq!(report.files_modified, 0);
            assert_eq!(report.strings_written, 0);
            assert_eq!(report.strings_skipped, 2);
            assert_eq!(report.warnings, ["legacy"]);
            assert!(report.files_written.is_empty());
            assert!(report.skip_reasons.is_empty());
        }

        #[test]
        fn loader_rejects_missing_exports_imports_traps_and_malformed_metadata() {
            let dir = tempdir();
            let valid = fixture(META.as_bytes(), "");
            let cases = [
                (
                    "missing-metadata",
                    "(module (memory (export \"memory\") 1))".to_string(),
                ),
                (
                    "missing-memory",
                    "(module (func (export \"locust_plugin_metadata\") (result i32) i32.const 0))"
                        .to_string(),
                ),
                (
                    "unknown-import",
                    valid.replacen("\"locust_log\"", "\"not_available\"", 1),
                ),
                (
                    "metadata-trap",
                    valid.replace("(result i32) i32.const 0)", "(result i32) unreachable)"),
                ),
                (
                    "pointer-beyond-memory",
                    valid.replace("(result i32) i32.const 0)", "(result i32) i32.const 65536)"),
                ),
                (
                    "length-beyond-memory",
                    fixture(b"", r#"(data (i32.const 0) "\00\00\01\00")"#),
                ),
                ("invalid-utf8", fixture(&[0xff], "")),
                ("malformed-json", fixture(b"{", "")),
                (
                    "missing-id",
                    fixture(br#"{"name":"missing","extensions":[]}"#, ""),
                ),
            ];
            for (name, wat) in cases {
                let path = dir.path().join(format!("{name}.wasm"));
                fs::write(&path, wat).unwrap();
                assert!(load_wasm_plugin(&path).is_err(), "{name}");
            }
        }

        #[test]
        fn public_operations_reject_missing_exports_traps_and_malformed_results() {
            let dir = tempdir();
            let path = std::path::Path::new("input.txt");
            let results = format!("{} {}", data(1024, b"not json"), data(8192, &[0xff]));
            let bad = format!(
                r#"{results}
                    (func (export "locust_extract") (param i32 i32) (result i32) i32.const 1024)
                    (func (export "locust_inject") (param i32 i32 i32 i32) (result i32) i32.const 8192)"#
            );
            let bad_shape = format!(
                r#"{} {}
                    (func (export "locust_extract") (param i32 i32) (result i32) i32.const 1024)
                    (func (export "locust_inject") (param i32 i32 i32 i32) (result i32) i32.const 8192)"#,
                data(1024, b"{}"),
                data(8192, b"{}")
            );
            let bad_pointer = r#"
                (func (export "locust_extract") (param i32 i32) (result i32) i32.const 65536)
                (func (export "locust_inject") (param i32 i32 i32 i32) (result i32) i32.const 65536)"#;
            let trapped = r#"
                (func (export "locust_extract") (param i32 i32) (result i32) unreachable)
                (func (export "locust_inject") (param i32 i32 i32 i32) (result i32) unreachable)"#;
            let wrong_type = r#"
                (func (export "locust_extract") (result i32) i32.const 0)
                (func (export "locust_inject") (result i32) i32.const 0)"#;
            for (name, wat) in [
                ("missing-operations", fixture(META.as_bytes(), "")),
                (
                    "missing-allocator",
                    fixture(META.as_bytes(), &bad).replace("\"locust_alloc\"", "\"unused_alloc\""),
                ),
                ("wrong-signatures", fixture(META.as_bytes(), wrong_type)),
                ("traps", fixture(META.as_bytes(), trapped)),
                ("malformed-results", fixture(META.as_bytes(), &bad)),
                ("wrong-json-shape", fixture(META.as_bytes(), &bad_shape)),
                (
                    "result-pointer-beyond-memory",
                    fixture(META.as_bytes(), bad_pointer),
                ),
            ] {
                let wasm = load(&dir, &format!("{name}.wasm"), &wat);
                let plugin: &dyn FormatPlugin = &wasm;
                assert!(plugin.extract(path).is_err(), "{name}: extract");
                assert!(plugin.inject(path, &[]).is_err(), "{name}: inject");
            }
        }
    }

    #[cfg(feature = "wasm-plugins")]
    #[test]
    #[ignore = "requires wasm32-wasi target: rustup target add wasm32-wasi"]
    fn test_load_example_wasm_plugin() {
        // This test requires building the example plugin first
        let plugin_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("plugin-example")
            .join("target")
            .join("wasm32-wasi")
            .join("release")
            .join("locust_example_plugin.wasm");

        if !plugin_path.exists() {
            panic!(
                "Example plugin not built. Run: cd docs/plugin-example && cargo build --target wasm32-wasi --release"
            );
        }

        let plugin = load_wasm_plugin(&plugin_path).unwrap();
        assert_eq!(plugin.metadata.id, "txt-lines");
        assert!(!plugin.metadata.name.is_empty());
    }
}

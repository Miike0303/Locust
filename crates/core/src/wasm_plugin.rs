//! WASM plugin support for external format plugins.
//!
//! Requires the `wasm-plugins` feature flag.

#[cfg(feature = "wasm-plugins")]
mod inner {
    use std::path::{Path, PathBuf};

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
                .func_wrap("env", "locust_log", |mut caller: Caller<'_, ()>, ptr: i32, len: i32| {
                    if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                        let data = memory.data(&caller);
                        let start = ptr as usize;
                        let end = start + len as usize;
                        if end <= data.len() {
                            if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                                tracing::info!("[wasm-plugin] {}", msg);
                            }
                        }
                    }
                })
                .map_err(|e| LocustError::Other(e.into()))?;

            let instance = linker
                .instantiate(&mut store, &self.module)
                .map_err(|e| LocustError::Other(e.into()))?;

            Ok((store, instance))
        }

        fn read_wasm_string(store: &Store<()>, memory: &Memory, ptr: i32) -> Result<String> {
            let data = memory.data(store);
            let start = ptr as usize;

            // Read length prefix (first 4 bytes, little-endian)
            if start + 4 > data.len() {
                return Err(LocustError::Other(anyhow::anyhow!("invalid WASM string pointer")));
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
                return Err(LocustError::Other(anyhow::anyhow!("WASM string out of bounds")));
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
                .map_err(|e| LocustError::Other(e.into()))?;

            let bytes = s.as_bytes();
            let total_len = 4 + bytes.len();
            let ptr = alloc
                .call(&mut *store, total_len as i32)
                .map_err(|e| LocustError::Other(e.into()))?;

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
                .map_err(|e| LocustError::Other(e.into()))?;

            let result_ptr = extract_fn
                .call(&mut store, (path_ptr, path_len))
                .map_err(|e| LocustError::Other(e.into()))?;

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
                .map_err(|e| LocustError::Other(e.into()))?;

            let result_ptr = inject_fn
                .call(&mut store, (path_ptr, path_len, entries_ptr, entries_len))
                .map_err(|e| LocustError::Other(e.into()))?;

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
        let module = Module::from_file(&engine, path)
            .map_err(|e| LocustError::Other(anyhow::anyhow!("failed to load WASM module: {}", e)))?;

        // Create a temporary instance to read metadata
        let mut store = Store::new(&engine, ());
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap("env", "locust_log", |_: Caller<'_, ()>, _: i32, _: i32| {})
            .map_err(|e| LocustError::Other(e.into()))?;

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| LocustError::Other(anyhow::anyhow!("failed to instantiate WASM: {}", e)))?;

        let metadata_fn = instance
            .get_typed_func::<(), i32>(&mut store, "locust_plugin_metadata")
            .map_err(|e| LocustError::Other(anyhow::anyhow!("missing locust_plugin_metadata: {}", e)))?;

        let meta_ptr = metadata_fn
            .call(&mut store, ())
            .map_err(|e| LocustError::Other(e.into()))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| LocustError::Other(anyhow::anyhow!("no memory export")))?;

        let meta_json = WasmPlugin::read_wasm_string(&store, &memory, meta_ptr)?;
        let metadata: WasmPluginMetadata = serde_json::from_str(&meta_json)?;

        tracing::info!("Loaded WASM plugin: {} v{}", metadata.name, metadata.version);

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
            if path.extension().map_or(false, |e| e == "wasm") {
                match load_wasm_plugin(&path) {
                    Ok(plugin) => {
                        tracing::info!("Registered WASM plugin: {}", plugin.metadata.id);
                        plugins.push(plugin);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load WASM plugin {}: {}",
                            path.display(),
                            e
                        );
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
    use std::path::Path;
    use crate::error::Result;

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

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_wasm_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_scan_plugin_dir_finds_wasm() {
        // Without actual WASM files, this just tests the scanning logic
        let dir = tempdir();
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
        let dir = tempdir();
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
        let dir = tempdir();
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

# Locust WASM Plugin Development Guide

## Overview

Locust supports external format plugins via WebAssembly (WASM). This allows you to add support for new game formats without modifying the core codebase.

## Interface Contract

Your WASM module must export the following functions:

### Required Exports

```
locust_plugin_metadata() -> i32
```
Returns a pointer to a length-prefixed JSON string containing plugin metadata.

```
locust_extract(path_ptr: i32, path_len: i32) -> i32
```
Extracts translatable strings from the given path. Returns pointer to JSON `Vec<StringEntry>`.

```
locust_inject(path_ptr: i32, path_len: i32, entries_ptr: i32, entries_len: i32) -> i32
```
Injects translations back. Returns pointer to JSON `InjectionReport`.

```
locust_alloc(size: i32) -> i32
```
Allocates memory in the WASM module. Returns pointer.

```
locust_free(ptr: i32, size: i32)
```
Frees previously allocated memory.

### Host Imports (provided by Locust)

```
env.locust_log(ptr: i32, len: i32)
```
Logs a UTF-8 string message via the host's tracing system.

## String Encoding

All strings passed between host and WASM use a length-prefixed format:
- First 4 bytes: little-endian u32 length of the string data
- Following bytes: UTF-8 encoded string data

## Metadata Format

```json
{
  "id": "my-format",
  "name": "My Game Format",
  "description": "Handles .xyz files",
  "version": "0.1.0",
  "extensions": [".xyz"],
  "author": "Your Name"
}
```

## Building

```bash
rustup target add wasm32-wasi
cargo build --target wasm32-wasi --release
```

Place the resulting `.wasm` file in `~/.config/project-locust/plugins/` (Linux), `~/Library/Application Support/project-locust/plugins/` (macOS), or `%LOCALAPPDATA%/project-locust/plugins/` (Windows).

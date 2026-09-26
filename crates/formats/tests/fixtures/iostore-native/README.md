# Independent native IoStore fixtures

These three tiny containers were generated using unmodified `retoc` 0.1.5 at
commit `885a8dae740cb1ce1e41ff2e74f67f9f0c118237` (MIT). They contain one deliberately
neutral, synthetic LocRes resource, with the text `Independent retoc fixture greeting`.
The source payload is `expected.locres`; no original game content is included.

All use chunk ID `010000000000000000000007` (`ExternalFile`) and the path
`../../../FixtureGame/Content/Localization/Game/en/Game.locres`. The supported TOC
versions are 5 (PerfectHashWithOverflow), 7 (RemovedOnDemandMetaData), and 8
(ReplaceIoChunkHashWithIoHash). The independent writer uses uncompressed blocks.

Reproduction from a Locust checkout, with a separately built reference tool:

```powershell
git clone https://github.com/trumank/retoc.git reference-retoc
git -C reference-retoc checkout 885a8dae740cb1ce1e41ff2e74f67f9f0c118237
cargo build --manifest-path reference-retoc/Cargo.toml -p retoc_cli --target-dir target-reference-retoc
cargo run -p locust-formats --example iostore_reference_fixture -- target-reference-retoc/debug/retoc.exe crates/formats/tests/fixtures/iostore-native
```

The example prepares only the input payload/manifest. The external retoc process
serializes each UTOC/UCAS pair, then its own `verify` and `list --path` commands
validate integrity and directory mapping. Locust's integration test separately
reads those files and checks byte-identical extraction and plugin output. The
reference tool is not a runtime dependency and is not included in the fixtures.

`PROVENANCE.json` records the source revision and fixture SHA-256 hashes. The
handcrafted codec/partition/adversarial fixtures are separate tests and are not
represented as retoc-produced samples.

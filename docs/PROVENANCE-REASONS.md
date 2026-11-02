# Injection provenance producer audit

Audited at `9b25f2f` (W4-10). **No current QA producer loses a reason.** No
repairable `reason: None` path was found, so no variants, producer changes,
duplicate tests, or localization keys are needed.

There are three calls to `invalid_provenance_issue`: binary budget validation
(`crates/core/src/validation.rs:506`), grouped metadata validation
(`crates/core/src/validation.rs:509`), and checked translation controls
(`crates/core/src/validation.rs:583`). Their complete error paths are below.
`P` means `InjectionProvenanceError::new` (`crates/core/src/error.rs:12`);
conversion to `LocustError::InjectionProvenance` preserves its reason
(`crates/core/src/error.rs:35`). Every `P` row already yields `Some(reason)`.

| Producer / condition (`path:line`) | Error constructor | Existing typed reason |
| --- | --- | --- |
| Physical source marker is empty or not a string: `crates/core/src/models.rs:303`. Reached through budget, checked controls, and grouped patch creation (`crates/core/src/textasset_group.rs:518`). | P | `MalformedMetadata { field }` |
| Immutable capacity is not an object; encoding is not a string; bytes are missing, negative, noninteger or cannot fit `usize`: `crates/core/src/models.rs:328`, `crates/core/src/models.rs:339`, `crates/core/src/models.rs:354`. | P | `MalformedMetadata { field }` (whole marker / `.encoding` / `.bytes`) |
| Explicit capacity is zero or encoding is unsupported (also checked for grouped rows): `crates/core/src/validation.rs:131`. | P | `InvalidCapacity { encoding, capacity }` |
| Slot and explicit capacity encodings disagree (also checked for grouped rows): `crates/core/src/validation.rs:146`. | P | `ConflictingEncodings` |
| Explicit nongrouped capacity disagrees with encoded physical source, including an unencodable source: `crates/core/src/validation.rs:159`. | P | `CapacitySourceMismatch` |
| Derived binary source cannot be encoded, or slot encoding is unsupported: `crates/core/src/validation.rs:188`. | P | `UnencodableSource { encoding }` |
| Nongrouped / unsupported extraction method parser guards: `crates/core/src/textasset_group.rs:258`, `crates/core/src/textasset_group.rs:279`. | P | `UnsupportedTextAssetLayout`; unreachable from QA because `require_valid_metadata` and group collection first check `is_grouped_entry` (`crates/core/src/textasset_group.rs:432`, `crates/core/src/textasset_group.rs:731`). |
| Extraction refusal marker, too many members, oversized original, or final combined bounds guard: `crates/core/src/textasset_group.rs:271`, `crates/core/src/textasset_group.rs:365`, `crates/core/src/textasset_group.rs:368`, `crates/core/src/textasset_group.rs:381`. | `too_large_error` → P (`crates/core/src/textasset_group.rs:234`) | `TextAssetGroupTooLarge` |
| Unknown group kind: `crates/core/src/textasset_group.rs:292`. | P | `UnsupportedTextAssetLayout` |
| Missing/nonstring kind; missing/nonstring/empty ID: `crates/core/src/textasset_group.rs:302`, `crates/core/src/textasset_group.rs:325`. | P | `MissingMetadata { field }` |
| Group kind disagrees with extraction method: `crates/core/src/textasset_group.rs:314`. | P | `ConflictingTextAssetMetadata` |
| Missing/invalid group capacity; zero capacity: `crates/core/src/textasset_group.rs:337`, `crates/core/src/textasset_group.rs:346`. | P | `MissingMetadata { field }`; `InvalidCapacity` |
| Missing/invalid/zero member count: `crates/core/src/textasset_group.rs:357`. | P | `MalformedMetadata { field }` |
| Legacy inline original is not a string, is empty, or fails original construction: `crates/core/src/textasset_group.rs:130`, `crates/core/src/textasset_group.rs:134`. | `TextAssetOriginal::new` string error (`crates/core/src/models.rs:390`) wrapped by `malformed` → P (`crates/core/src/textasset_group.rs:120`) | `MalformedMetadata { field }`; oversized originals hit the bounds guard first. |
| Original reference is not a string: `crates/core/src/textasset_group.rs:140`. | `malformed` → P | `MalformedMetadata { field }` |
| Reference has no hydrated/inline original: `crates/core/src/textasset_group.rs:147`. | P | `MissingTextAssetOriginal` |
| Digest or original byte length is wrong/missing/invalid: `crates/core/src/textasset_group.rs:159`. | P | `TextAssetOriginalMismatch` |
| Original length exists without digest reference: `crates/core/src/textasset_group.rs:165`. | P | `MissingMetadata { field }` |
| Inline and shared originals disagree: `crates/core/src/textasset_group.rs:174`. | P | `ConflictingTextAssetMetadata` |
| No original after resolving metadata: `crates/core/src/textasset_group.rs:372`. | P | `MissingTextAssetOriginal` |
| Original exceeds recorded capacity: `crates/core/src/textasset_group.rs:384`. | P | `TextAssetOriginalExceedsCapacity` |
| Missing/invalid overhead; overhead exceeds original: `crates/core/src/textasset_group.rs:397`, `crates/core/src/textasset_group.rs:406`. | P | `MissingMetadata { field }`; `InvalidTextAssetOverhead` |
| Group metadata disagrees across members: `crates/core/src/textasset_group.rs:752`. | Direct `ValidationKind::InvalidInjectionProvenance` | `Some(ConflictingTextAssetMetadata)` |
| Group patch physical-source check fails: `crates/core/src/textasset_group.rs:777`. | Direct kind from checked P error | `Some(error.reason)` (malformed source above) |
| Reconstruction rejects CR/LF (both layouts), CSV comma/quote, missing row/column, header-row edit, or duplicate row: `crates/core/src/textasset_group.rs:563`, `crates/core/src/textasset_group.rs:568`, `crates/core/src/textasset_group.rs:570`, `crates/core/src/textasset_group.rs:571`, `crates/core/src/textasset_group.rs:573`, `crates/core/src/textasset_group.rs:577`. | Internal `Err("error")` token → direct kind (`crates/core/src/textasset_group.rs:793`) | `Some(UnsupportedTextAssetReconstruction { layout })`; fixed token, no English diagnostic parsing. |
| Generic mapper fallback: `crates/core/src/validation.rs:484`. | Any non-`InjectionProvenance` `LocustError` | `None`, currently unreachable: budget errors are all P, metadata wraps only P (`crates/core/src/textasset_group.rs:435`), and checked controls return only P (`crates/core/src/models.rs:248`). Retained as a defensive fallback. |
| Historical unit kind, absent/null/incomplete/unknown reason read from storage: `crates/core/src/models.rs:597`, `crates/core/src/models.rs:606`. | Deserialization, no error producer | `None` intentionally: old/future display data cannot supply a known reason; preserve readability and the raw diagnostic. |

Other string-returning gates do not feed this kind: `require_current_translation`
(`crates/core/src/models.rs:207`) produces `StaleTranslation`
(`crates/core/src/validation.rs:498`); `require_preserved_translation_controls`
(`crates/core/src/models.rs:276`) is an injection/pivot gate, while QA uses the
checked method and emits placeholder kinds. Public string wrappers for source,
capacity, controls, group parsing and shared originals are not the QA error path.
Structural capacity (`crates/core/src/textasset_group.rs:89`) cannot error.
Reconstruction's `target_missing` / `source_changed` tokens emit no provenance
issue (`crates/core/src/textasset_group.rs:789`); reconstructed overflow emits
`ExceedsBinarySlot` (`crates/core/src/textasset_group.rs:819`), not an untyped
provenance issue.

Existing real-entry tests cover these producer families in
`crates/core/tests/provenance_reasons.rs:61`, `:106`, `:153`, `:191`, `:223`,
`:290`, and `:324`; grouped fixtures use extraction's `attach_to_entries`
(`crates/core/tests/provenance_reasons.rs:29`,
`crates/formats/src/unity.rs:1038`, `crates/formats/src/unity.rs:1140`). Report /
stored API coverage starts at `crates/core/tests/provenance_reasons.rs:397`;
all 14 reasons round-trip at `crates/core/tests/provenance_reasons.rs:430`.
Legacy readability and reason-independent ignore identity are already tested at
`crates/core/src/database/validation.rs:202` and
`crates/core/src/database/validation.rs:235`. EN/ES reason rendering and fallback
tests are in `apps/desktop/src/lib/validationLabels.test.ts:181` and `:212`.
No RED→GREEN test is warranted: the audit found no missing typed producer.

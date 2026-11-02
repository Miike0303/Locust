# Deployment specification for 0.3.0

This is a future deployment runbook, not a record of deployed infrastructure. The audit permits static distribution, an updater mirror and patch hosting; domains, VPS/DNS control and credentials remain owner dependencies. Repository evidence is cited as `path:line`; operational steps below are proposed policy. (`odd/tasks/cycle-2026-11-01-release-audit.md:153`, `odd/tasks/cycle-2026-11-01-release-audit.md:178`)

## Public hosting boundary

| May be hosted | What it serves | Application changes |
| --- | --- | --- |
| Static downloads/docs site | Public documentation and links to verified installers/CLI downloads | None for a separate static site; do not deploy the desktop frontend as a translation service. (`odd/tasks/cycle-2026-11-01-release-audit.md:157`) |
| Manifest-only updater mirror | `latest.json`, retaining GitHub artifact URLs | Endpoint configuration and a verified-manifest upload/promotion step. (`apps/desktop/src-tauri/tauri.conf.json:63`, `scripts/generate-updater-manifest.py:88`) |
| Full updater mirror | JSON plus byte-identical signed updater artifacts | Endpoint configuration, separate mirror URL generation/verification and artifact upload/promotion. (`scripts/generate-updater-manifest.py:99`, `scripts/release-gate.py:97`) |
| Patch host | Immutable, versioned patch ZIPs for `locust apply --url` | No client change for compliant public HTTP(S) URLs. (`crates/cli/src/main.rs:218`, `crates/core/src/patch/net_policy.rs:14`) |

**Never expose the Axum server through a public listener, reverse proxy or Cloudflare Tunnel.** Desktop binds `127.0.0.1:0`; normal CLI/server startup binds `127.0.0.1:<port>`. The outer Host/Origin guard rejects foreign values before handlers. Missing headers are allowed, so this is not authentication; the library's explicit-address bind is not a public-service support promise. A proxy rewriting Host to localhost would undermine the boundary. (`apps/desktop/src-tauri/src/main.rs:28`, `crates/server/src/lib.rs:566`, `crates/server/src/lib.rs:654`, `crates/server/src/lib.rs:662`, `crates/server/src/lib.rs:671`, `odd/tasks/cycle-2026-11-01-release-audit.md:167`)

Keep databases, settings/API keys, backups and translation operations off public storage. These are local application data, and patch handlers operate on filesystem paths. (`README.md:348`, `README.md:350`, `README.md:360`, `crates/server/src/lib.rs:663`)

## Common DNS, TLS, cache and credential policy

Apply these steps to each chosen hostname below. All angle-bracket values are placeholders, not assigned infrastructure. **Owner input needed:** choose the domain/zone, hostnames, hosting provider, public origin addresses or provider CNAME target, and proxied versus DNS-only operation. (`odd/tasks/cycle-2026-11-01-release-audit.md:157`, `odd/tasks/cycle-2026-11-01-release-audit.md:158`, `odd/tasks/cycle-2026-11-01-release-audit.md:159`)

1. Create an A record to `<public-vps-ipv4>` and, only if IPv6 works, an AAAA record to `<public-vps-ipv6>`; alternatively use the static/object host's supplied CNAME target. Validate public resolution before promotion. Cloudflare supports proxying A/AAAA/CNAME records; DNS-only sends clients directly to the origin. ([Cloudflare proxy status](https://developers.cloudflare.com/dns/proxy-status/); deployment basis: `odd/tasks/cycle-2026-11-01-release-audit.md:157`.)
2. For proxied hosting, provision the edge certificate and an origin certificate matching the hostname, then use **Full (strict)**. A public CA certificate or Cloudflare Origin CA certificate can secure that origin hop. For DNS-only hosting, use a publicly trusted origin certificate; an Origin CA certificate alone is unsuitable for direct clients. Plan renewal and test HTTPS before publishing URLs. ([Full (strict)](https://developers.cloudflare.com/ssl/origin-configuration/ssl-modes/full-strict/), [Origin CA](https://developers.cloudflare.com/ssl/origin-configuration/origin-ca/); TLS prerequisite: `odd/tasks/cycle-2026-11-01-release-audit.md:158`.)
3. Proposed cache policy: mutable `latest.json` gets `Cache-Control: public, max-age=60, no-transform` with an edge TTL no longer than 60 seconds, or bypass caching and purge on promotion. Versioned assets get `Cache-Control: public, max-age=31536000, immutable, no-transform`; never overwrite a versioned URL. Make Cloudflare rules respect these lifetimes; `immutable` alone does not set its edge TTL. ([Cache control](https://developers.cloudflare.com/cache/concepts/cache-control/); promotion basis: `odd/tasks/cycle-2026-11-01-release-audit.md:161`.)
4. On machine download paths, exclude interactive/JavaScript challenges, Access login pages and other HTML interstitials. Ensure Rocket Loader, any minification, Workers/body transforms and other content rewrites do not alter ZIP, JSON or signature responses. Do not assume `no-transform` exempts a URL from all security rules. Compare downloaded bytes/hashes with the approved originals through the public hostname. Challenges return HTML and interrupt non-HTML clients. ([Challenge limitations](https://developers.cloudflare.com/cloudflare-challenges/challenge-types/challenge-pages/#compatibility-limitations); byte-preservation requirement: `odd/tasks/cycle-2026-11-01-release-audit.md:159`, exact manifest comparison: `scripts/release-gate.py:90`.)
5. **Owner input needed:** supply CI upload credentials scoped to the selected bucket/prefix or a restricted VPS deployment directory, and a separate zone-scoped cache-purge token if needed. Store their values as GitHub Actions secrets; agree their names only when implementing the integration. Expose them only to the upload/promotion step, never public files or logs. Keep updater signing secrets separate and retain the configured public key. Existing signing secret wiring is at `.github/workflows/release.yml:120`; scoped deployment credentials are required by `odd/tasks/cycle-2026-11-01-release-audit.md:158`.

## Option 1: static download/docs site

1. **Owner input needed:** select `<www.example.com>`, static host/VPS directory and the public documents to publish. Create its A/AAAA or provider CNAME and configure TLS using the common policy. This is separate public content, not the bundled desktop UI. (`odd/tasks/cycle-2026-11-01-release-audit.md:157`)
2. Prepare a versioned site snapshot linking only accepted GitHub release assets and published documentation. Add a scoped upload step or separate deployment workflow; no change to updater endpoints or manifest generation is needed. Release asset acceptance remains the existing finalizer's responsibility. (`.github/workflows/release.yml:229`, `.github/workflows/release.yml:237`)
3. Publish the snapshot atomically; use short caching/purge for its mutable download index and immutable caching only for versioned files. Apply the common byte/challenge policy to any download paths. This implements the audit's verified-assets-first promotion policy. (`odd/tasks/cycle-2026-11-01-release-audit.md:161`)
4. Roll back by restoring the previous site snapshot and purging mutable pages. Keep links to a known-good published release; removing a bad release from listings does not downgrade installed apps. (`RELEASE.md:117`)

## Option 2: updater mirror

The current endpoint is `https://github.com/Miike0303/Locust/releases/latest/download/latest.json`. The generator writes GitHub artifact URLs; no custom mirror switch exists. Strict generation requires all five desktop artifact/signature pairs; the workflow also gates all four CLI binaries before publishing. (`apps/desktop/src-tauri/tauri.conf.json:64`, `scripts/generate-updater-manifest.py:77`, `scripts/generate-updater-manifest.py:88`, `.github/workflows/release.yml:200`)

### Manifest-only mirror

1. **Owner input needed:** select `<updates.example.com>` and storage. Configure its DNS/TLS and machine-download rules above. Stage the verified JSON under a versioned release path; serve the promoted document at `https://<updates.example.com>/latest.json`. (`odd/tasks/cycle-2026-11-01-release-audit.md:158`)
2. In a future migration release, set `apps/desktop/src-tauri/tauri.conf.json` → `plugins.updater.endpoints` to this overlap list, leaving `plugins.updater.pubkey` unchanged:

   ```json
   [
     "https://<updates.example.com>/latest.json",
     "https://github.com/Miike0303/Locust/releases/latest/download/latest.json"
   ]
   ```

   This is the audit's proposed overlap configuration, not a deployed change. Old binaries retain their embedded GitHub URL. (`apps/desktop/src-tauri/tauri.conf.json:62`, `odd/tasks/cycle-2026-11-01-release-audit.md:161`)
3. Leave `scripts/generate-updater-manifest.py` unchanged: copy the exact verified GitHub `latest.json`, including its GitHub artifact URLs. Extend `.github/workflows/release.yml` after the uploaded-inventory verification at line 229 to stage/download/compare the mirror copy. After the GitHub publish step at line 237 succeeds, atomically promote that copy to the mirror's `latest.json` and purge its cache. A mirror failure must leave its previous manifest intact and be reported for recovery; do not bypass the GitHub gate. (`scripts/generate-updater-manifest.py:88`, `scripts/release-gate.py:90`, `.github/workflows/release.yml:224`)
4. Verify every mirror manifest URL, signature and platform entry, then perform older-installation download/install/relaunch acceptance. JSON shape checks alone do not establish key correspondence or native updating. (`RELEASE.md:81`, `RELEASE.md:88`, `RELEASE.md:117`)

### Full artifact mirror

1. Use the same DNS/TLS, endpoint overlap and cache policy. **Owner input needed:** choose artifact storage, capacity/bandwidth and retention. Use `https://<updates.example.com>/releases/<tag>/<exact-asset-name>` for immutable copies, preserving filenames and `.sig` companions. (`docs/update-server.md:53`, `odd/tasks/cycle-2026-11-01-release-audit.md:158`)
2. Extend `scripts/generate-updater-manifest.py` with an explicit, validated HTTPS artifact-base input for a **separate mirror manifest**. Keep GitHub metadata/signature retrieval at lines 84 and 91 unchanged; separate the signature-download base from the output artifact base used at line 100. Preserve strict five-platform pairing, version/notes/signatures and atomic output replacement. Generate the normal GitHub manifest first and save it separately before generating mirror JSON; do not replace the GitHub manifest with mirror URLs. (`scripts/generate-updater-manifest.py:83`, `scripts/generate-updater-manifest.py:99`, `scripts/generate-updater-manifest.py:119`)
3. Extend `.github/workflows/release.yml` within the finalizer, after its existing verification, to copy the exact verified artifacts and signatures to versioned storage and compare downloaded hashes/bytes before promoting mirror JSON. Preserve the GitHub `latest.json` verification and publish sequence. Add separate mirror verification of every expected HTTPS URL and signature. **Implementation prerequisite:** `scripts/release-gate.py:97` currently insists on GitHub URLs, so it needs an explicit mirror-verification mode or a separate verifier; changing only the generator would not suffice. (`.github/workflows/release.yml:229`, `scripts/release-gate.py:90`, `scripts/release-gate.py:97`)
4. Publish GitHub successfully before atomically promoting mirror `latest.json`; purge or honor its short TTL. Validate native updating through the mirror for all declared platform families. Mirror-host migration must preserve signing identity. (`odd/tasks/cycle-2026-11-01-release-audit.md:161`, `RELEASE.md:117`)

### Overlap and rollback for either mirror

**Owner input needed:** define the overlap/support period and who monitors both endpoints. Keep GitHub publication, its `latest.json` and referenced assets working throughout that period and for every older installation still supported. A newly shipped endpoint list cannot redirect old binaries; there is no justified fixed retirement date in this specification. (`apps/desktop/src-tauri/tauri.conf.json:64`, `odd/tasks/cycle-2026-11-01-release-audit.md:161`)

Retain the previous accepted manifest and assets. To roll back promotion, restore that manifest on the mirror, purge mutable caches and restore GitHub's known-good latest designation as appropriate; never replace signed versioned bytes or move a tag. Mirror JSON can also be restored to the accepted manifest containing GitHub artifact URLs. This stops offering a bad update; users already on a higher version may need manual installation, and database downgrade is unsupported (see [compatibility](COMPATIBILITY.md)). (`RELEASE.md:64`, `RELEASE.md:121`, `crates/core/src/database/schema.rs:8`)

## Option 3: patch hosting

1. **Owner input needed:** select `<patches.example.com>`, storage/bandwidth/retention, scoped upload credentials and permission to distribute each patch. Set up A/AAAA or provider CNAME, TLS and the common Cloudflare rules. Publish only distribution ZIPs, not project databases or backups. (`odd/tasks/cycle-2026-11-01-release-audit.md:159`, `README.md:348`, `README.md:360`)
2. Upload to an immutable path such as `https://<patches.example.com>/<patch-id>/<version>/patch.zip`; compare its public download with the approved original before sharing:

   ```text
   locust apply "<game-copy>" --url "https://<patches.example.com>/<patch-id>/<version>/patch.zip"
   ```

   This uses the existing CLI interface; no updater configuration, release workflow or updater-generator change is needed unless the owner separately chooses CI patch uploads. (`crates/cli/src/main.rs:218`)
3. Keep the URL and all redirect destinations public. The client permits HTTP or HTTPS initially, but rejects HTTPS-to-HTTP redirects, non-HTTP(S) schemes, more than five redirects, empty DNS answers and any answer rejected by its private-address policy (including loopback, private/link-local and mapped private IPv4). Each hop is re-resolved and pinned to the checked addresses; ambient proxies are disabled. This runbook requires HTTPS from the initial URL. (`crates/core/src/patch/net_policy.rs:11`, `crates/core/src/patch/net_policy.rs:14`, `crates/core/src/patch/net_policy.rs:25`, `crates/core/src/patch/net_policy.rs:54`, `crates/cli/src/commands/patch.rs:284`)
4. Serve ZIP bytes directly, without login/challenge HTML. Plan for the client's declared-length and streamed-byte checks: the default download ceiling is 32 GiB, configurable with a positive `LOCUST_PATCH_MAX_DOWNLOAD`; empty downloads fail. Check actual host/proxy file-size limits before choosing proxied delivery. (`crates/core/src/patch/zipsec.rs:235`, `crates/cli/src/commands/patch.rs:315`, `crates/cli/src/commands/patch.rs:343`)
5. Roll back distribution by removing the bad version from the mutable listing and directing users to a verified version; do not overwrite an immutable ZIP. An already applied patch is a local operation: preview `locust patch-rollback "<game-copy>" --dry-run`, then roll back using its recorded backup. User-edited added files cause refusal unless explicit force authorizes deletion. (`crates/cli/src/main.rs:238`, `crates/cli/src/commands/patch.rs:351`)

## Owner prerequisite checklist

- [ ] **Owner input needed:** chosen hosting options; domain/zone access; hostnames; VPS public IPv4/working IPv6 or provider CNAME; origin/storage paths; proxy mode and certificate/renewal responsibility.
- [ ] **Owner input needed:** storage, bandwidth, provider size limits, asset/site-snapshot retention and patch redistribution rights.
- [ ] **Owner input needed:** scoped upload and optional cache-purge credentials, agreed GitHub secret names, deployment/promotion operator and rollback access.
- [ ] **Owner input needed:** mirror overlap/support period, endpoint monitoring and accepted short-TTL/purge policy.
- [ ] **Owner input needed:** validate existing updater private-key/password secrets against the configured public key and supply supported older installations/platform access for native acceptance. Do not read/print secrets or disable signing. (`RELEASE.md:11`, `RELEASE.md:117`)

The first four groups implement the owner prerequisites in `odd/tasks/cycle-2026-11-01-release-audit.md:157` through line 161. No infrastructure or repository configuration is changed by this document.

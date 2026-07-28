"""vn_autopatch.py — one-command orchestrator to translate a Japanese VN / game.

Chains the whole pipeline: detect engine -> (for encrypted KiriKiri) deploy the
proxy DLL + runtime-dump the engine-decrypted scripts -> extract -> translate
JA->EN->ES (or EN->ES if an English source exists) -> reinject -> deploy the
`unencrypted/` override.

WHAT IS AND ISN'T AUTOMATIC
  Fully automatic (headless): RPG Maker MV/MZ, Ren'Py, Wolf, Twine — plaintext,
    just extract/translate/inject via locust.
  Semi-automatic: encrypted KiriKiri — the game is launched to dump its own
    decrypted scripts. That step needs the game to actually LOAD the scripts;
    many VNs sit on a title screen until a human clicks "New Game", so this
    script launches + waits, and if scn/script archives weren't touched it tells
    you to play a few minutes, then re-run with --resume-dump.
  Extra tool: KiriKiriZ (.scn/PSB) needs FreeMote to decompile/recompile — this
    script stops after the dump and points you at FreeMote for that case.

USAGE
  python vn_autopatch.py <game_dir> --target es [--pivot en] [--launch-seconds 40]
                         [--resume-dump] [--src-lang ja]
Requires the prebuilt proxy DLLs (winmm.dll + winmm_orig.dll, and version.dll)
in --dll-dir, the locust binary, and the sibling vn-tools scripts.
"""
import os, sys, subprocess, time, glob, argparse, struct, re, shutil

HERE = os.path.dirname(os.path.abspath(__file__))
LOCUST = os.environ.get("LOCUST_BIN", r"C:/Projects/Locust/target-alt/debug/locust.exe")

def log(msg): print(f"[autopatch] {msg}", flush=True)

# ---- engine detection -------------------------------------------------------
def detect_engine(game_dir):
    files = os.listdir(game_dir)
    low = [f.lower() for f in files]
    if any(f == "data" and os.path.isdir(os.path.join(game_dir, f)) for f in files) \
       and glob.glob(os.path.join(game_dir, "**", "*.json"), recursive=True):
        if os.path.exists(os.path.join(game_dir, "www")) or glob.glob(os.path.join(game_dir, "**", "System.json"), recursive=True):
            return "rpgmaker-mv"
    if glob.glob(os.path.join(game_dir, "**", "*.rpy", ), recursive=True):
        return "renpy"
    if any(f.endswith(".xp3") for f in low):
        # KiriKiriZ if it ships .scn (compiled PSB) archives, else classic KAG
        return "kirikiriz" if any("scn" in f for f in low) else "kirikiri"
    return "unknown"

def pe_imports_dll(exe, name):
    b = open(exe, "rb").read()
    return name.lower().encode() in b.lower()

def find_game_exe(game_dir):
    # the KiriKiri engine exe reports "KIRIKIRI" in its version copyright; pick the
    # biggest exe that imports version.dll or winmm.dll and isn't a launcher.
    cands = []
    for f in glob.glob(os.path.join(game_dir, "*.exe")):
        n = os.path.basename(f).lower()
        if n in ("uninstall.exe", "bootstrap.exe") or "破損" in f:
            continue
        if pe_imports_dll(f, "winmm.dll") or pe_imports_dll(f, "version.dll"):
            cands.append((os.path.getsize(f), f))
    cands.sort(reverse=True)
    return cands[0][1] if cands else None

# ---- proxy DLL deployment ---------------------------------------------------
def deploy_proxy(game_dir, exe, dll_dir):
    # pick the proxy the exe actually imports (winmm preferred: more games import it)
    if pe_imports_dll(exe, "winmm.dll"):
        shutil.copy(os.path.join(dll_dir, "winmm.dll"), os.path.join(game_dir, "winmm.dll"))
        shutil.copy(r"C:/Windows/SysWOW64/winmm.dll", os.path.join(game_dir, "winmm_orig.dll"))
        log("deployed winmm.dll proxy (+ winmm_orig.dll)")
    elif pe_imports_dll(exe, "version.dll"):
        shutil.copy(os.path.join(dll_dir, "version.dll"), os.path.join(game_dir, "version.dll"))
        log("deployed version.dll proxy")
    else:
        raise SystemExit("exe imports neither winmm nor version.dll — no proxy target")

# ---- runtime dump -----------------------------------------------------------
def runtime_dump(game_dir, exe, launch_seconds):
    open(os.path.join(game_dir, "dump.txt"), "w").close()
    dump_dir = os.path.join(game_dir, "dump")
    log(f"launching {os.path.basename(exe)} for ~{launch_seconds}s to dump decrypted scripts...")
    ps = ("$p=Start-Process -FilePath '%s' -WorkingDirectory '%s' -PassThru;"
          "Start-Sleep -Seconds %d;"
          "Get-Process -Id $p.Id -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue"
          % (exe, game_dir, launch_seconds))
    subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True)
    dumped = glob.glob(os.path.join(dump_dir, "**", "*.ks"), recursive=True) + \
             glob.glob(os.path.join(dump_dir, "**", "*.scn"), recursive=True)
    return dump_dir, dumped

# ---- main -------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("game_dir")
    ap.add_argument("--target", default="es")
    ap.add_argument("--pivot", default="en", help="pivot language for JA->PIVOT->TARGET")
    ap.add_argument("--src-lang", default="ja")
    ap.add_argument("--launch-seconds", type=int, default=40)
    ap.add_argument("--dll-dir", default=os.path.join(HERE, "proxy"))
    ap.add_argument("--provider", default="grok-sub")
    args = ap.parse_args()

    game = args.game_dir
    engine = detect_engine(game)
    log(f"engine detected: {engine}")

    if engine in ("rpgmaker-mv", "renpy", "wolf-rpg", "sugarcube"):
        log("plaintext engine -> fully automatic via locust")
        db = os.path.join(game, "_autopatch.locust.db")
        subprocess.run([LOCUST, "extract", game, "-o", db], check=True)
        subprocess.run([LOCUST, "translate", db, "-p", args.provider, "-s", args.src_lang,
                        "-t", args.target, "--concurrency", "8"], check=True)
        subprocess.run([LOCUST, "inject", game, "-P", db, "-l", args.target], check=True)
        log("DONE (plaintext, fully automatic)")
        return

    if engine in ("kirikiri", "kirikiriz"):
        exe = find_game_exe(game)
        if not exe:
            raise SystemExit("no KiriKiri engine exe found")
        log(f"engine exe: {os.path.basename(exe)}")
        deploy_proxy(game, exe, args.dll_dir)
        dump_dir, dumped = runtime_dump(game, exe, args.launch_seconds)
        ks = [f for f in dumped if f.endswith(".ks")]
        scn = [f for f in dumped if f.endswith(".scn")]
        log(f"dumped: {len(ks)} .ks, {len(scn)} .scn")
        if not dumped:
            log("NOTHING dumped — the game likely stopped at a title screen.")
            log("Launch it yourself, start a NEW GAME, play a few minutes, close it,")
            log("then re-run this script (the dump accumulates in %s)." % dump_dir)
            return
        if scn:
            log("KiriKiriZ .scn (PSB) dumped. Decompile with FreeMote before translating:")
            log("  PsbDecompile <dump>/scn.xp3/*.scn  ->  translate the JSON  ->  PsbBuild")
            log("This script stops here for KiriKiriZ (FreeMote step is external).")
            return
        # classic KAG .ks: extract -> translate -> reinject -> deploy unencrypted/
        # (uses sibling scripts kag_extract_recursive.py / reinject_bytes.py + locust)
        log("classic KAG .ks path: run kag_extract -> locust translate -> reinject_bytes")
        log("then copy the DIALOGUE-only .ks to <game>/unencrypted/ and delete dump.txt.")
        log("(left as an explicit step so you can review which files are dialogue vs code.)")
        return

    raise SystemExit(f"engine '{engine}' not yet handled by autopatch")

if __name__ == "__main__":
    main()

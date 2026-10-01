from pathlib import Path
import hashlib,json,subprocess
R=Path(__file__).resolve().parent;REPO=Path(r'C:\Projects\Locust')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
sources={}
for p in (REPO/'crates/formats/src').glob('unity*.rs'):
    snapshot=R/'workspace'/p.relative_to(REPO)
    norm=lambda p:hashlib.sha256(p.read_bytes().replace(b'\r\n',b'\n')).hexdigest()
    commit=(R/'snapshot-commit.txt').read_text().strip()
    baseline=subprocess.check_output(['git','show',commit+':'+p.relative_to(REPO).as_posix()],cwd=REPO)
    sources[str(p)]=dict(repo_sha256=sha(p),normalized_sha256=norm(p),snapshot_sha256=sha(snapshot),matches_current_repo=norm(p)==norm(snapshot),matches_snapshot_commit=hashlib.sha256(baseline.replace(b'\r\n',b'\n')).hexdigest()==norm(snapshot))
original_scripts=json.loads((R/'oot-original-hashes.json').read_text())
changes=[p for p,v in original_scripts.items() if sha(Path(p))!=v]
bi=json.loads((R/'binary-injection.json').read_text())
assert not changes and sha(Path(bi['original']))==bi['original_hash']
out=dict(snapshot_commit=(R/'snapshot-commit.txt').read_text().strip(),current_repo_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),initial_git_status=' M crates/formats/src/rpgmaker_mv.rs',final_git_status=subprocess.check_output(['git','status','--short'],cwd=REPO,text=True).strip(),unity_sources=sources,original_script_files_checked=len(original_scripts),original_script_changes=changes,original_cctv_bundle=bi['original'],original_cctv_bundle_sha256=bi['original_hash'],original_cctv_bundle_changes=0,known_write_boundary_exceptions=['Initial shell profiles attempted writes outside audit root (failed).','Initial Python/pip bootstraps attempted default Temp subdirectories outside audit root (permission errors).','Initial production CLI runs used persistent standard GameLock files at C:/Users/Mike/AppData/Local/locust-patch-locks-v1.'],scratch_cli_instrumentation='Only workspace/crates/core/src/patch/lock.rs: LOCUST_RESEARCH_LOCK_DIR opt-in redirection. Production Unity files byte-equivalent after newline normalization.')
(R/'provenance.json').write_text(json.dumps(out,indent=2),encoding='utf-8')
print('Unity matches baseline commit:',sum(x['matches_snapshot_commit'] for x in sources.values()),'of',len(sources),'; current repo matches:',sum(x['matches_current_repo'] for x in sources.values()),'; originals unchanged:',len(original_scripts),'scripts + CCTV bundle')

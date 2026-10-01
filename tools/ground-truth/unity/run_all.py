from pathlib import Path
import subprocess,os,sys,shutil,json,hashlib
ROOT=Path(__file__).resolve().parent
REPO=Path(r'C:\Projects\Locust')
PY=ROOT/'venv/Scripts/python.exe'
def run(args,log,env=None,allow_fail=False):
    print('RUN',log,flush=True)
    with (ROOT/log).open('wb') as f:p=subprocess.run(list(map(str,args)),cwd=ROOT,stdout=f,stderr=subprocess.STDOUT,env=env)
    if p.returncode and not allow_fail:raise RuntimeError(f'{log}: exit {p.returncode}')
    return p.returncode
if not PY.exists():
    run(['py','-3.13','-m','venv','--without-pip',ROOT/'venv'],'venv-setup.log')
try:subprocess.run([str(PY),'-c','import UnityPy,TypeTreeGeneratorAPI'],check=True,capture_output=True)
except subprocess.CalledProcessError:run(['py','-3.13',ROOT/'install_parser.py','UnityPy==1.25.2','TypeTreeGeneratorAPI==0.0.10'],'dependencies.log')
if '--no-build' not in sys.argv:
    run(['py','-3.13',ROOT/'bootstrap.py',*(['--live-unity'] if '--live-unity' in sys.argv else [])],'inventory-interesting.txt')
    # Copy immutable third-party build artifacts from the old cycle cache, only
    # where our scratch target lacks them; never write into that cache.
    cache=Path(r'C:\Users\Mike\AppData\Local\Temp\locust-cycle101\target/debug/deps')
    dst=ROOT/'target/debug/deps';dst.mkdir(parents=True,exist_ok=True);copied=0
    if cache.exists():
        for p in cache.iterdir():
            if p.is_file() and p.suffix in ('.rlib','.rmeta','.lib','.d') and not p.name.startswith(('locust','liblocust')) and not (dst/p.name).exists():shutil.copy2(p,dst/p.name);copied+=1
    (ROOT/'cache-reuse.json').write_text(json.dumps(dict(source=str(cache),copied_files=copied)),encoding='utf-8')
    if '--live-unity' in sys.argv:
        for p in (REPO/'crates/formats/src').glob('unity*.rs'):shutil.copy2(p,ROOT/'workspace'/p.relative_to(REPO))
    run([PY,ROOT/'isolate_cli.py'],'cli-isolation.log')
    env=dict(os.environ,CARGO_TARGET_DIR=str(ROOT/'target'),LOCUST_DATA_DIR=str(ROOT/'locust-data'),LOCUST_RESEARCH_LOCK_DIR=str(ROOT/'locks'))
    run(['cargo','build','-p','locust-cli','--locked','--offline','--manifest-path',ROOT/'workspace/Cargo.toml'],'build.log',env)
env=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'locust-data'),LOCUST_RESEARCH_LOCK_DIR=str(ROOT/'locks'))
games={'cctv':'CCTV_WINDOWS_1_3_FULL','ussr':'CCTV_USSR_WINDOWS_FULL','sunkissed':'Sunkissed_windows_full','oot-alone':'Out of Touch','boxman-alone':'es/BOXMAN_v0.5.02_x64'}
if '--reuse-extractions' not in sys.argv:
    for slug,g in games.items():run([ROOT/'target/debug/locust.exe','extract',Path(r'D:\juegos\unity')/g,'-o',ROOT/(slug+'.db')],slug+'-extract.log',env)
for script in ['probe.py','headers.py','trees.py','oot_audit.py','binary_copy_probe.py','binary_audit.py','make_briefs.py','provenance.py']:
    run([PY,ROOT/script],script+'.log')
run([PY,ROOT/'regressions.py'],'regression.log',allow_fail=True)
run([PY,ROOT/'report.py'],'report-generation.log')
print('DONE',ROOT/'last-report.md',flush=True)

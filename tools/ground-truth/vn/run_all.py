"""One command: py -3.13 run_all.py [--cli PATH] [--build]."""
import argparse,os,subprocess,sys,tarfile,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
def main():
    p=argparse.ArgumentParser();p.add_argument('--cli',type=Path);p.add_argument('--build',action='store_true');args=p.parse_args()
    env=dict(os.environ,LOCUST_DATA_DIR=str(ROOT/'cli-data'),TEMP=str(ROOT),TMP=str(ROOT),PYTHONIOENCODING='utf-8',CARGO_TARGET_DIR=str(ROOT/'target'))
    if args.cli:env['LOCUST_AUDIT_CLI']=str(args.cli.resolve())
    exe=args.cli or ROOT/'target/debug/locust.exe'
    if args.build or not exe.exists():
        snapshot=ROOT/'snapshot'
        if not (snapshot/'Cargo.toml').exists():
            subprocess.run(['git','-C','C:/Projects/Locust','archive','HEAD','--format=tar','--output='+str(ROOT/'head.tar')],env=env,check=True)
            snapshot.mkdir(exist_ok=True)
            with tarfile.open(ROOT/'head.tar') as t:t.extractall(snapshot,filter='data')
        with (ROOT/'build.log').open('w',encoding='utf-8') as log:subprocess.run(['cargo','build','--locked','-p','locust-cli'],cwd=snapshot,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    for step in ['inventory.py','prepare.py','audit.py','ns_audit.py','pairs.py','injection.py','archive_injection.py','regressions.py','inventory.py','report.py']:
        print('RUN',step,flush=True);subprocess.run([sys.executable,str(ROOT/step)],cwd=ROOT,env=env,check=True)
    print('DONE',ROOT/'last-report.md',flush=True)
if __name__=='__main__':main()

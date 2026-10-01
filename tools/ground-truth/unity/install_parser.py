from pathlib import Path
import tempfile,uuid,os,sys
ROOT=Path(__file__).resolve().parent
os.environ['TMP']=os.environ['TEMP']=str(ROOT/'pip-temp')
(ROOT/'pip-temp').mkdir(exist_ok=True)
def mkdtemp(suffix=None,prefix=None,dir=None):
    p=Path(dir or ROOT/'pip-temp')/((prefix or 'tmp')+uuid.uuid4().hex+(suffix or ''))
    p.mkdir(mode=0o777)
    return str(p)
tempfile.mkdtemp=mkdtemp
from pip._internal.cli.main import main
sys.exit(main(['install','--no-cache-dir','--only-binary=:all:','--target',str(ROOT/'venv/Lib/site-packages'),*(sys.argv[1:] or ['UnityPy'])]))

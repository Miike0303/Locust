from inventory import *
from readers import *
import shutil,zipfile,io
def main():
    cases=[];errs=[]
    for game in sorted(Path('D:/juegos/VN').iterdir()):
        loose=list(game.rglob('*.ks'))
        if not loose and game.name!='Ochiru Hitozuma':continue
        slug=game.name.split()[0];dest=ROOT/'corpus'/slug;dest.mkdir(parents=True,exist_ok=True);mapping={}
        for p in loose:
            rel=p.relative_to(game);target=dest/rel;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,target);mapping[str(rel).replace('\\','/')]=str(p)
        if game.name=='Ochiru Hitozuma':
            for archive in sorted(game.glob('*.xp3')):
                if archive.stem not in {'data','patch','patch2'}:continue
                for name,e in xp3_entries(archive):
                    if not name.lower().endswith('.ks'):continue
                    try:
                        b=xp3_read(archive,e);text=decode_ks(b)
                        if not text or sum(ord(c)<32 and c not in '\t\r\n' or c=='\ufffd' for c in text)*20>len(text):raise ValueError('encrypted/unreadable')
                        # Transport normalization only. Preserve a raw snapshot separately.
                        p=dest/archive.name/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b);mapping[str(p.relative_to(dest)).replace('\\','/')]=str(archive)+'/'+name
                    except Exception as ex:errs.append(dict(path=str(archive)+'/'+name,error=str(ex)))
        cases.append(dict(slug=slug,engine='kirikiri',root=str(dest),original=str(game),mapping=mapping))
    game=Path('D:/juegos/VN/Injuu Kangoku RE');dest=ROOT/'corpus/Injuu';dest.mkdir(parents=True,exist_ok=True);mapping={}
    for p in sorted((game/'res').glob('*.ybn')):
        shutil.copy2(p,dest/p.name);mapping[p.name]=str(p)
    cases.append(dict(slug='Injuu',engine='yuris',root=str(dest),original=str(game),mapping=mapping))
    # Official engine author's sample scenarios; separate from installed-game claims.
    dest=ROOT/'corpus/TyranoOfficial/data/scenario';dest.mkdir(parents=True,exist_ok=True);mapping={};srcs=[]
    for name in ['cg.ks','config.ks','first.ks','make.ks','replay.ks','scene1.ks','title.ks','tyrano.ks']:
        url='https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/'+name
        p=dest/name
        if not p.exists():p.write_bytes(urllib.request.urlopen(url,timeout=30).read())
        mapping['data/scenario/'+name]=url;srcs.append(dict(url=url,sha256=digest(p),scratch=str(p)))
    dump('tyrano-public-sources.json',srcs)
    cases.append(dict(slug='TyranoOfficial',engine='tyrano',root=str(dest.parent.parent),original='ShikemokuMK/tyranoscript official example scenarios',mapping=mapping))
    # A third-party maintainer's Japanese compatibility demo, separately scoped.
    jp=ROOT/'corpus/NScripterJP/0.txt';jp.parent.mkdir(parents=True,exist_ok=True)
    url='https://raw.githubusercontent.com/weimingtom/onscripter-libretro_fork/master/test_suite/onscripter_jp_test.zip'
    archive=ROOT/'nscripter-jp-test.zip'
    if not archive.exists():archive.write_bytes(urllib.request.urlopen(url,timeout=60).read())
    if not jp.exists():
        with zipfile.ZipFile(archive) as z:jp.write_bytes(z.read('onscripter_jp_test/0.txt'))
    dump('nscripter-jp-source.json',dict(url=url,member='onscripter_jp_test/0.txt',scratch=str(jp),script_sha256=digest(jp),zip_sha256=digest(archive)))
    dump('cases.json',cases);dump('prepare-errors.json',errs)
    print('CASES',[(x['slug'],len(x['mapping'])) for x in cases],flush=True);print('ERRORS',collections.Counter(x['error'] for x in errs),flush=True)
if __name__=='__main__':main()

from pathlib import Path
ROOT=Path(__file__).resolve().parent
p=ROOT/'workspace/crates/core/src/patch/lock.rs'
s=p.read_text(encoding='utf-8')
old='''let base = dirs::data_local_dir()
            .ok_or_else(|| lock_error("cannot locate the user's local data directory"))?;'''
new='''let base = std::env::var_os("LOCUST_RESEARCH_LOCK_DIR")
            .map(PathBuf::from)
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| lock_error("cannot locate the user's local data directory"))?;'''
assert old in s or new in s
p.write_text(s.replace(old,new),encoding='utf-8')
(ROOT/'locks').mkdir(exist_ok=True)
print('Scratch-only CLI isolation shim: lock namespace redirect; Unity plugin unchanged')

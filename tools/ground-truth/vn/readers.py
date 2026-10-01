"""Independent asset readers. No imports from Locust or its tests.
YSTB layout: specs/YurisNotes.txt and YurisScenarioScript.cs.
KAG tokens: specs/KAGParser.cpp (quoted attributes, [[ escape, iscript blocks).
XP3 framing: specs/ArcXP3.cs:97/:111/:150/:216; container transport only, not the dialogue oracle.
"""
import struct,zlib,re
from pathlib import Path
def decode_ks(b):
    if b[:2]==b'\xfe\xfe':
        mode=b[2];raw=b[5:]
        if mode==2:
            packed,n=struct.unpack_from('<QQ',raw);raw=zlib.decompress(raw[16:16+packed]); assert len(raw)==n
        else:
            units=list(struct.unpack('<'+'H'*(len(raw)//2),raw))
            if mode==1: units=[((u&0xaaaa)>>1)|((u&0x5555)<<1) for u in units]
            elif mode==0:units=[u if u<32 else (((u>>8)^((u&255)&254))<<8)|((u&255)^1) for u in units]
            else:raise ValueError('cipher mode')
            raw=struct.pack('<'+'H'*len(units),*units)
        return raw.decode('utf-16-le')
    if b.startswith(b'\xff\xfe'):return b[2:].decode('utf-16-le')
    try:return b.decode('utf-8-sig')
    except UnicodeDecodeError:return b.decode('cp932')
def chunks(b):
    pos=0
    while pos+12<=len(b):
        tag=b[pos:pos+4];n=struct.unpack_from('<Q',b,pos+4)[0];start=pos+12
        assert start+n<=len(b);yield tag,b[start:start+n];pos=start+n
def xp3_entries(path):
    """Stream index and requested payloads; never load multi-GB media containers."""
    with Path(path).open('rb') as f:
        header=f.read(40); assert header[:11]==b'XP3\r\n \n\x1a\x8b\x67\x01'
        off=struct.unpack_from('<Q',header,11)[0]
        if off==0x17: off=struct.unpack_from('<Q',header,0x20)[0]
        f.seek(off);flag=f.read(1)[0];assert flag in (0,1)
        n=struct.unpack('<Q',f.read(8))[0]
        if flag==1:rawsize=struct.unpack('<Q',f.read(8))[0];idx=zlib.decompress(f.read(n));assert len(idx)==rawsize
        else:idx=f.read(n)
        for tag,body in chunks(idx):
            if tag!=b'File':continue
            parts=dict(chunks(body));info=parts[b'info'];flags,orig,packed,nchars=struct.unpack_from('<IQQH',info)
            name=info[22:22+nchars*2].decode('utf-16-le').replace('\\','/')
            yield name,dict(flags=flags,orig=orig,segments=[struct.unpack_from('<IQQQ',parts[b'segm'],i) for i in range(0,len(parts[b'segm']),28)])
def xp3_read(path,entry):
    raw=b''
    with Path(path).open('rb') as f:
        for flags,off,size,n in entry['segments']:
            f.seek(off);b=f.read(n)
            if flags&1:b=zlib.decompress(b)
            assert len(b)==size;raw+=b
    return raw
def tokens(s):
    """KAG tag scan: quotes shield brackets; doubled [[ renders a literal [."""
    out=[];i=0;start=0
    while i<len(s):
        if s[i:i+2]=='[[':i+=2;continue
        if s[i]!='[':i+=1;continue
        if start<i:out.append(('text',start,i,s[start:i]))
        j=i+1;q=None
        while j<len(s):
            c=s[j]
            if q:
                if c==q:q=None
            elif c in '\"\'':q=c
            elif c==']':break
            j+=1
        if j==len(s):out.append(('text',i,j,s[i:j]));return out
        out.append(('tag',i,j+1,s[i:j+1]));i=j+1;start=i
    if start<len(s):out.append(('text',start,len(s),s[start:]))
    return out
def tagname(t):return t.strip('[]').split(None,1)[0].lower() if t.strip('[]').strip() else ''
def attrs(t):return {m[1].lower():m[2] if m[2] is not None else m[3] for m in re.finditer(r'(\w+)\s*=\s*(?:"([^"]*)"|([^\s\]]+))',t)}
def kag_slots(text,tyrano=False):
    slots=[];code=False;technical=[]
    for n,line in enumerate(text.splitlines(),1):
        t=line.lstrip()
        if code:
            technical.append((n,'script_body',line))
            if t.lower().startswith('@endscript') or '[endscript]' in t.lower():code=False
            continue
        if not t or t[0] in ';*':continue
        ts=[('tag',0,len(t),'['+t[1:]+']')] if t.startswith('@') else tokens(line)
        for kind,start,end,s in ts:
            if kind=='tag':
                name=tagname(s)
                if name=='iscript':code=True
                elif name in {'button','glink','link','name_w','name','chara_ptext'} or tyrano and name in {'ruby','chara_new','ptext'}:
                    a=attrs(s)
                    for key in ({'name_w':'n','name':'text','chara_ptext':'name','button':'text','glink':'text','ruby':'text','chara_new':'jname','ptext':'text'}.get(name),):
                        if key and key in a and a[key].strip() and not a[key].startswith(('%','&')):
                            slots.append(dict(line=n,kind='attribute_'+name+'_'+key,text=a[key],physical=line))
                continue
            value=s.rstrip('\\').strip().replace('[[','[')
            if value:slots.append(dict(line=n,kind='message_text',text=value,physical=line))
    return slots,technical
def ystb(b):
    assert b[:4]==b'YSTB';version,num,isz,dsz,vsz,lsz=struct.unpack_from('<6I',b,4);assert isz==num*4 and dsz%12==0
    offsets=[32,32+isz,32+isz+dsz,32+isz+dsz+vsz]
    key=b[offsets[1]+8:offsets[1]+12] if dsz else bytes(4);data=bytearray(b)
    for off,n in zip(offsets,[isz,dsz,vsz,lsz]):
        for j in range(n):data[off+j]^=key[j%4]
    commands=[];attr_command={};ai=0
    for ci in range(num):
        op,n,other,pad=struct.unpack_from('<4B',data,32+ci*4); commands.append((op,n))
        for j in range(n):attr_command[ai]=(ci,op,j);ai+=1
    attributes=[]
    for i in range(dsz//12):
        ident,typ,n,off=struct.unpack_from('<HhII',data,offsets[1]+12*i);raw=bytes(data[offsets[2]+off:offsets[2]+off+n])
        text=None;expr=False
        if typ==0:text=raw.decode('cp932',errors='replace')
        elif typ==3 and len(raw)>=3 and raw[0]==0x4d and struct.unpack_from('<H',raw,1)[0]+3==len(raw):
            quoted=raw[3:].decode('cp932',errors='replace');text=quoted[1:-1] if len(quoted)>=2 else quoted
            text=re.sub(r'\\([ntr\\])',lambda m:{'n':'\r\n','t':'\t','r':'\r','\\':'\\'}[m[1]],text);expr=True
        cmd=attr_command.get(i,(-1,-1,-1))
        attributes.append(dict(index=i,id=ident,type=typ,size=n,offset=off,raw=raw.hex(),text=text,command=cmd[0],opcode=cmd[1],arg=cmd[2],expr=expr))
    return dict(version=version,key=key.hex(),instructions=bytes(data[offsets[0]:offsets[1]]).hex(),lines=bytes(data[offsets[3]:offsets[3]+lsz]).hex(),tail=bytes(data[offsets[3]+lsz:]).hex(),attributes=attributes)
def yscm(b):
    assert b[:4]==b'YSCM';n=struct.unpack_from('<I',b,8)[0];pos=16;out=[]
    def string():
        nonlocal pos
        end=b.index(0,pos);s=b[pos:end].decode('cp932');pos=end+1;return s
    for i in range(n):
        name=string();argc=b[pos];pos+=1;args=[]
        for j in range(argc):
            args.append(string());pos+=2
        out.append(dict(name=name,args=args))
    return out
def ypf_members(path):
    """Modern standard YPF, GARbro ArcYPF.cs transport rules; sizes checked.
    Shipping archives may use nonstandard name hashes: GARbro derives the
    filename key from the dot before its three-character extension instead.
    """
    b=Path(path).read_bytes();assert b[:4]==b'YPF\0';version,count,dirsize=struct.unpack_from('<III',b,4)
    table=bytes.fromhex('03 48 06 35 0c 10 11 19 1c 1e 09 0b 0d 13 15 1b 20 23 26 29 2c 2f 2e 32')
    if version<0x100:table=table[4:]
    elif 0x12c<=version<0x196:table=table[10:]
    pos=32;out={};key=None
    for i in range(count):
        crc=struct.unpack_from('<I',b,pos)[0];n=b[pos+4]^255
        if n in table:n=table[table.index(n)^1]
        encrypted=b[pos+5:pos+5+n];after=pos+5+n
        if key is None:
            key=encrypted[-4]^ord('.')
        name=bytes(x^key for x in encrypted).decode('cp932').replace('\\','/')
        typ,packed,size,psz,off,adler=struct.unpack_from('<BBIIII',b,after)
        payload=b[off:off+psz];raw=zlib.decompress(payload) if packed else payload;assert len(raw)==size
        if adler:assert adler in {zlib.adler32(payload),zlib.adler32(raw)}
        out[name]=raw;pos=after+18+(4 if version>=0x1d9 else 8 if version==0xde else 0)
    return out

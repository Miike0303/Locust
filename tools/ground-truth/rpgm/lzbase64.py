# Independent LZString base64 decoder; no Locust library calls.
def decode(s):
    alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
    bits=[]
    for ch in s.strip().rstrip('='):
        n=alphabet.index(ch);bits.extend((n>>i)&1 for i in range(5,-1,-1))
    pos=0
    def read(n):
        nonlocal pos
        if pos+n>len(bits):raise ValueError('truncated LZString bitstream')
        value=sum(bits[pos+i]<<i for i in range(n));pos+=n;return value
    n=read(2)
    if n==2:return ''
    c=chr(read(8 if n==0 else 16));dictionary={3:c};w=c;out=[c];size=4;enlarge=4;width=3
    while True:
        n=read(width)
        if n==2:break
        if n in (0,1):dictionary[size]=chr(read(8 if n==0 else 16));n=size;size+=1;enlarge-=1
        if enlarge==0:enlarge=1<<width;width+=1
        if n in dictionary:entry=dictionary[n]
        elif n==size:entry=w+w[0]
        else:raise ValueError('invalid LZString dictionary reference')
        out.append(entry);dictionary[size]=w+entry[0];size+=1;enlarge-=1;w=entry
        if enlarge==0:enlarge=1<<width;width+=1
    units=''.join(out)
    return units.encode('utf-16-le',errors='surrogatepass').decode('utf-16-le')

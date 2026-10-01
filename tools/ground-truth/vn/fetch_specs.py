from inventory import *
SOURCES={
'YurisScenarioScript.cs':'https://raw.githubusercontent.com/arcusmaximus/VNTranslationTools/main/VNTextPatch.Shared/Scripts/Yuris/YurisScenarioScript.cs',
'YurisNotes.txt':'https://raw.githubusercontent.com/arcusmaximus/VNTranslationTools/main/VNTextPatch.Shared/Scripts/Yuris/Notes.txt',
'YurisAttribute.cs':'https://raw.githubusercontent.com/arcusmaximus/VNTranslationTools/main/VNTextPatch.Shared/Scripts/Yuris/YurisAttribute.cs',
'ArcYPF.cs':'https://raw.githubusercontent.com/morkt/GARbro/master/ArcFormats/YuRis/ArcYPF.cs',
'ArcXFL.cs':'https://raw.githubusercontent.com/morkt/GARbro/master/ArcFormats/Liar/ArcXFL.cs',
'ArcDAT.cs':'https://raw.githubusercontent.com/morkt/GARbro/master/ArcFormats/GScripter/ArcDAT.cs',
'kag.parser.js':'https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/tyrano/plugins/kag/kag.parser.js',
'KAGParser.cpp':'https://raw.githubusercontent.com/krkrz/krkr2/master/kirikiri2/src/plugins/win32/krkrkag/KAGParser.cpp',
'ScriptHandler.cpp':'https://raw.githubusercontent.com/ogapee/onscripter/master/ScriptHandler.cpp',
}
def main():
    (ROOT/'specs').mkdir(exist_ok=True); out=[]
    for name,url in SOURCES.items():
        try:
            raw=urllib.request.urlopen(url,timeout=45).read(); p=ROOT/'specs'/name;p.write_bytes(raw)
            out.append(dict(file=str(p),url=url,sha256=digest(p)));print(name,len(raw),flush=True)
        except Exception as e:out.append(dict(file=name,url=url,error=str(e)));print(name,str(e),flush=True)
    dump('spec-sources.json',out)
if __name__=='__main__':main()

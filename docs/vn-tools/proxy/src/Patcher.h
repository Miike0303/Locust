#pragma once

class Patcher
{
    friend CompilerHelper;

public:
    static bool                 PatchSignatureCheck                     (HMODULE hModule);

    static void                 PatchXP3StreamCreation                  ();
    static void                 PatchAutoPathExports                    ();
    static void                 PatchStorageMediaRegistration           ();

private:
    static void __stdcall       CustomTVPAddAutoPath                    (const ttstr& url);
    static void __stdcall       CustomTVPRemoveAutoPath                 (const ttstr& url);

    static void __stdcall       CustomTVPRegisterStorageMedia           (iTVPStorageMedia* pMedia);
    static void __stdcall       CustomTVPUnregisterStorageMedia         (iTVPStorageMedia* pMedia);
    static tTJSBinaryStream*    CustomStorageMediaOpen                  (iTVPStorageMedia* pMedia, const ttstr& name, tjs_uint32 flags);
    static void                 WriteStreamToFile                       (tTJSBinaryStream* pStream, const std::wstring& filePath);

    static bool                 CustomGetSignatureVerificationResult    ();

    template<CompilerType TCompilerType>
    class CustomCreateStreamByIndex
    {
    public:
        static tTJSBinaryStream* Call(tTVPXP3Archive<TCompilerType>* pArchive, tjs_uint idx)
        {
            int itemSize = ((BYTE*)pArchive->ItemVector.end() - (BYTE*)pArchive->ItemVector.begin()) / pArchive->Count;
            auto* pItem = (typename tTVPXP3Archive<TCompilerType>::tArchiveItem*)((BYTE*)pArchive->ItemVector.begin() + idx * itemSize);

            // Override any archive entry with a loose file under <game>/unencrypted/<item>.
            // This routes through the already-installed CreateStreamByIndex hook, so it
            // works even when the storage-media hook never fires (as on this game).
            {
                static std::wstring folderPath = Path::GetModuleFolderPath(nullptr);
                std::wstring loosePath = Path::Combine(Path::Combine(folderPath, L"unencrypted"),
                    StringUtil::Replace<wchar_t>(pItem->Name.c_str(), L'/', L'\\'));
                std::wstring url = Kirikiri::FilePathToUrl(loosePath);
                if (Kirikiri::TVPIsExistentStorageNoSearchNoNormalize(url.c_str()))
                {
                    Debugger::Log(L"Overriding %s from unencrypted/", pItem->Name.c_str());
                    void* pComStream = Kirikiri::TVPCreateIStream(url.c_str(), 0);
                    return Kirikiri::TVPCreateBinaryStreamAdapter(pComStream);
                }
            }

            // Runtime extraction: when <game>/dump.txt exists, on first access to each
            // archive dump ALL its .ks entries (engine-decrypted) to dump/<archive>/<item>.
            // Captures clean plaintext (incl. English patch2) without the CxDec scheme.
            {
                static std::wstring folderPath = Path::GetModuleFolderPath(nullptr);
                static bool dump = GetFileAttributes(Path::Combine(folderPath, L"dump.txt").c_str()) != INVALID_FILE_ATTRIBUTES;
                if (dump)
                {
                    static std::set<void*> dumpedArchives;
                    if (dumpedArchives.find(pArchive) == dumpedArchives.end())
                    {
                        dumpedArchives.insert(pArchive);
                        std::wstring an(pArchive->Name.c_str());
                        size_t sl = an.find_last_of(L'/');
                        std::wstring base = (sl == std::wstring::npos) ? an : an.substr(sl + 1);
                        for (tjs_uint i = 0; i < pArchive->Count; i++)
                        {
                            auto* pIt = (typename tTVPXP3Archive<TCompilerType>::tArchiveItem*)((BYTE*)pArchive->ItemVector.begin() + i * itemSize);
                            std::wstring nm(pIt->Name.c_str());
                            bool isks  = nm.size() >= 3 && nm.compare(nm.size() - 3, 3, L".ks") == 0;
                            bool isscn = nm.size() >= 4 && nm.compare(nm.size() - 4, 4, L".scn") == 0;
                            if (isks || isscn)
                            {
                                auto* s = CompilerHelper::CallInstanceMethod<tTJSBinaryStream*, &OriginalCreateStreamByIndex, tTVPXP3Archive<TCompilerType>*, tjs_uint>(pArchive, i);
                                if (s != nullptr)
                                {
                                    std::wstring dp = Path::Combine(Path::Combine(Path::Combine(folderPath, L"dump"), base),
                                        StringUtil::Replace<wchar_t>(pIt->Name.c_str(), L'/', L'\\'));
                                    Patcher::WriteStreamToFile(s, dp);
                                }
                            }
                        }
                        Debugger::Log(L"Dumped all .ks from %s", base.c_str());
                    }
                }
            }

            if (pItem->FileHash != 0 || !pArchive->Name.StartsWith(L"file://"))
                return CompilerHelper::CallInstanceMethod<tTJSBinaryStream*, &OriginalCreateStreamByIndex, tTVPXP3Archive<TCompilerType>*, tjs_uint>(pArchive, idx);
                
            Debugger::Log(L"Creating unencrypted XP3 stream for %s", pItem->Name.c_str());
            tTVPXP3ArchiveSegment* pSegment = pItem->Segments.begin();
            auto* pStream = new CustomTVPXP3ArchiveStream(pArchive->Name, pSegment->Start, pSegment->OrgSize, pSegment->ArcSize, pSegment->IsCompressed);
            tTJSBinaryStream::ApplyWrappedVTable(pStream);
            return pStream;
        }
    };

    static inline void* OriginalCreateStreamByIndex{};

    static inline void (__stdcall* OriginalTVPAddAutoPath)(const ttstr& path){};
    static inline void (__stdcall* OriginalTVPRemoveAutoPath)(const ttstr& path){};
    static inline void (__stdcall* OriginalTVPRegisterStorageMedia)(iTVPStorageMedia* pMedia){};
    static inline void (__stdcall* OriginalTVPUnregisterStorageMedia)(iTVPStorageMedia* pMedia){};
    static inline std::map<iTVPStorageMedia*, tTJSBinaryStream* (*)(iTVPStorageMedia* pMedia, const ttstr& name, tjs_uint32 flags)> OriginalStorageMediaOpen{};
};

#include "stdafx.h"

BOOL WINAPI DllMain(HINSTANCE hInstance, DWORD reason, LPVOID reserved)
{
    if (reason == DLL_PROCESS_ATTACH)
    {
        Debugger::Log(L"=== DllMain DLL_PROCESS_ATTACH (winmm proxy) ===");
        Proxy::Init();

        Debugger::RegisterDllLoadHandler(
            [](const wchar_t* pwszDllPath, HMODULE hDll)
            {
                if (Debugger::FindExport(hDll, "V2Link") != nullptr)
                    Patcher::PatchSignatureCheck(hDll);
            }
        );

        Kirikiri::Init(
            []
            {
                Debugger::Log(L"=== init callback: installing XP3/patch hooks ===");
                CompilerHelper::Init();
                Patcher::PatchXP3StreamCreation();
                Patcher::PatchAutoPathExports();
                Patcher::PatchStorageMediaRegistration();
                Debugger::Log(L"=== init callback: hooks installed ===");
            }
        );
    }

    return TRUE;
}

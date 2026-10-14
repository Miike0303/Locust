#include "runtime_map.h"
#include <algorithm>
#include <string>

namespace {
constexpr DWORD MaxImage = 128 * 1024 * 1024;
struct Slot { void** address; void* original; void* replacement; };
SRWLOCK lock = SRWLOCK_INIT;
std::array<wchar_t, 65536> mapping{};
std::array<Slot, 3> installed{};
bool active = false;
DWORD owner = 0;

struct SharedLock {
    SharedLock() { AcquireSRWLockShared(&lock); }
    ~SharedLock() { ReleaseSRWLockShared(&lock); }
};
struct ExclusiveLock {
    ExclusiveLock() { AcquireSRWLockExclusive(&lock); }
    ~ExclusiveLock() { ReleaseSRWLockExclusive(&lock); }
};
bool fail(DWORD error = ERROR_INVALID_DATA) { SetLastError(error); return false; }
bool readable(const void* pointer, size_t size) {
    auto start = reinterpret_cast<uintptr_t>(pointer);
    if (!start || size > UINTPTR_MAX - start) return false;
    const auto end = start + size;
    while (start < end) {
        MEMORY_BASIC_INFORMATION info{};
        if (!VirtualQuery(reinterpret_cast<const void*>(start), &info, sizeof(info)) ||
            info.State != MEM_COMMIT || (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)) ||
            !(info.Protect & (PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY |
                PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY))) return false;
        const auto next = reinterpret_cast<uintptr_t>(info.BaseAddress) + info.RegionSize;
        if (next <= start) return false;
        start = (std::min)(static_cast<uintptr_t>(next), end);
    }
    return true;
}
struct Image {
    const unsigned char* base;
    DWORD size;
    bool range(DWORD rva, size_t bytes) const {
        return rva <= size && bytes <= size - rva && readable(base + rva, bytes);
    }
    template<class T> bool read(DWORD rva, T& value) const {
        if (!range(rva, sizeof(T))) return false;
        memcpy(&value, base + rva, sizeof(T));
        return true;
    }
    bool name(DWORD rva, std::string& value) const {
        value.clear();
        for (DWORD i = 0; i < 256; ++i) {
            if (rva > size || i >= size - rva || !range(rva + i, 1)) return false;
            const char c = static_cast<char>(base[rva + i]);
            if (!c) return true;
            value.push_back(c);
        }
        return false;
    }
};

// Structural validation also runs on owned malformed fixtures, without patching them.
// Every dereference is checked against both the supplied image span and committed pages.
bool discover(const void* base, DWORD bytes, std::array<DWORD, 3>& offsets) {
    if (!base || bytes < sizeof(IMAGE_DOS_HEADER) || bytes > MaxImage) return false;
    Image image{static_cast<const unsigned char*>(base), bytes};
    IMAGE_DOS_HEADER dos{};
    IMAGE_NT_HEADERS32 nt{};
    if (!image.read(0, dos) || dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < 0 ||
        !image.read(static_cast<DWORD>(dos.e_lfanew), nt) || nt.Signature != IMAGE_NT_SIGNATURE ||
        nt.FileHeader.Machine != IMAGE_FILE_MACHINE_I386 ||
        nt.FileHeader.SizeOfOptionalHeader != sizeof(IMAGE_OPTIONAL_HEADER32) ||
        nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR32_MAGIC ||
        nt.OptionalHeader.NumberOfRvaAndSizes != IMAGE_NUMBEROF_DIRECTORY_ENTRIES ||
        nt.OptionalHeader.SizeOfImage != bytes ||
        !(nt.FileHeader.Characteristics & IMAGE_FILE_EXECUTABLE_IMAGE) ||
        (nt.FileHeader.Characteristics & IMAGE_FILE_DLL)) return false;
    const auto imports = nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
    const auto iat = nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IAT];
    if (!imports.VirtualAddress || !iat.VirtualAddress ||
        imports.Size < sizeof(IMAGE_IMPORT_DESCRIPTOR) || iat.Size < sizeof(DWORD) ||
        !image.range(imports.VirtualAddress, imports.Size) ||
        !image.range(iat.VirtualAddress, iat.Size)) return false;
    offsets.fill(0);
    bool terminated = false;
    for (DWORD d = 0; d <= imports.Size - sizeof(IMAGE_IMPORT_DESCRIPTOR);
        d += sizeof(IMAGE_IMPORT_DESCRIPTOR)) {
        IMAGE_IMPORT_DESCRIPTOR descriptor{};
        if (!image.read(imports.VirtualAddress + d, descriptor)) return false;
        if (!descriptor.Name && !descriptor.FirstThunk && !descriptor.OriginalFirstThunk &&
            !descriptor.TimeDateStamp && !descriptor.ForwarderChain) { terminated = true; break; }
        std::string module;
        if (!descriptor.OriginalFirstThunk || !descriptor.FirstThunk ||
            !image.name(descriptor.Name, module)) return false;
        bool thunkEnd = false;
        for (DWORD i = 0; i < bytes / sizeof(DWORD); ++i) {
            const DWORD step = i * sizeof(DWORD);
            if (descriptor.OriginalFirstThunk > bytes || step > bytes - descriptor.OriginalFirstThunk ||
                descriptor.FirstThunk > bytes || step > bytes - descriptor.FirstThunk) return false;
            const DWORD slot = descriptor.FirstThunk + step;
            DWORD thunk = 0, target = 0;
            if (slot % sizeof(void*) || slot < iat.VirtualAddress ||
                slot - iat.VirtualAddress > iat.Size - sizeof(DWORD) ||
                !image.read(descriptor.OriginalFirstThunk + step, thunk) ||
                !image.read(slot, target)) return false;
            if (!thunk) { if (target) return false; thunkEnd = true; break; }
            if (!target) return false;
            if (IMAGE_SNAP_BY_ORDINAL32(thunk)) continue;
            std::string function;
            if (!image.range(thunk, sizeof(WORD)) || thunk > bytes - sizeof(WORD) ||
                !image.name(thunk + sizeof(WORD), function)) return false;
            int index = -1;
            if (_stricmp(module.c_str(), "kernel32.dll") == 0 && function == "MultiByteToWideChar") index = 0;
            if (_stricmp(module.c_str(), "gdi32.dll") == 0 && function == "TextOutA") index = 1;
            if (_stricmp(module.c_str(), "gdi32.dll") == 0 && function == "GetTextExtentPoint32A") index = 2;
            if (index >= 0) {
                if (offsets[static_cast<size_t>(index)]) return false;
                offsets[static_cast<size_t>(index)] = slot;
            }
        }
        if (!thunkEnd) return false;
    }
    return terminated && offsets[0] && offsets[1] && offsets[2] &&
        offsets[0] != offsets[1] && offsets[0] != offsets[2] && offsets[1] != offsets[2];
}
DWORD mainSpan(HMODULE module) {
    const auto base = reinterpret_cast<uintptr_t>(module);
    uintptr_t cursor = base;
    while (cursor - base < MaxImage) {
        MEMORY_BASIC_INFORMATION info{};
        if (!VirtualQuery(reinterpret_cast<void*>(cursor), &info, sizeof(info)) ||
            info.AllocationBase != module) break;
        if (!info.RegionSize || info.RegionSize > MaxImage - (cursor - base)) return 0;
        cursor += info.RegionSize;
    }
    return static_cast<DWORD>(cursor - base);
}
bool absolute(const wchar_t* path) {
    // Local absolute drive paths only; no device/UNC/network map access in this proof.
    return path && readable(path, 4 * sizeof(wchar_t)) &&
        ((path[0] >= L'A' && path[0] <= L'Z') || (path[0] >= L'a' && path[0] <= L'z')) &&
        path[1] == L':' && path[2] == L'\\';
}
bool loadMap(const wchar_t* path, const uni::Generation& generation, const uni::Digest& trusted,
    std::array<wchar_t, 65536>& result) {
    if (!absolute(path)) return false;
    HANDLE file = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    LARGE_INTEGER size{};
    BY_HANDLE_FILE_INFORMATION info{};
    bool ok = GetFileInformationByHandle(file, &info) &&
        !(info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)) &&
        GetFileSizeEx(file, &size) && size.QuadPart >= uni::HeaderSize &&
        size.QuadPart <= uni::HeaderSize + uni::Capacity * 4;
    std::vector<unsigned char> bytes;
    if (ok) {
        bytes.resize(static_cast<size_t>(size.QuadPart));
        DWORD count = 0;
        ok = ReadFile(file, bytes.data(), static_cast<DWORD>(bytes.size()), &count, nullptr) && count == bytes.size();
    }
    CloseHandle(file);
    if (!ok || memcmp(bytes.data(), uni::Magic, 8) || uni::u32(bytes.data() + 8) != uni::Version)
        return false;
    const auto count = uni::u32(bytes.data() + 12);
    uni::Digest computed{};
    if (!count || count > uni::Capacity || bytes.size() != uni::HeaderSize + count * 4 ||
        memcmp(bytes.data() + 16, generation.data(), generation.size()) ||
        !uni::mapDigest(bytes, computed) || computed != trusted ||
        memcmp(bytes.data() + 32, computed.data(), computed.size())) return false;
    result.fill(0);
    std::array<bool, 65536> used{};
    for (size_t i = uni::HeaderSize; i < bytes.size(); i += 4) {
        const auto lead = bytes[i], trail = bytes[i + 1];
        const auto code = uni::u16(bytes.data() + i + 2);
        const size_t pair = (static_cast<size_t>(lead) << 8) | trail;
        if (!uni::candidate(lead, trail) || code < 0x20 || (code >= 0x7f && code <= 0x9f) ||
            (code >= 0xd800 && code <= 0xdfff) || (code >= 0xfdd0 && code <= 0xfdef) ||
            code >= 0xfffe || result[pair] || used[code]) return false;
        result[pair] = static_cast<wchar_t>(code);
        used[code] = true;
    }
    return true;
}

bool decode(const char* input, int count, bool allowNulTerminated, std::wstring& text) {
    if (!input || count < (allowNulTerminated ? -1 : 0) || count > uni::MaxText) return fail(ERROR_INVALID_PARAMETER);
    if (count == -1) {
        count = 0;
        for (; count < uni::MaxText; ++count) {
            if (!readable(input + count, 1)) return fail(ERROR_INVALID_PARAMETER);
            if (!input[count]) { ++count; break; }
        }
        if (!count || input[count - 1] != '\0') return fail(ERROR_INVALID_PARAMETER);
    }
    if (!readable(input, static_cast<size_t>(count))) return fail(ERROR_INVALID_PARAMETER);
    text.clear();
    text.reserve(static_cast<size_t>(count));
    for (int i = 0; i < count;) {
        const auto lead = static_cast<unsigned char>(input[i]);
        const bool doubleByte = (lead >= 0x81 && lead <= 0x9f) || (lead >= 0xe0 && lead <= 0xfc);
        const int width = doubleByte ? 2 : 1;
        if (width > count - i) return fail(ERROR_NO_UNICODE_TRANSLATION);
        if (doubleByte) {
            const auto trail = static_cast<unsigned char>(input[i + 1]);
            if (uni::candidate(lead, trail)) {
                const wchar_t code = mapping[(static_cast<size_t>(lead) << 8) | trail];
                if (!code) return fail(ERROR_NO_UNICODE_TRANSLATION);
                text.push_back(code);
                i += 2;
                continue;
            }
            if (trail < 0x40 || trail == 0x7f || trail > 0xfc) return fail(ERROR_NO_UNICODE_TRANSLATION);
        }
        wchar_t code = 0;
        if (::MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, input + i, width, &code, 1) != 1)
            return fail(ERROR_NO_UNICODE_TRANSLATION);
        text.push_back(code);
        i += width;
    }
    return true;
}
int WINAPI convert(UINT page, DWORD flags, LPCCH input, int count, LPWSTR output, int capacity) {
    if (page != 932) return ::MultiByteToWideChar(page, flags, input, count, output, capacity);
    SharedLock guard;
    try {
        if (!active || (flags != 0 && flags != MB_ERR_INVALID_CHARS)) return fail(ERROR_INVALID_FLAGS);
        if (!count || capacity < 0 || (capacity && !output)) return fail(ERROR_INVALID_PARAMETER);
        std::wstring text;
        if (!decode(input, count, true, text)) return 0;
        if (!capacity) return static_cast<int>(text.size());
        if (static_cast<size_t>(capacity) < text.size()) return fail(ERROR_INSUFFICIENT_BUFFER);
        // Explicit lengths retain embedded NULs; -1 includes exactly the terminator.
        memcpy(output, text.data(), text.size() * sizeof(wchar_t));
        return static_cast<int>(text.size());
    } catch (...) { return fail(ERROR_NOT_ENOUGH_MEMORY); }
}
BOOL WINAPI draw(HDC dc, int x, int y, LPCSTR input, int count) {
    SharedLock guard;
    try {
        std::wstring text;
        if (!active || !decode(input, count, false, text)) return FALSE;
        return ::TextOutW(dc, x, y, text.data(), static_cast<int>(text.size()));
    } catch (...) { return fail(ERROR_NOT_ENOUGH_MEMORY); }
}
BOOL WINAPI extent(HDC dc, LPCSTR input, int count, LPSIZE size) {
    SharedLock guard;
    try {
        std::wstring text;
        if (!active || !size || !decode(input, count, false, text)) return FALSE;
        return ::GetTextExtentPoint32W(dc, text.data(), static_cast<int>(text.size()), size);
    } catch (...) { return fail(ERROR_NOT_ENOUGH_MEMORY); }
}

// The owning host must quiesce callers during lifecycle operations. Each pointer
// exchange is atomic; the three-slot transaction is not a multiword CPU primitive.
// Fault injection belongs only to this synthetic proof (0 = no fault, 1..3 = slot).
bool transaction(const std::array<Slot, 3>& slots, bool installing, DWORD fault) {
    struct Page { void* address; DWORD protection; };
    std::vector<Page> pages;
    SYSTEM_INFO system{};
    GetSystemInfo(&system);
    for (const auto& slot : slots) {
        if (*slot.address != (installing ? slot.original : slot.replacement)) return false;
        const auto address = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(slot.address) &
            ~(static_cast<uintptr_t>(system.dwPageSize) - 1));
        if (std::any_of(pages.begin(), pages.end(), [address](const Page& p) { return p.address == address; })) continue;
        MEMORY_BASIC_INFORMATION info{};
        if (!VirtualQuery(address, &info, sizeof(info)) || info.State != MEM_COMMIT ||
            info.Type != MEM_IMAGE || info.AllocationBase != GetModuleHandleW(nullptr) ||
            (info.Protect != PAGE_READONLY && info.Protect != PAGE_READWRITE && info.Protect != PAGE_WRITECOPY))
            return false;
        pages.push_back({address, info.Protect});
    }
    auto restorePages = [&]() {
        bool ok = true;
        for (const auto& page : pages) {
            DWORD ignored = 0;
            if (!VirtualProtect(page.address, system.dwPageSize, page.protection, &ignored)) ok = false;
        }
        return ok;
    };
    for (const auto& page : pages) {
        DWORD ignored = 0;
        if (!VirtualProtect(page.address, system.dwPageSize, PAGE_READWRITE, &ignored)) {
            if (!restorePages()) RaiseFailFastException(nullptr, nullptr, 0);
            return false;
        }
    }
    size_t changed = 0;
    for (; changed < slots.size(); ++changed) {
        const auto& slot = slots[changed];
        if (fault == changed + 1 || InterlockedCompareExchangePointer(slot.address,
            installing ? slot.replacement : slot.original,
            installing ? slot.original : slot.replacement) != (installing ? slot.original : slot.replacement)) break;
    }
    const bool complete = changed == slots.size();
    if (complete && restorePages()) return true;
    // Roll back on exchange/protection failure. Invariant loss is fatal, never an
    // apparently safe return with dangling imports or writable executable pages.
    for (const auto& page : pages) {
        DWORD ignored = 0;
        if (!VirtualProtect(page.address, system.dwPageSize, PAGE_READWRITE, &ignored))
            RaiseFailFastException(nullptr, nullptr, 0);
    }
    while (changed) {
        const auto& slot = slots[--changed];
        if (InterlockedCompareExchangePointer(slot.address,
            installing ? slot.original : slot.replacement,
            installing ? slot.replacement : slot.original) != (installing ? slot.replacement : slot.original))
            RaiseFailFastException(nullptr, nullptr, 0);
    }
    if (!restorePages()) RaiseFailFastException(nullptr, nullptr, 0);
    return false;
}
} // namespace

extern "C" BOOL __cdecl UniProbeImage(const void* image, DWORD bytes) {
    try { std::array<DWORD, 3> slots{}; return discover(image, bytes, slots); }
    catch (...) { return FALSE; }
}
extern "C" BOOL __cdecl UniInitialize(const wchar_t* path, const uni::Generation* generation,
    const uni::Digest* digest, DWORD fault) {
    ExclusiveLock guard;
    try {
        if (active || fault > 3 || !generation || !digest) return fail();
        // Heap storage avoids a large stack frame; published only after full validation.
        auto proposed = std::vector<std::array<wchar_t, 65536>>(1);
        if (!loadMap(path, *generation, *digest, proposed[0])) return fail();
        HMODULE main = GetModuleHandleW(nullptr);
        std::array<DWORD, 3> offsets{};
        if (!discover(main, mainSpan(main), offsets)) return fail(ERROR_BAD_EXE_FORMAT);
        const std::array<void*, 3> originals = {reinterpret_cast<void*>(::MultiByteToWideChar),
            reinterpret_cast<void*>(::TextOutA), reinterpret_cast<void*>(::GetTextExtentPoint32A)};
        const std::array<void*, 3> replacements = {reinterpret_cast<void*>(convert),
            reinterpret_cast<void*>(draw), reinterpret_cast<void*>(extent)};
        std::array<Slot, 3> slots{};
        for (size_t i = 0; i < slots.size(); ++i) {
            slots[i] = {reinterpret_cast<void**>(reinterpret_cast<unsigned char*>(main) + offsets[i]),
                originals[i], replacements[i]};
        }
        mapping = proposed[0];
        if (!transaction(slots, true, fault)) { mapping.fill(0); return fail(); }
        installed = slots;
        owner = GetCurrentThreadId();
        active = true;
        return TRUE;
    } catch (...) { return fail(ERROR_NOT_ENOUGH_MEMORY); }
}
extern "C" BOOL __cdecl UniUninstall(DWORD fault) {
    ExclusiveLock guard;
    try {
        if (!active || owner != GetCurrentThreadId() || fault > 3) return fail();
        if (!transaction(installed, false, fault)) return fail();
        active = false;
        mapping.fill(0);
        installed = {};
        owner = 0;
        return TRUE;
    } catch (...) { return fail(ERROR_NOT_ENOUGH_MEMORY); }
}
// No custom DllMain: no file reads, hooks, thread creation or proxy auto-loading.

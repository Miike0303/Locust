#include "runtime_map.h"
#include <algorithm>
#include <cstdio>
#include <string>

namespace {
int passed = 0, failed = 0;
void check(bool ok, const char* name) {
    printf("%s %s\n", ok ? "PASS" : "FAIL", name);
    if (ok) ++passed; else ++failed;
}
bool save(const std::wstring& path, const std::vector<unsigned char>& bytes) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    DWORD written = 0;
    const bool ok = WriteFile(file, bytes.data(), static_cast<DWORD>(bytes.size()),
        &written, nullptr) && written == bytes.size();
    CloseHandle(file);
    return ok;
}
void put16(std::vector<unsigned char>& bytes, size_t at, unsigned value) {
    bytes.at(at) = static_cast<unsigned char>(value);
    bytes.at(at + 1) = static_cast<unsigned char>(value >> 8);
}
void put32(std::vector<unsigned char>& bytes, size_t at, DWORD value) {
    for (unsigned i = 0; i < 4; ++i) bytes.at(at + i) = static_cast<unsigned char>(value >> (8 * i));
}
uni::Digest seal(std::vector<unsigned char>& bytes) {
    // Independent schema assembly: authenticate prefix [0,32) and entries [64,end).
    std::vector<unsigned char> authenticated;
    for (size_t i = 0; i < bytes.size(); ++i) if (i < 32 || i >= 64) authenticated.push_back(bytes[i]);
    uni::Digest digest{};
    if (!uni::sha256(authenticated.data(), authenticated.size(), digest)) throw "CNG SHA256 failed";
    memcpy(bytes.data() + 32, digest.data(), digest.size());
    return digest;
}
std::vector<unsigned char> fixture(const uni::Generation& generation) {
    std::vector<unsigned char> map(uni::HeaderSize, 0);
    memcpy(map.data(), uni::Magic, 8);
    put32(map, 8, 1);
    put32(map, 12, 4);
    memcpy(map.data() + 16, generation.data(), generation.size());
    const unsigned char entries[] = {0x81, 1, 0xf1, 0, 0x81, 2, 0xe9, 0,
        0x81, 3, 0xa1, 0, 0x81, 4, 0xbf, 0};
    map.insert(map.end(), std::begin(entries), std::end(entries));
    seal(map);
    return map;
}
// Independent observation of this compiler-built image, not the runtime parser.
struct Imports {
    std::array<void**, 3> slots{};
    std::array<void*, 3> original{};
    std::array<DWORD, 3> protection{};
    DWORD* table = nullptr;
    std::vector<DWORD> snapshot;
    explicit Imports(HMODULE module) {
        auto base = reinterpret_cast<unsigned char*>(module);
        const auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
        const auto nt = reinterpret_cast<IMAGE_NT_HEADERS32*>(base + dos->e_lfanew);
        const auto iat = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IAT];
        table = reinterpret_cast<DWORD*>(base + iat.VirtualAddress);
        snapshot.assign(table, table + iat.Size / sizeof(DWORD));
        auto descriptor = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base +
            nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress);
        for (; descriptor->Name; ++descriptor) {
            auto names = reinterpret_cast<IMAGE_THUNK_DATA32*>(base + descriptor->OriginalFirstThunk);
            auto targets = reinterpret_cast<void**>(base + descriptor->FirstThunk);
            for (size_t i = 0; names[i].u1.AddressOfData; ++i) {
                if (IMAGE_SNAP_BY_ORDINAL32(names[i].u1.Ordinal)) continue;
                const auto name = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + names[i].u1.AddressOfData)->Name;
                int index = -1;
                if (!strcmp(name, "MultiByteToWideChar")) index = 0;
                if (!strcmp(name, "TextOutA")) index = 1;
                if (!strcmp(name, "GetTextExtentPoint32A")) index = 2;
                if (index < 0) continue;
                const auto j = static_cast<size_t>(index);
                slots[j] = targets + i;
                original[j] = targets[i];
                MEMORY_BASIC_INFORMATION info{};
                VirtualQuery(slots[j], &info, sizeof(info));
                protection[j] = info.Protect;
            }
        }
    }
    bool unchanged() const { return std::equal(snapshot.begin(), snapshot.end(), table); }
    bool protections() const {
        for (size_t i = 0; i < slots.size(); ++i) {
            if (!slots[i]) return false;
            MEMORY_BASIC_INFORMATION info{};
            if (!VirtualQuery(slots[i], &info, sizeof(info)) || info.Protect != protection[i]) return false;
        }
        return true;
    }
    bool onlyThreeChanged() const {
        size_t changed = 0;
        for (size_t i = 0; i < snapshot.size(); ++i) {
            if (snapshot[i] == table[i]) continue;
            ++changed;
            if (std::none_of(slots.begin(), slots.end(), [&](void** p) {
                return reinterpret_cast<DWORD*>(p) == table + i;
            })) return false;
        }
        return changed == 3 && protections();
    }
};
struct Canvas {
    static constexpr int Width = 512, Height = 96;
    HDC dc = CreateCompatibleDC(nullptr);
    HBITMAP bitmap = nullptr;
    HGDIOBJ old = nullptr;
    void* pixels = nullptr;
    Canvas() {
        BITMAPINFO info{};
        info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
        info.bmiHeader.biWidth = Width;
        info.bmiHeader.biHeight = -Height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        if (dc) bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &pixels, nullptr, 0);
        if (bitmap) old = SelectObject(dc, bitmap);
        if (dc) { SetTextColor(dc, RGB(0, 0, 0)); SetBkMode(dc, TRANSPARENT); }
    }
    bool valid() const { return dc && bitmap && old && old != HGDI_ERROR && pixels; }
    void clear() { GdiFlush(); memset(pixels, 0xff, Width * Height * 4); }
    ~Canvas() {
        if (old && old != HGDI_ERROR) SelectObject(dc, old);
        if (bitmap) DeleteObject(bitmap);
        if (dc) DeleteDC(dc);
    }
};
const char Spanish[] = "\x81\x01\x81\x02\x81\x03\x81\x04";
const wchar_t SpanishW[] = L"\u00f1\u00e9\u00a1\u00bf";
bool converted(const char* bytes, int length, const std::wstring& expected) {
    std::array<wchar_t, 256> result{};
    const int count = MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, bytes, length,
        result.data(), static_cast<int>(result.size()));
    return count == static_cast<int>(expected.size()) &&
        std::equal(expected.begin(), expected.end(), result.begin());
}
void conversionTests() {
    check(converted(Spanish, 8, SpanishW), "actual import: exact Spanish n-tilde/e-acute/inverted punctuation");
    const char mixed[] = "A\x81\x01\x93\xfa\x96\x7b\x8c\xea\x81\x02\x81\x01!";
    check(converted(mixed, sizeof(mixed) - 1, L"A\u00f1\u65e5\u672c\u8a9e\u00e9\u00f1!"),
        "mixed/repeated Spanish + exact Japanese + ASCII conversion (not CJK font proof)");
    check(converted("ASCII", 5, L"ASCII"), "ordinary ASCII remains exact");
    check(converted("\xb6\xc0\xb6\xc5", 4, L"\uff76\uff80\uff76\uff85"), "ordinary CP932 halfwidth kana remains exact");
    check(converted("\xf0\x40", 2, L"\ue000"), "ordinary CP932 user-defined pair remains exact");
    check(converted(Spanish, 2, L"\u00f1"), "explicit length does not consume the next pair");
    const char nul[] = {'A', 0, static_cast<char>(0x81), 1, 0};
    check(converted(nul, 4, std::wstring(L"A\0\u00f1", 3)), "explicit length preserves embedded NUL");
    check(converted(nul, -1, std::wstring(L"A\0", 2)), "minus-one stops at first NUL and includes terminator");
    check(converted(Spanish, -1, std::wstring(SpanishW, 5)), "minus-one mapped input includes terminator");
    check(MultiByteToWideChar(932, 0, Spanish, 8, nullptr, 0) == 4, "zero-capacity size query");
    wchar_t result[8];
    std::fill(std::begin(result), std::end(result), L'Z');
    check(!MultiByteToWideChar(932, 0, Spanish, 8, result, 3) && GetLastError() == ERROR_INSUFFICIENT_BUFFER &&
        std::all_of(std::begin(result), std::end(result), [](wchar_t c) { return c == L'Z'; }),
        "short output fails without partial writes");
    check(!MultiByteToWideChar(932, MB_COMPOSITE, Spanish, 8, result, 8) && GetLastError() == ERROR_INVALID_FLAGS,
        "unsupported CP932 flags fail closed");
    check(!MultiByteToWideChar(932, 0, Spanish, 0, result, 8) &&
        !MultiByteToWideChar(932, 0, Spanish, -2, result, 8) &&
        !MultiByteToWideChar(932, 0, Spanish, uni::MaxText + 1, result, 8) &&
        !MultiByteToWideChar(932, 0, nullptr, 1, result, 8) &&
        !MultiByteToWideChar(932, 0, Spanish, 8, nullptr, 4) &&
        !MultiByteToWideChar(932, 0, Spanish, 8, result, -1), "invalid conversion lengths and pointers refused");
    const char* bad[] = {"\x81", "\x81\x05", "\x81\x09", "\xfc\x01", "\x81\x7f", "\x81\x00"};
    bool rejected = true;
    for (size_t i = 0; i < std::size(bad); ++i) {
        if (MultiByteToWideChar(932, 0, bad[i], i == 0 ? 1 : 2, result, 8)) rejected = false;
    }
    check(rejected, "truncated/unknown/reserved/FC/invalid/NUL trails refused without substitution");
    const auto native = reinterpret_cast<decltype(&MultiByteToWideChar)>(
        GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "MultiByteToWideChar"));
    bool cp932Same = true;
    size_t ordinaryCases = 0, differences = 0;
    for (unsigned lead = 0; lead <= 255; ++lead) {
        const char single = static_cast<char>(lead);
        wchar_t expected[2]{}, actual[2]{};
        const int n = native(932, MB_ERR_INVALID_CHARS, &single, 1, expected, 2);
        const int m = MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, &single, 1, actual, 2);
        // Native failure output is unspecified (Windows may write U+30FB before
        // returning zero). Our stronger failure contract leaves output untouched.
        if (n != m || (n > 0 && memcmp(expected, actual, sizeof(expected))) ||
            (m == 0 && (actual[0] || actual[1]))) {
            cp932Same = false;
            if (++differences <= 8) printf("CP932_DIFF byte=%02x native=%d/%04x hook=%d/%04x\n", lead, n, expected[0], m, actual[0]);
        }
        ++ordinaryCases;
        if (!((lead >= 0x81 && lead <= 0x9f) || (lead >= 0xe0 && lead <= 0xfc))) continue;
        for (unsigned trail = 0x40; trail <= 0xfc; ++trail) {
            if (trail == 0x7f) continue;
            const char pair[] = {static_cast<char>(lead), static_cast<char>(trail)};
            std::fill(std::begin(expected), std::end(expected), L'\0');
            std::fill(std::begin(actual), std::end(actual), L'\0');
            const int a = native(932, MB_ERR_INVALID_CHARS, pair, 2, expected, 2);
            const int b = MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, pair, 2, actual, 2);
            if (a != b || (a > 0 && memcmp(expected, actual, sizeof(expected))) ||
                (b == 0 && (actual[0] || actual[1]))) {
                cp932Same = false;
                if (++differences <= 8) printf("CP932_DIFF pair=%02x%02x native=%d/%04x hook=%d/%04x\n", lead, trail, a, expected[0], b, actual[0]);
            }
            ++ordinaryCases;
        }
    }
    check(cp932Same, "all ordinary CP932 single bytes and legal double-byte shapes match strict native conversion");
    printf("CP932 ordinary_cases=%zu differences=%zu CJK_font_claim=false\n", ordinaryCases, differences);
    bool same = true;
    for (const UINT page : std::array<UINT, 4>{CP_UTF8, 1252u, CP_ACP, CP_THREAD_ACP}) {
        wchar_t expected[16]{}, actual[16]{};
        SetLastError(123);
        const int n = native(page, 0, "A\xc3\xb1", 3, expected, 16);
        const DWORD error = GetLastError();
        SetLastError(123);
        const int m = MultiByteToWideChar(page, 0, "A\xc3\xb1", 3, actual, 16);
        if (n != m || GetLastError() != error || memcmp(expected, actual, sizeof(expected))) same = false;
    }
    check(same, "UTF8/1252/ACP/thread-ACP pass original API unchanged");
    std::string maximum(uni::MaxText, 'A');
    std::vector<wchar_t> maximumOutput(uni::MaxText);
    check(MultiByteToWideChar(932, 0, maximum.data(), uni::MaxText, maximumOutput.data(), uni::MaxText) == uni::MaxText &&
        std::all_of(maximumOutput.begin(), maximumOutput.end(), [](wchar_t c) { return c == L'A'; }),
        "maximum explicit byte length is exact");
    check(!MultiByteToWideChar(932, 0, maximum.c_str(), -1, nullptr, 0), "NUL beyond maximum input bound rejected");
    maximum.back() = '\0';
    check(MultiByteToWideChar(932, 0, maximum.data(), -1, nullptr, 0) == uni::MaxText,
        "NUL at maximum input boundary includes terminator");
    SYSTEM_INFO system{};
    GetSystemInfo(&system);
    auto guard = static_cast<char*>(VirtualAlloc(nullptr, system.dwPageSize * 2, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE));
    if (!guard) { check(false, "guard allocation"); return; }
    DWORD old = 0;
    const bool protectedPage = VirtualProtect(guard + system.dwPageSize, system.dwPageSize, PAGE_NOACCESS, &old) != FALSE;
    char* end = guard + system.dwPageSize - 1;
    *end = static_cast<char>(0x81);
    check(protectedPage && !MultiByteToWideChar(932, 0, end, 1, result, 8), "guard page: dangling lead does not overread");
    check(protectedPage && !MultiByteToWideChar(932, 0, end, -1, result, 8), "guard page: missing NUL scan fails without overread");
    *end = 0;
    check(protectedPage && converted(end, -1, std::wstring(1, L'\0')), "guard page: exact terminal NUL is in bounds");
    VirtualFree(guard, 0, MEM_RELEASE);
}
void renderingTests() {
    Canvas actual, reference;
    if (!actual.valid() || !reference.valid()) { check(false, "offscreen DIB creation"); return; }
    HFONT font = CreateFontW(-32, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE, DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, NONANTIALIASED_QUALITY, DEFAULT_PITCH, L"Segoe UI");
    if (!font) { check(false, "required installed Spanish font"); return; }
    const HGDIOBJ oldA = SelectObject(actual.dc, font), oldW = SelectObject(reference.dc, font);
    wchar_t face[LF_FACESIZE]{};
    WORD glyphs[4]{};
    const bool coverage = GetTextFaceW(actual.dc, LF_FACESIZE, face) > 0 &&
        !wcscmp(face, L"Segoe UI") && GetGlyphIndicesW(actual.dc, SpanishW, 4, glyphs,
            GGI_MARK_NONEXISTING_GLYPHS) != GDI_ERROR &&
        std::all_of(std::begin(glyphs), std::end(glyphs), [](WORD g) { return g != 0xffff && g != 0; });
    check(coverage, "installed Segoe UI has actual Spanish glyph indices (no fallback font claim)");
    printf("FONT Spanish glyph indices=%u,%u,%u,%u; CJK glyph coverage NOT asserted\n",
        glyphs[0], glyphs[1], glyphs[2], glyphs[3]);
    SIZE a{}, w{};
    check(GetTextExtentPoint32A(actual.dc, Spanish, 8, &a) &&
        GetTextExtentPoint32W(reference.dc, SpanishW, 4, &w) && a.cx == w.cx && a.cy == w.cy && a.cx > 0,
        "actual imported GetTextExtentPoint32A equals independent W metrics");
    printf("METRICS A=%ldx%ld W=%ldx%ld\n", a.cx, a.cy, w.cx, w.cy);
    bool pixels = coverage;
    size_t inkTotal = 0;
    // Whole string and each separate glyph must paint nonblank identical pixels.
    for (int sample = -1; sample < 4; ++sample) {
        actual.clear(); reference.clear();
        const char* bytes = sample < 0 ? Spanish : Spanish + sample * 2;
        const wchar_t* unicode = sample < 0 ? SpanishW : SpanishW + sample;
        const int n = sample < 0 ? 4 : 1;
        const bool drawn = TextOutA(actual.dc, 10, 10, bytes, n * 2) && TextOutW(reference.dc, 10, 10, unicode, n);
        GdiFlush();
        const auto p = static_cast<const DWORD*>(actual.pixels);
        size_t ink = 0;
        for (int i = 0; i < Canvas::Width * Canvas::Height; ++i) if ((p[i] & 0xffffff) != 0xffffff) ++ink;
        inkTotal += ink;
        if (!drawn || !ink || memcmp(actual.pixels, reference.pixels, Canvas::Width * Canvas::Height * 4)) pixels = false;
    }
    check(pixels, "actual imported TextOutA: whole Spanish and four individual DIBs equal TextOutW, nonblank");
    printf("PIXELS compared=5 ink_total=%zu no_on_screen_window=true\n", inkTotal);
    const char nul[] = {'A', 0, static_cast<char>(0x81), 1};
    const wchar_t nulW[] = {L'A', 0, L'\u00f1'};
    actual.clear(); reference.clear();
    const bool nulDraw = TextOutA(actual.dc, 10, 10, nul, 4) && TextOutW(reference.dc, 10, 10, nulW, 3);
    GdiFlush();
    check(nulDraw && !memcmp(actual.pixels, reference.pixels, Canvas::Width * Canvas::Height * 4) &&
        GetTextExtentPoint32A(actual.dc, nul, 4, &a) && GetTextExtentPoint32W(reference.dc, nulW, 3, &w) &&
        a.cx == w.cx && a.cy == w.cy, "GDI explicit length retains embedded NUL and exact following glyph");
    const char mixed[] = "A\x81\x01\x81\x02\x81\x01!";
    const wchar_t mixedW[] = L"A\u00f1\u00e9\u00f1!";
    actual.clear(); reference.clear();
    const bool mixedDraw = TextOutA(actual.dc, 10, 10, mixed, sizeof(mixed) - 1) &&
        TextOutW(reference.dc, 10, 10, mixedW, 5);
    GdiFlush();
    check(mixedDraw && !memcmp(actual.pixels, reference.pixels, Canvas::Width * Canvas::Height * 4) &&
        GetTextExtentPoint32A(actual.dc, mixed, sizeof(mixed) - 1, &a) &&
        GetTextExtentPoint32W(reference.dc, mixedW, 5, &w) && a.cx == w.cx && a.cy == w.cy,
        "GDI mixed ASCII and repeated Spanish has exact pixels and W metrics");
    check(GetTextExtentPoint32A(actual.dc, "\x93\xfa\x81\x01", 4, &a) &&
        GetTextExtentPoint32W(reference.dc, L"\u65e5\u00f1", 2, &w) && a.cx == w.cx && a.cy == w.cy,
        "mixed CJK/Spanish W metrics agree without claiming CJK glyph coverage");
    actual.clear();
    a = {123, 456};
    const bool refused = !TextOutA(actual.dc, 0, 0, "\x81", 1) &&
        !TextOutA(actual.dc, 0, 0, "\x81\x05", 2) &&
        !TextOutA(actual.dc, 0, 0, Spanish, -1) &&
        !TextOutA(actual.dc, 0, 0, Spanish, uni::MaxText + 1) &&
        !GetTextExtentPoint32A(actual.dc, "\x81", 1, &a) && a.cx == 123 && a.cy == 456 &&
        !GetTextExtentPoint32A(actual.dc, Spanish, -1, &a);
    GdiFlush();
    const auto raw = static_cast<const unsigned char*>(actual.pixels);
    check(refused && std::all_of(raw, raw + Canvas::Width * Canvas::Height * 4,
        [](unsigned char b) { return b == 0xff; }), "malformed GDI input returns failure without pixels/metrics changes");
    check(TextOutA(actual.dc, 0, 0, "", 0) && GetTextExtentPoint32A(actual.dc, "", 0, &a),
        "GDI zero explicit length accepted");
    SelectObject(actual.dc, oldA); SelectObject(reference.dc, oldW);
    DeleteObject(font);
}
// Original synthetic PE-shaped buffer: never executed, never patched.
std::vector<unsigned char> imageFixture() {
    std::vector<unsigned char> bytes(4096, 0);
    IMAGE_DOS_HEADER dos{};
    dos.e_magic = IMAGE_DOS_SIGNATURE; dos.e_lfanew = 128;
    memcpy(bytes.data(), &dos, sizeof(dos));
    IMAGE_NT_HEADERS32 nt{};
    nt.Signature = IMAGE_NT_SIGNATURE;
    nt.FileHeader.Machine = IMAGE_FILE_MACHINE_I386;
    nt.FileHeader.Characteristics = IMAGE_FILE_EXECUTABLE_IMAGE;
    nt.FileHeader.SizeOfOptionalHeader = sizeof(IMAGE_OPTIONAL_HEADER32);
    nt.OptionalHeader.Magic = IMAGE_NT_OPTIONAL_HDR32_MAGIC;
    nt.OptionalHeader.SizeOfImage = static_cast<DWORD>(bytes.size());
    nt.OptionalHeader.NumberOfRvaAndSizes = IMAGE_NUMBEROF_DIRECTORY_ENTRIES;
    nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT] = {512, 60};
    nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IAT] = {1024, 32};
    memcpy(bytes.data() + 128, &nt, sizeof(nt));
    IMAGE_IMPORT_DESCRIPTOR kernel{}, gdi{};
    kernel.Name = 700; kernel.OriginalFirstThunk = 800; kernel.FirstThunk = 1024;
    gdi.Name = 720; gdi.OriginalFirstThunk = 820; gdi.FirstThunk = 1040;
    memcpy(bytes.data() + 512, &kernel, sizeof(kernel));
    memcpy(bytes.data() + 532, &gdi, sizeof(gdi));
    memcpy(bytes.data() + 700, "kernel32.dll", 13); memcpy(bytes.data() + 720, "gdi32.dll", 10);
    put32(bytes, 800, 1200); put32(bytes, 820, 1240); put32(bytes, 824, 1280);
    memcpy(bytes.data() + 1202, "MultiByteToWideChar", 19);
    memcpy(bytes.data() + 1242, "TextOutA", 9);
    memcpy(bytes.data() + 1282, "GetTextExtentPoint32A", 21);
    put32(bytes, 1024, 1); put32(bytes, 1040, 2); put32(bytes, 1044, 3);
    return bytes;
}
void parserTests(uni::ProbeImage probe) {
    const auto good = imageFixture();
    check(probe(good.data(), static_cast<DWORD>(good.size())) != FALSE, "bounded synthetic PE32 has all three allowlisted imports");
    auto bad = good;
    auto reject = [&](const char* name) {
        check(!probe(bad.data(), static_cast<DWORD>(bad.size())), name); bad = good;
    };
    put16(bad, 132, IMAGE_FILE_MACHINE_AMD64); reject("wrong architecture rejected");
    put16(bad, 152, IMAGE_NT_OPTIONAL_HDR64_MAGIC); reject("PE32+ optional header rejected");
    put32(bad, 60, 0x7fffffff); reject("e_lfanew out of image rejected");
    put32(bad, 512, 4094); reject("truncated name thunk rejected");
    put32(bad, 512 + 16, 4092); reject("IAT outside declared directory rejected");
    put32(bad, 800, 4095); reject("import hint/name overflow rejected");
    bad[1242] = 'X'; reject("missing allowlisted API rejected");
    bad[720] = 'X'; reject("wrong API module rejected");
    put32(bad, 824, 1240); reject("duplicate allowlisted API rejected");
    put32(bad, 804, 1200); reject("unterminated/inconsistent thunk rejected");
    put32(bad, 552 + 12, 700); reject("missing descriptor terminator rejected");
    put16(bad, 148, sizeof(IMAGE_OPTIONAL_HEADER32) - 1); reject("wrong optional header size rejected");
    const size_t dirs = 128 + offsetof(IMAGE_NT_HEADERS32, OptionalHeader) + offsetof(IMAGE_OPTIONAL_HEADER32, DataDirectory);
    put32(bad, dirs + IMAGE_DIRECTORY_ENTRY_IMPORT * 8, 0xfffffff0); reject("import directory integer wrap rejected");
    put32(bad, dirs + IMAGE_DIRECTORY_ENTRY_IMPORT * 8 + 4, 19); reject("short descriptor directory rejected");
    put32(bad, dirs + IMAGE_DIRECTORY_ENTRY_IAT * 8 + 4, 0xffffffff); reject("IAT size overflow rejected");
    check(!probe(good.data(), 63) && !probe(nullptr, 4096) && !probe(good.data(), 0xffffffff), "image outer bounds rejected");
    SYSTEM_INFO system{}; GetSystemInfo(&system);
    auto memory = static_cast<unsigned char*>(VirtualAlloc(nullptr, system.dwPageSize * 2, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));
    if (!memory) { check(false, "PE guard allocation"); return; }
    DWORD old = 0;
    const bool protectedPage = VirtualProtect(memory + system.dwPageSize, system.dwPageSize, PAGE_NOACCESS, &old) != FALSE;
    memcpy(memory + system.dwPageSize - 64, good.data(), 64);
    check(protectedPage && !probe(memory + system.dwPageSize - 64, 4096), "guard page: malicious PE span cannot read inaccessible header");
    VirtualFree(memory, 0, MEM_RELEASE);
}
uni::Digest codeDigest() {
    const auto base = reinterpret_cast<const unsigned char*>(GetModuleHandleW(nullptr));
    const auto dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
    const auto nt = reinterpret_cast<const IMAGE_NT_HEADERS32*>(base + dos->e_lfanew);
    const auto sections = IMAGE_FIRST_SECTION(nt);
    std::vector<unsigned char> code;
    for (WORD i = 0; i < nt->FileHeader.NumberOfSections; ++i) {
        if (sections[i].Characteristics & IMAGE_SCN_MEM_EXECUTE) {
            const auto start = base + sections[i].VirtualAddress;
            code.insert(code.end(), start, start + sections[i].Misc.VirtualSize);
        }
    }
    uni::Digest digest{};
    if (code.empty() || !uni::sha256(code.data(), code.size(), digest)) throw "code digest failed";
    return digest;
}
int run(const std::wstring& directory) {
    const unsigned char abc[] = {'a', 'b', 'c'};
    const uni::Digest known = {0xba,0x78,0x16,0xbf,0x8f,0x01,0xcf,0xea,0x41,0x41,0x40,0xde,0x5d,0xae,0x22,0x23,
        0xb0,0x03,0x61,0xa3,0x96,0x17,0x7a,0x9c,0xb4,0x10,0xff,0x61,0xf2,0x00,0x15,0xad};
    uni::Digest observed{};
    check(uni::sha256(abc, 3, observed) && observed == known, "CNG SHA256 matches published abc test vector");
    uni::Generation generation{}; generation[0] = 1;
    const auto good = fixture(generation);
    uni::Digest trusted{}; memcpy(trusted.data(), good.data() + 32, trusted.size());
    const auto path = directory + L"\\valid.map";
    if (!save(path, good)) return 2;
    Imports host(GetModuleHandleW(nullptr));
    const auto originalCode = codeDigest();
    check(std::all_of(host.slots.begin(), host.slots.end(), [](void** p) { return p != nullptr; }),
        "host contains three real imports");
    check(!LoadLibraryExW((directory + L"\\missing-owned.dll").c_str(), nullptr,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) && host.unchanged(), "missing DLL leaves host imports unchanged");
    HMODULE dll = LoadLibraryExW((directory + L"\\locust_unicode_proof.dll").c_str(), nullptr,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!dll) { check(false, "absolute DLL load"); return 2; }
    Imports runtime(dll);
    check(host.unchanged(), "DLL load alone does not hook");
    check(!GetProcAddress(dll, "MissingRequiredApi") && host.unchanged(), "missing export lookup makes no changes");
    const auto init = reinterpret_cast<uni::Initialize>(GetProcAddress(dll, "UniInitialize"));
    const auto stop = reinterpret_cast<uni::Uninstall>(GetProcAddress(dll, "UniUninstall"));
    const auto probe = reinterpret_cast<uni::ProbeImage>(GetProcAddress(dll, "UniProbeImage"));
    if (!init || !stop || !probe) { check(false, "required ABI exports"); FreeLibrary(dll); return 2; }
    parserTests(probe);
    check(!stop(0) && host.unchanged(), "uninstall before init refused without writes");
    check(!init((directory + L"\\missing.map").c_str(), &generation, &trusted, 0) && host.unchanged(),
        "missing map installs no hooks");
    check(!init(L"valid.map", &generation, &trusted, 0) && host.unchanged(), "relative map path refused");
    check(!init(path.c_str(), nullptr, &trusted, 0) && !init(path.c_str(), &generation, nullptr, 0) &&
        !init(path.c_str(), &generation, &trusted, 4) && host.unchanged(), "null trust inputs and unsupported transaction fault refused");
    auto bad = good;
    size_t invalidIndex = 0;
    auto reject = [&](const char* name, bool reseal = true) {
        uni::Digest digest = trusted;
        if (reseal && bad.size() >= uni::HeaderSize) digest = seal(bad);
        // Preserve each negative artifact and avoid overwriting a recently read file.
        const auto badPath = directory + L"\\invalid-" + std::to_wstring(++invalidIndex) + L".map";
        const bool saved = save(badPath, bad);
        const DWORD saveError = GetLastError();
        const bool refused = saved && !init(badPath.c_str(), &generation, &digest, 0);
        const bool unchanged = host.unchanged(), protectedPages = host.protections();
        const bool ok = saved && refused && unchanged && protectedPages;
        if (!ok) printf("MAP_FAILURE saved=%d save_error=%lu refused=%d unchanged=%d protections=%d\n",
            saved, saveError, refused, unchanged, protectedPages);
        check(ok, name); bad = good;
    };
    bad[0] = 'X'; reject("wrong map magic rejected");
    put32(bad, 8, 2); reject("unknown version rejected");
    put32(bad, 12, 0); reject("zero entry count rejected");
    put32(bad, 12, 3423); reject("over-capacity count rejected");
    put32(bad, 12, 0xffffffff); reject("count arithmetic overflow rejected");
    bad.resize(20); reject("truncated header rejected", false);
    bad.pop_back(); reject("odd/truncated entry rejected");
    bad.push_back(0); reject("extra byte rejected");
    bad.resize(uni::HeaderSize + uni::Capacity * 4 + 1); reject("oversize file rejected");
    bad[68] = bad[64]; bad[69] = bad[65]; reject("duplicate pair rejected");
    bad[70] = bad[66]; bad[71] = bad[67]; reject("duplicate Unicode codepoint rejected");
    for (const unsigned trail : {0u, 9u, 10u, 13u, 0x20u, 0x2cu, 0x40u, 0x7fu}) {
        bad[65] = static_cast<unsigned char>(trail); reject("reserved/noncandidate trail rejected");
    }
    bad[64] = 0xfc; reject("FC lead outside prototype allocation rejected");
    bad[64] = 0x80; reject("nonlead pair rejected");
    for (const unsigned code : {0u, 0x1fu, 0x7fu, 0x9fu, 0xd800u, 0xdfffu, 0xfdd0u, 0xfffeu, 0xffffu}) {
        put16(bad, 66, code); reject("NUL/control/surrogate/noncharacter mapping rejected");
    }
    bad[16] ^= 1; reject("content generation must match trusted host");
    bad[32] ^= 1; reject("stored digest mismatch rejected", false);
    bad[66] ^= 1; reject("content digest corruption rejected", false);
    auto otherDigest = trusted; otherDigest[0] ^= 1;
    check(!init(path.c_str(), &generation, &otherDigest, 0) && host.unchanged(), "trusted host digest mismatch rejected");
    for (DWORD fault = 1; fault <= 3; ++fault) {
        check(!init(path.c_str(), &generation, &trusted, fault) && host.unchanged() && host.protections() && runtime.unchanged(),
            "injected install failure rolls every IAT pointer and protection back");
    }
    if (!init(path.c_str(), &generation, &trusted, 0)) { check(false, "validated initialization"); FreeLibrary(dll); return 1; }
    check(host.onlyThreeChanged() && runtime.unchanged(), "exactly three main-module IAT slots changed; DLL imports untouched");
    check(!stop(4) && host.onlyThreeChanged(), "unsupported uninstall fault leaves complete installed state");
    check(codeDigest() == originalCode, "executable memory sections unchanged after IAT installation");
    check(!init(path.c_str(), &generation, &trusted, 0) && host.onlyThreeChanged(), "duplicate initialization refused");
    conversionTests();
    renderingTests();
    // An initialized map is immutable even if its input file later changes.
    bad[66] ^= 1;
    check(save(path, bad) && converted(Spanish, 8, SpanishW), "active map is immutable (no lazy file reload)");
    for (DWORD fault = 1; fault <= 3; ++fault) {
        check(!stop(fault) && host.onlyThreeChanged() && converted(Spanish, 8, SpanishW),
            "injected uninstall failure restores the complete installed state");
    }
    const bool restored = stop(0) && host.unchanged() && host.protections() && runtime.unchanged();
    check(restored, "uninstall restores every original IAT pointer and page protection");
    check(codeDigest() == originalCode, "executable memory sections unchanged after proof and restoration");
    printf("API_RESTORE all_original_pointers=%s dll_imports_unchanged=%s\n",
        restored ? "true" : "false", runtime.unchanged() ? "true" : "false");
    if (!restored) return 1; // Never unload a DLL that might still own an import.
    wchar_t result[4]{};
    check(MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, Spanish, 8, result, 4) == 0,
        "uninstalled actual import returns to native CP932 rejection");
    // Exercise all 3,422 prototype pairs, including both allocation boundaries.
    std::vector<unsigned char> full(good.begin(), good.begin() + uni::HeaderSize);
    std::string encoded; std::wstring expected;
    for (unsigned lead = 0; lead <= 255; ++lead) for (unsigned trail = 0; trail <= 255; ++trail) {
        if (!uni::candidate(static_cast<unsigned char>(lead), static_cast<unsigned char>(trail))) continue;
        const unsigned code = 0x1000 + static_cast<unsigned>(expected.size());
        full.push_back(static_cast<unsigned char>(lead)); full.push_back(static_cast<unsigned char>(trail));
        full.push_back(static_cast<unsigned char>(code)); full.push_back(static_cast<unsigned char>(code >> 8));
        encoded.push_back(static_cast<char>(lead)); encoded.push_back(static_cast<char>(trail));
        expected.push_back(static_cast<wchar_t>(code));
    }
    put32(full, 12, static_cast<DWORD>(expected.size()));
    const auto fullHash = seal(full);
    std::vector<wchar_t> decoded(expected.size());
    const auto fullPath = directory + L"\\maximum.map";
    const bool fullInit = save(fullPath, full) && init(fullPath.c_str(), &generation, &fullHash, 0);
    check(fullInit && expected.size() == uni::Capacity &&
        MultiByteToWideChar(932, MB_ERR_INVALID_CHARS, encoded.data(), static_cast<int>(encoded.size()),
            decoded.data(), static_cast<int>(decoded.size())) == static_cast<int>(expected.size()) &&
        std::equal(expected.begin(), expected.end(), decoded.begin()), "all 3422 candidate pairs decode exactly; prototype only, not engine-safe allocation");
    if (fullInit) check(stop(0) && host.unchanged() && host.protections(), "maximum-map uninstall restoration");
    if (!host.unchanged()) return 1;
    check(FreeLibrary(dll) != FALSE && host.unchanged(), "safe unload after pointer restoration");
    return failed ? 1 : 0;
}
} // namespace
int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 2;
    try {
        const int result = run(argv[1]);
        printf("SUMMARY passed=%d failed=%d skipped=0 no_gui=true original_writes=0\n", passed, failed);
        return result;
    } catch (...) {
        printf("FAIL unexpected proof exception\nSUMMARY passed=%d failed=%d skipped=0\n", passed, failed + 1);
        return 2;
    }
}

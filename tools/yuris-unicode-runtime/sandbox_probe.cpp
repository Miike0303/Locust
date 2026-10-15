// Original, owned-fixture-only UNI-S experiment. No profile or registry mutation.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#define UNICODE
#define _UNICODE
#define _WIN32_WINNT 0x0A00
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <userenv.h>
#include <sddl.h>
#include <bcrypt.h>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#include <stdexcept>

namespace {
bool cleanupOk = true;
struct Failure { const char* operation; DWORD code; };
void require(bool ok, const char* op, DWORD code = MAXDWORD) {
    if (!ok) throw Failure{op, code == MAXDWORD ? GetLastError() : code};
}
struct Handle {
    HANDLE value = nullptr;
    Handle() = default;
    explicit Handle(HANDLE h) : value(h) {}
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
    bool close() {
        if (!value || value == INVALID_HANDLE_VALUE) { value = nullptr; return true; }
        const BOOL ok = CloseHandle(value); value = nullptr;
        if (!ok) cleanupOk = false;
        return ok != FALSE;
    }
    ~Handle() { close(); }
};
struct Socket {
    SOCKET value = INVALID_SOCKET;
    Socket() = default;
    Socket(const Socket&) = delete;
    ~Socket() { if (value != INVALID_SOCKET && closesocket(value) != 0) cleanupOk = false; }
};
struct Winsock {
    Winsock() { WSADATA data{}; const int e = WSAStartup(MAKEWORD(2, 2), &data);
        require(e == 0, "WSAStartup", static_cast<DWORD>(e)); }
    ~Winsock() { if (WSACleanup() != 0) cleanupOk = false; }
};
std::wstring modulePath() {
    wchar_t path[32768]{};
    const DWORD n = GetModuleFileNameW(nullptr, path, ARRAYSIZE(path));
    require(n != 0 && n < ARRAYSIZE(path), "module path");
    return std::wstring(path, n);
}
std::wstring parent(const std::wstring& path) {
    const auto pos = path.find_last_of(L'\\');
    require(pos != std::wstring::npos, "parent path", ERROR_INVALID_NAME);
    return path.substr(0, pos);
}
std::wstring quote(const std::wstring& text) {
    require(text.find(L'"') == std::wstring::npos && !text.empty() && text.back() != L'\\',
        "quote fixed fixture argument", ERROR_INVALID_NAME);
    return L"\"" + text + L"\"";
}
[[maybe_unused]] std::vector<BYTE> tokenData(HANDLE token, TOKEN_INFORMATION_CLASS kind) {
    DWORD bytes = 0;
    GetTokenInformation(token, kind, nullptr, 0, &bytes);
    require(bytes > 0 && bytes <= 65536, "token data size");
    std::vector<BYTE> data(bytes);
    require(GetTokenInformation(token, kind, data.data(), bytes, &bytes) != FALSE, "token data");
    return data;
}
[[maybe_unused]] DWORD tokenDword(HANDLE token, TOKEN_INFORMATION_CLASS kind) {
    DWORD value = 0, bytes = 0;
    require(GetTokenInformation(token, kind, &value, sizeof(value), &bytes) != FALSE && bytes == sizeof(value),
        "token DWORD");
    return value;
}
[[maybe_unused]] std::wstring sidText(PSID sid) {
    LPWSTR text = nullptr;
    require(ConvertSidToStringSidW(sid, &text) != FALSE, "SID text");
    const std::wstring result(text);
    LocalFree(text);
    return result;
}
[[maybe_unused]] std::wstring objectName(HANDLE object) {
    wchar_t name[256]{}; DWORD needed = 0;
    require(GetUserObjectInformationW(object, UOI_NAME, name, sizeof(name), &needed) != FALSE,
        "user object name");
    return name;
}
void writeExact(const std::wstring& path, const void* data, DWORD size, DWORD disposition,
    SECURITY_ATTRIBUTES* security = nullptr) {
    Handle file(CreateFileW(path.c_str(), GENERIC_WRITE, 0, security, disposition,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
    require(file.value != INVALID_HANDLE_VALUE, "write owned file open");
    DWORD written = 0;
    require(WriteFile(file.value, data, size, &written, nullptr) != FALSE && written == size,
        "write owned file bytes");
    require(FlushFileBuffers(file.value) != FALSE, "flush owned file");
}
[[maybe_unused]] std::vector<BYTE> readExact(const std::wstring& path, DWORD maximum) {
    Handle file(CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
    require(file.value != INVALID_HANDLE_VALUE, "read owned file open");
    BY_HANDLE_FILE_INFORMATION info{};
    require(GetFileInformationByHandle(file.value, &info) != FALSE &&
        !(info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)) &&
        info.nFileSizeHigh == 0 && info.nFileSizeLow <= maximum, "read owned file bound");
    std::vector<BYTE> bytes(info.nFileSizeLow);
    DWORD got = 0;
    require(ReadFile(file.value, bytes.data(), static_cast<DWORD>(bytes.size()), &got, nullptr) != FALSE &&
        got == bytes.size(), "read owned file bytes");
    return bytes;
}
constexpr DWORD reportMagic = 0x53494E55;
struct Report {
    DWORD magic = reportMagic;
    DWORD size = sizeof(Report);
    DWORD appContainer = 0;
    DWORD sidMatches = 0;
    DWORD capabilities = MAXDWORD;
    DWORD integrity = MAXDWORD;
    DWORD childPolicy = 0;
    DWORD outsideError = MAXDWORD;
    DWORD registryError = MAXDWORD;
    DWORD networkError = MAXDWORD;
    DWORD spawnError = MAXDWORD;
    DWORD allowedWrite = 0;
    DWORD inheritedHandleAbsent = 0;
    DWORD privateObjects = 0;
};
constexpr char pristine[] = "owned-before";
constexpr char changed[] = "owned-after!";
static_assert(sizeof(pristine) == sizeof(changed), "fixed sentinel length");

#ifdef LOCUST_FIXTURE
// The fixture derives all file targets from its own executable directory. There
// is no path or executable argument and no registry value read/write operation.
int fixture(int argc, wchar_t** argv) {
    if (argc == 2 && std::wstring(argv[1]) == L"--grandchild") return 91;
    require(argc == 8, "fixture argument count", ERROR_BAD_ARGUMENTS);
    const std::wstring mode(argv[1]), nonce(argv[2]), expectedSid(argv[3]);
    require((mode == L"--baseline" || mode == L"--probe" || mode == L"--hang") &&
        nonce.size() == 32 && nonce.find_first_not_of(L"0123456789abcdef") == std::wstring::npos,
        "fixture bounded mode/nonce", ERROR_BAD_ARGUMENTS);
    const std::wstring self = modulePath(), allowed = parent(self), root = parent(allowed);
    require(parent(allowed).substr(parent(allowed).find_last_of(L'\\') + 1) == L"case-" + nonce &&
        allowed.substr(allowed.find_last_of(L'\\') + 1) == L"allowed", "fixture owned layout", ERROR_INVALID_NAME);
    const unsigned long port = wcstoul(argv[4], nullptr, 10);
    const unsigned long inherited = wcstoul(argv[5], nullptr, 10);
    const std::wstring stationName(argv[7]);
    require(!stationName.empty() && stationName.size() < 256 && stationName.find_first_of(L"\\/\"") == std::wstring::npos &&
        _wcsicmp(stationName.c_str(), L"WinSta0") != 0, "fixture actual private station name", ERROR_INVALID_NAME);
    require(port > 0 && port <= 65535 && std::wstring(argv[6]) == L"owned-v1", "fixture protocol", ERROR_BAD_ARGUMENTS);
    if (mode == L"--hang") {
        writeExact(allowed + L"\\hang-ready.bin", &reportMagic, sizeof(reportMagic), CREATE_NEW);
        Sleep(60000);
        return 92;
    }
    Report report{};
    Handle token;
    require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token.value) != FALSE, "fixture token");
    report.appContainer = tokenDword(token.value, TokenIsAppContainer);
    if (report.appContainer) {
        auto sid = tokenData(token.value, TokenAppContainerSid);
        auto caps = tokenData(token.value, TokenCapabilities);
        auto integrity = tokenData(token.value, TokenIntegrityLevel);
        report.sidMatches = sidText(reinterpret_cast<TOKEN_APPCONTAINER_INFORMATION*>(sid.data())->TokenAppContainer) == expectedSid;
        report.capabilities = reinterpret_cast<TOKEN_GROUPS*>(caps.data())->GroupCount;
        const PSID il = reinterpret_cast<TOKEN_MANDATORY_LABEL*>(integrity.data())->Label.Sid;
        report.integrity = *GetSidSubAuthority(il, *GetSidSubAuthorityCount(il) - 1);
        PROCESS_MITIGATION_CHILD_PROCESS_POLICY policy{};
        require(GetProcessMitigationPolicy(GetCurrentProcess(), ProcessChildProcessPolicy, &policy, sizeof(policy)) != FALSE,
            "fixture child policy");
        report.childPolicy = policy.NoChildProcessCreation;
    }
    DWORD flags = 0;
    SetLastError(ERROR_SUCCESS);
    report.inheritedHandleAbsent = !GetHandleInformation(reinterpret_cast<HANDLE>(static_cast<ULONG_PTR>(inherited)), &flags) &&
        GetLastError() == ERROR_INVALID_HANDLE;
    report.privateObjects = objectName(GetProcessWindowStation()) == stationName &&
        objectName(GetThreadDesktop(GetCurrentThreadId())) == L"NonInput";
    // OPEN_EXISTING: even the intentionally uncontained baseline can only mutate
    // the sentinel the parent prepared; it cannot create an arbitrary target.
    {
        Handle outside(CreateFileW((root + L"\\outside.sentinel").c_str(), GENERIC_WRITE, 0, nullptr,
            OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
        report.outsideError = outside.value == INVALID_HANDLE_VALUE ? GetLastError() : ERROR_SUCCESS;
        if (outside.value != INVALID_HANDLE_VALUE) {
            DWORD written = 0;
            require(WriteFile(outside.value, changed, sizeof(changed), &written, nullptr) != FALSE &&
                written == sizeof(changed), "fixture own sentinel modification");
        }
    }
    writeExact(allowed + L"\\positive.txt", pristine, sizeof(pristine), CREATE_NEW);
    report.allowedWrite = 1;
    if (mode != L"--baseline") {
        HKEY key = nullptr;
        report.registryError = static_cast<DWORD>(RegOpenKeyExW(HKEY_CURRENT_USER, L"Software", 0, KEY_SET_VALUE, &key));
        if (key && RegCloseKey(key) != ERROR_SUCCESS) cleanupOk = false;
        Winsock winsock;
        Socket connection;
        connection.value = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        require(connection.value != INVALID_SOCKET, "fixture socket", static_cast<DWORD>(WSAGetLastError()));
        u_long nonblocking = 1;
        require(ioctlsocket(connection.value, FIONBIO, &nonblocking) == 0, "fixture nonblocking socket");
        sockaddr_in address{}; address.sin_family = AF_INET;
        address.sin_addr.s_addr = htonl(INADDR_LOOPBACK); address.sin_port = htons(static_cast<u_short>(port));
        const int connected = connect(connection.value, reinterpret_cast<sockaddr*>(&address), sizeof(address));
        report.networkError = connected == 0 ? 0 : static_cast<DWORD>(WSAGetLastError());
        if (report.networkError == WSAEWOULDBLOCK) {
            fd_set writable{}, exceptional{};
            FD_ZERO(&writable); FD_ZERO(&exceptional);
            FD_SET(connection.value, &writable); FD_SET(connection.value, &exceptional);
            timeval timeout{1, 0};
            const int ready = select(0, nullptr, &writable, &exceptional, &timeout);
            int error = 0, size = sizeof(error);
            if (ready > 0 && getsockopt(connection.value, SOL_SOCKET, SO_ERROR, reinterpret_cast<char*>(&error), &size) == 0)
                report.networkError = static_cast<DWORD>(error);
            else report.networkError = ready == 0 ? WSAETIMEDOUT : static_cast<DWORD>(WSAGetLastError());
        }
        STARTUPINFOW startup{}; startup.cb = sizeof(startup);
        PROCESS_INFORMATION child{};
        std::wstring command = quote(self) + L" --grandchild";
        const BOOL spawned = CreateProcessW(self.c_str(), command.data(), nullptr, nullptr, FALSE,
            CREATE_SUSPENDED | CREATE_NO_WINDOW, nullptr, allowed.c_str(), &startup, &child);
        report.spawnError = spawned ? ERROR_SUCCESS : GetLastError();
        if (spawned) {
            Handle process(child.hProcess), thread(child.hThread);
            require(TerminateProcess(process.value, 93) != FALSE && WaitForSingleObject(process.value, 3000) == WAIT_OBJECT_0,
                "terminate unexpected own grandchild");
        }
    }
    writeExact(allowed + L"\\report.bin", &report, sizeof(report), CREATE_NEW);
    return cleanupOk ? 0 : 94;
}
#else
#ifndef LOCUST_FIXTURE_SHA
#error Host requires the compiled owned-fixture SHA256 pin.
#endif
std::string hashBytes(const std::vector<BYTE>& bytes) {
    BYTE digest[32]{};
    require(BCryptHash(BCRYPT_SHA256_ALG_HANDLE, nullptr, 0, const_cast<PUCHAR>(bytes.data()),
        static_cast<ULONG>(bytes.size()), digest, sizeof(digest)) == 0, "SHA256", ERROR_INVALID_DATA);
    const char digits[] = "0123456789abcdef";
    std::string result;
    for (BYTE byte : digest) { result += digits[byte >> 4]; result += digits[byte & 15]; }
    return result;
}
std::wstring nonce() {
    BYTE random[16]{};
    require(BCryptGenRandom(nullptr, random, sizeof(random), BCRYPT_USE_SYSTEM_PREFERRED_RNG) == 0,
        "nonce", ERROR_INVALID_DATA);
    const wchar_t digits[] = L"0123456789abcdef";
    std::wstring result;
    for (BYTE byte : random) { result += digits[byte >> 4]; result += digits[byte & 15]; }
    return result;
}
struct Descriptor {
    PSECURITY_DESCRIPTOR value = nullptr;
    explicit Descriptor(const std::wstring& sddl) {
        require(ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(), SDDL_REVISION_1,
            &value, nullptr) != FALSE, "owned object security descriptor");
    }
    Descriptor(const Descriptor&) = delete;
    ~Descriptor() { if (value) LocalFree(value); }
    SECURITY_ATTRIBUTES attributes() const { return {sizeof(SECURITY_ATTRIBUTES), value, FALSE}; }
};
struct DerivedSid {
    PSID value = nullptr;
    explicit DerivedSid(const std::wstring& name) {
        const HRESULT result = DeriveAppContainerSidFromAppContainerName(name.c_str(), &value);
        require(SUCCEEDED(result) && value && IsValidSid(value), "derive profileless SID", static_cast<DWORD>(result));
    }
    DerivedSid(const DerivedSid&) = delete;
    ~DerivedSid() { if (value) FreeSid(value); }
};
void realDirectoryAncestors(std::wstring path) {
    while (path.size() > 3) {
        const DWORD attributes = GetFileAttributesW(path.c_str());
        require(attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) &&
            !(attributes & FILE_ATTRIBUTE_REPARSE_POINT), "owned path has only real directory ancestors");
        path = parent(path);
    }
}
void logUserObjectSecurity(HANDLE object, const char* kind) {
    // Read only the newly owned object's descriptor. LABEL queries do not
    // request audit-SACL access or enable SeSecurityPrivilege.
    for (SECURITY_INFORMATION information : {static_cast<SECURITY_INFORMATION>(OWNER_SECURITY_INFORMATION |
        GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION), static_cast<SECURITY_INFORMATION>(LABEL_SECURITY_INFORMATION)}) {
        DWORD needed = 0;
        SetLastError(ERROR_SUCCESS);
        const BOOL sized = GetUserObjectSecurity(object, &information, nullptr, 0, &needed);
        const DWORD sizeError = GetLastError();
        if (sized || sizeError != ERROR_INSUFFICIENT_BUFFER || !needed || needed > 65536) {
            std::printf("OBJECT_SECURITY kind=%s information=0x%08lx unavailable_error=%lu bytes=%lu\n",
                kind, information, sizeError, needed);
            continue;
        }
        std::vector<BYTE> descriptor(needed);
        const BOOL read = GetUserObjectSecurity(object, &information, descriptor.data(), needed, &needed);
        const DWORD readError = read ? ERROR_SUCCESS : GetLastError();
        if (!read) {
            std::printf("OBJECT_SECURITY kind=%s information=0x%08lx unavailable_error=%lu\n", kind, information, readError);
            continue;
        }
        LPWSTR text = nullptr;
        const BOOL converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor.data(), SDDL_REVISION_1,
            information, &text, nullptr);
        const DWORD convertError = converted ? ERROR_SUCCESS : GetLastError();
        if (converted) std::printf("OBJECT_SECURITY kind=%s information=0x%08lx sddl=%ls\n", kind, information, text);
        else std::printf("OBJECT_SECURITY kind=%s information=0x%08lx convert_error=%lu\n", kind, information, convertError);
        if (text) LocalFree(text);
    }
}
struct StationReference {
    HWINSTA value = nullptr;
    StationReference() = default;
    explicit StationReference(HWINSTA station) : value(station) {}
    StationReference(const StationReference&) = delete;
    StationReference& operator=(const StationReference&) = delete;
    bool close() {
        if (!value) return true;
        const BOOL closed = CloseWindowStation(value);
        if (closed) value = nullptr;
        else cleanupOk = false;
        return closed != FALSE;
    }
    ~StationReference() { close(); }
};
void logDiagnosticTokenScalar(HANDLE token, const char* kind, TOKEN_INFORMATION_CLASS information, const char* field) {
    // Diagnostics record the native return size rather than imposing the strict
    // child's DWORD contract on every host-token information class.
    DWORD value = 0, bytes = 0;
    const BOOL queried = GetTokenInformation(token, information, &value, sizeof(value), &bytes);
    const DWORD error = queried ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_TOKEN kind=%s field=%s available=%u bytes=%lu value=%lu error=%lu\n",
        kind, field, queried ? 1U : 0U, bytes, queried ? value : 0UL, error);
}
void logOwnToken(HANDLE token, const char* kind) {
    auto user = tokenData(token, TokenUser);
    auto owner = tokenData(token, TokenOwner);
    auto integrity = tokenData(token, TokenIntegrityLevel);
    auto groupsData = tokenData(token, TokenGroups);
    auto restricted = tokenData(token, TokenRestrictedSids);
    const PSID label = reinterpret_cast<TOKEN_MANDATORY_LABEL*>(integrity.data())->Label.Sid;
    const DWORD rid = *GetSidSubAuthority(label, *GetSidSubAuthorityCount(label) - 1);
    std::printf("DIAGNOSTIC_TOKEN kind=%s user_sid=%ls owner_sid=%ls integrity_sid=%ls integrity_rid=%lu\n", kind,
        sidText(reinterpret_cast<TOKEN_USER*>(user.data())->User.Sid).c_str(),
        sidText(reinterpret_cast<TOKEN_OWNER*>(owner.data())->Owner).c_str(), sidText(label).c_str(), rid);
    std::printf("DIAGNOSTIC_TOKEN kind=%s restricted=%u restricted_sid_count=%lu\n", kind,
        IsTokenRestricted(token) ? 1U : 0U, reinterpret_cast<TOKEN_GROUPS*>(restricted.data())->GroupCount);
    logDiagnosticTokenScalar(token, kind, TokenIsAppContainer, "appcontainer");
    logDiagnosticTokenScalar(token, kind, TokenHasRestrictions, "has_restrictions");
    logDiagnosticTokenScalar(token, kind, TokenType, "type");
    logDiagnosticTokenScalar(token, kind, TokenSessionId, "session");
    logDiagnosticTokenScalar(token, kind, TokenElevation, "elevated");
    const auto groups = reinterpret_cast<TOKEN_GROUPS*>(groupsData.data());
    DWORD count = 0;
    for (DWORD i = 0; i < groups->GroupCount; ++i) {
        if ((groups->Groups[i].Attributes & SE_GROUP_LOGON_ID) == SE_GROUP_LOGON_ID) {
            ++count;
            std::printf("DIAGNOSTIC_TOKEN kind=%s logon_sid=%ls\n", kind, sidText(groups->Groups[i].Sid).c_str());
        }
    }
    std::printf("DIAGNOSTIC_TOKEN kind=%s logon_groups=%lu\n", kind, count);
}
template<class Policy>
void logOwnMitigation(PROCESS_MITIGATION_POLICY kind, const char* name) {
    Policy policy{};
    const BOOL queried = GetProcessMitigationPolicy(GetCurrentProcess(), kind, &policy, sizeof(policy));
    const DWORD error = queried ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_MITIGATION name=%s available=%u flags=0x%08lx error=%lu\n",
        name, queried ? 1U : 0U, queried ? policy.Flags : 0UL, error);
}
void logOwnExecutionContext(HANDLE token) {
    logOwnToken(token, "process");
    DWORD session = MAXDWORD;
    const BOOL sessionKnown = ProcessIdToSessionId(GetCurrentProcessId(), &session);
    const DWORD sessionError = sessionKnown ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_PROCESS session_available=%u session=%lu error=%lu\n", sessionKnown ? 1U : 0U, session, sessionError);
    Handle threadToken;
    const BOOL impersonating = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, TRUE, &threadToken.value);
    const DWORD threadError = impersonating ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_IMPERSONATION token_opened=%u error=%lu\n", impersonating ? 1U : 0U, threadError);
    if (impersonating) {
        logOwnToken(threadToken.value, "thread");
        logDiagnosticTokenScalar(threadToken.value, "thread", TokenImpersonationLevel, "impersonation_level");
    }
    BOOL inJob = FALSE;
    const BOOL jobKnown = IsProcessInJob(GetCurrentProcess(), nullptr, &inJob);
    const DWORD jobError = jobKnown ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_JOB membership_available=%u member=%u error=%lu\n", jobKnown ? 1U : 0U, inJob ? 1U : 0U, jobError);
    if (jobKnown && inJob) {
        // NULL means this process's associated job, not another process/job.
        JOBOBJECT_BASIC_UI_RESTRICTIONS ui{};
        const BOOL queried = QueryInformationJobObject(nullptr, JobObjectBasicUIRestrictions, &ui, sizeof(ui), nullptr);
        const DWORD error = queried ? ERROR_SUCCESS : GetLastError();
        std::printf("DIAGNOSTIC_JOB ui_available=%u ui_flags=0x%08lx error=%lu scope=own_associated_job\n",
            queried ? 1U : 0U, ui.UIRestrictionsClass, error);
    }
    logOwnMitigation<PROCESS_MITIGATION_SYSTEM_CALL_DISABLE_POLICY>(ProcessSystemCallDisablePolicy, "system_call_disable");
    logOwnMitigation<PROCESS_MITIGATION_CHILD_PROCESS_POLICY>(ProcessChildProcessPolicy, "child_process");
    logOwnMitigation<PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY>(ProcessExtensionPointDisablePolicy, "extension_points");
    logOwnMitigation<PROCESS_MITIGATION_DYNAMIC_CODE_POLICY>(ProcessDynamicCodePolicy, "dynamic_code");
    logOwnMitigation<PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY>(ProcessSignaturePolicy, "binary_signature");
    logOwnMitigation<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(ProcessImageLoadPolicy, "image_load");
}
void diagnoseStationCase(const wchar_t* caseName, SECURITY_ATTRIBUTES* security) {
    constexpr DWORD access = WINSTA_ALL_ACCESS & ~WINSTA_READSCREEN;
    StationReference station;
    SetLastError(ERROR_SUCCESS);
    station.value = CreateWindowStationW(nullptr, CWF_CREATE_ONLY, access, security);
    const DWORD error = GetLastError();
    std::printf("DIAGNOSTIC_CASE name=%ls naming=system_generated requested_access=0x%08lx created=%u immediate_error=%lu\n",
        caseName, access, station.value ? 1U : 0U, error);
    if (!station.value) return; // A recorded failed case is not containment success.
    require(error != ERROR_ALREADY_EXISTS, "diagnostic refuses existing station", ERROR_ALREADY_EXISTS);
    const std::wstring name = objectName(station.value);
    require(!name.empty() && _wcsicmp(name.c_str(), objectName(GetProcessWindowStation()).c_str()) != 0,
        "diagnostic station differs from original", ERROR_INVALID_HANDLE);
    std::printf("DIAGNOSTIC_CASE name=%ls actual_station=%ls\n", caseName, name.c_str());
    USEROBJECTFLAGS flags{}; DWORD needed = 0;
    const BOOL flagsRead = GetUserObjectInformationW(station.value, UOI_FLAGS, &flags, sizeof(flags), &needed);
    const DWORD flagsError = flagsRead ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_CASE name=%ls flags_available=%u flags=0x%08lx error=%lu\n",
        caseName, flagsRead ? 1U : 0U, flags.dwFlags, flagsError);
    require(flagsRead && !(flags.dwFlags & WSF_VISIBLE), "diagnostic station must be non-input", flagsError);
    {
        StationReference view(OpenWindowStationW(name.c_str(), FALSE, READ_CONTROL));
        const DWORD viewError = view.value ? ERROR_SUCCESS : GetLastError();
        if (view.value) logUserObjectSecurity(view.value, "diagnostic_station");
        else std::printf("OBJECT_SECURITY kind=diagnostic_station unavailable_error=%lu\n", viewError);
        require(view.close(), "close diagnostic descriptor view");
    }
    require(station.close(), "close diagnostic station");
    StationReference reopened(OpenWindowStationW(name.c_str(), FALSE, WINSTA_ENUMDESKTOPS));
    const DWORD reopenError = reopened.value ? ERROR_SUCCESS : GetLastError();
    std::printf("DIAGNOSTIC_CASE name=%ls closed=1 reopen_error=%lu\n", caseName, reopenError);
    require(!reopened.value && reopenError == ERROR_FILE_NOT_FOUND, "diagnostic owned station disappeared after close", reopenError);
}
int diagnoseObjectsOnly() {
    // This function cannot reach fixture setup, listener/job creation, desktop
    // selection or any child launch. Descriptor variants are diagnostic-only.
    std::printf("DIAGNOSTIC_ONLY begin children=0 fallback=0 descriptor_variants_not_for_sandbox=1\n");
    const HWINSTA originalStation = GetProcessWindowStation();
    const HDESK originalDesktop = GetThreadDesktop(GetCurrentThreadId());
    require(originalStation && originalDesktop, "diagnostic borrowed user object identity");
    Handle token;
    require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token.value) != FALSE, "diagnostic own token");
    logOwnExecutionContext(token.value);
    auto user = tokenData(token.value, TokenUser);
    const std::wstring userSid = sidText(reinterpret_cast<TOKEN_USER*>(user.data())->User.Sid);
    const wchar_t* cases[] = {L"existing_ga_low", L"default_null", L"explicit_ga_no_sacl",
        L"explicit_ga_low_repeat", L"numeric_full_low"};
    for (unsigned i = 0; i < ARRAYSIZE(cases); ++i) {
        const std::wstring id = nonce();
        DerivedSid sid(L"Locust.UniSDiagnostic." + id);
        const std::wstring appSid = sidText(sid.value);
        std::printf("DIAGNOSTIC_CASE name=%ls nonce=%ls derived_sid=%ls profile_created=0\n", cases[i], id.c_str(), appSid.c_str());
        if (i == 1) diagnoseStationCase(cases[i], nullptr);
        else {
            // Numeric full rights include standard rights, unlike the 0x017F
            // creation request. This comparison does not change the strict SD.
            const std::wstring rights = i == 4 ? L"0x000f037f" : L"GA";
            const std::wstring label = i == 2 ? L"" : L"S:(ML;;NW;;;LW)";
            Descriptor descriptor(L"D:P(A;;" + rights + L";;;" + userSid + L")(A;;" + rights + L";;;" + appSid + L")" + label);
            auto security = descriptor.attributes();
            diagnoseStationCase(cases[i], &security);
        }
        require(GetProcessWindowStation() == originalStation && GetThreadDesktop(GetCurrentThreadId()) == originalDesktop,
            "diagnostic never changed station/desktop selection", ERROR_INVALID_HANDLE);
    }
    require(cleanupOk, "diagnostic reference cleanup", ERROR_INVALID_HANDLE);
    std::printf("DIAGNOSTIC_ONLY complete cases=5 children=0 containment_verdict=none\n");
    return 20; // Deliberately distinct from strict proof success (0).
}
void verifyPrivateObjectSecurity(HANDLE object, PSID user, PSID app, DWORD requiredRights, const char* kind) {
    SECURITY_INFORMATION information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;
    DWORD needed = 0;
    const BOOL sized = GetUserObjectSecurity(object, &information, nullptr, 0, &needed);
    const DWORD sizeError = GetLastError();
    require(!sized && sizeError == ERROR_INSUFFICIENT_BUFFER && needed && needed <= 65536,
        "strict private object security size", sizeError);
    std::vector<BYTE> descriptor(needed);
    require(GetUserObjectSecurity(object, &information, descriptor.data(), needed, &needed) != FALSE,
        "strict private object security readback");
    PSID owner = nullptr; BOOL defaulted = FALSE, present = FALSE;
    PACL dacl = nullptr, sacl = nullptr;
    SECURITY_DESCRIPTOR_CONTROL control{}; DWORD revision = 0;
    require(GetSecurityDescriptorControl(descriptor.data(), &control, &revision) && (control & SE_DACL_PROTECTED) &&
        GetSecurityDescriptorOwner(descriptor.data(), &owner, &defaulted) && owner && EqualSid(owner, user),
        "strict private object protected DACL and owner", ERROR_INVALID_SECURITY_DESCR);
    require(GetSecurityDescriptorDacl(descriptor.data(), &present, &dacl, &defaulted) && present && dacl &&
        dacl->AceCount == 2, "strict private object exactly two allow ACEs", ERROR_INVALID_ACL);
    unsigned userAces = 0, appAces = 0;
    for (DWORD i = 0; i < dacl->AceCount; ++i) {
        void* raw = nullptr;
        require(GetAce(dacl, i, &raw) != FALSE, "strict private object ACE");
        const auto ace = static_cast<ACCESS_ALLOWED_ACE*>(raw);
        require(ace->Header.AceType == ACCESS_ALLOWED_ACE_TYPE && ace->Header.AceFlags == 0 &&
            ((ace->Mask & GENERIC_ALL) || (ace->Mask & requiredRights) == requiredRights),
            "strict private object allow rights", ERROR_INVALID_ACL);
        PSID allowedSid = const_cast<DWORD*>(&ace->SidStart);
        if (EqualSid(allowedSid, user)) ++userAces;
        else if (EqualSid(allowedSid, app)) ++appAces;
        else throw Failure{"strict private object unexpected principal", ERROR_INVALID_ACL};
    }
    require(userAces == 1 && appAces == 1, "strict private object user/AppContainer principals", ERROR_INVALID_ACL);
    require(GetSecurityDescriptorSacl(descriptor.data(), &present, &sacl, &defaulted) && present && sacl && sacl->AceCount == 1,
        "strict private object mandatory label present", ERROR_INVALID_ACL);
    void* raw = nullptr;
    require(GetAce(sacl, 0, &raw) != FALSE, "strict private object label ACE");
    const auto label = static_cast<SYSTEM_MANDATORY_LABEL_ACE*>(raw);
    BYTE low[SECURITY_MAX_SID_SIZE]{}; DWORD lowSize = sizeof(low);
    require(CreateWellKnownSid(WinLowLabelSid, nullptr, low, &lowSize) != FALSE, "low mandatory SID");
    require(label->Header.AceType == SYSTEM_MANDATORY_LABEL_ACE_TYPE && label->Header.AceFlags == 0 &&
        label->Mask == SYSTEM_MANDATORY_LABEL_NO_WRITE_UP && EqualSid(const_cast<DWORD*>(&label->SidStart), low),
        "strict private object exact low no-write-up label", ERROR_INVALID_SECURITY_DESCR);
    std::printf("OBJECT_SECURITY_VERIFIED kind=%s owner_matches=1 protected_dacl=1 user_app_only=1 low_label=1 no_write_up=1\n", kind);
}
struct PrivateObjects {
    HWINSTA original = GetProcessWindowStation(); // Borrowed; never close or change its ACL.
    HWINSTA station = nullptr;
    HDESK desktop = nullptr;
    std::wstring name;
    bool boundName = false;
    bool switched = false;
    PrivateObjects() = default;
    PrivateObjects(const PrivateObjects&) = delete;
    void create(SECURITY_ATTRIBUTES* security, PSID user, PSID app) {
        require(original != nullptr, "original windowstation handle");
        // A non-input station has no screen to read; request no unused READSCREEN right.
        constexpr DWORD requestedAccess = WINSTA_ALL_ACCESS & ~WINSTA_READSCREEN;
        static_assert(requestedAccess == 0x017F, "controlled non-input station access mask");
        SetLastError(ERROR_SUCCESS);
        // Supplying a name requires administrator membership. NULL asks Windows
        // for the logon-derived name; CREATE_ONLY still refuses any collision.
        station = CreateWindowStationW(nullptr, CWF_CREATE_ONLY, requestedAccess, security);
        const DWORD createError = GetLastError(); // Capture before diagnostics or another API call.
        std::printf("WINDOWSTATION naming=system_generated requested_access=0x%08lx created=%u immediate_error=%lu\n",
            requestedAccess, station ? 1U : 0U, createError);
        require(station != nullptr, "create unique private windowstation", createError);
        if (createError == ERROR_ALREADY_EXISTS) {
            // Release only the acquired reference; never inspect/reuse an existing object.
            if (!CloseWindowStation(station)) cleanupOk = false;
            station = nullptr;
            throw Failure{"refuse existing windowstation", createError};
        }
        name = objectName(station);
        require(!name.empty() && station != original &&
            _wcsicmp(name.c_str(), objectName(original).c_str()) != 0,
            "new private station differs from original", ERROR_INVALID_HANDLE);
        boundName = true;
        std::printf("WINDOWSTATION actual_name=%ls original_distinct=1\n", name.c_str());
        // The creation mask lacks READ_CONTROL; this separate handle is bound
        // to the actual OS-returned name while the new object remains held.
        {
            StationReference securityView(OpenWindowStationW(name.c_str(), FALSE, READ_CONTROL));
            require(securityView.value != nullptr, "open new station security view");
            logUserObjectSecurity(securityView.value, "station");
            verifyPrivateObjectSecurity(securityView.value, user, app, WINSTA_CREATEDESKTOP | WINSTA_ENUMDESKTOPS, "station");
            require(securityView.close(), "close owned station security view");
        }
        USEROBJECTFLAGS flags{}; DWORD needed = 0;
        require(GetUserObjectInformationW(station, UOI_FLAGS, &flags, sizeof(flags), &needed) != FALSE &&
            !(flags.dwFlags & WSF_VISIBLE), "private station must be non-input");
        require(SetProcessWindowStation(station) != FALSE, "select owned station for desktop creation");
        switched = true;
        desktop = CreateDesktopW(L"NonInput", nullptr, nullptr, 0,
            DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | READ_CONTROL, security);
        require(desktop != nullptr, "create private non-input desktop");
        logUserObjectSecurity(desktop, "desktop");
        verifyPrivateObjectSecurity(desktop, user, app, DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS, "desktop");
        require(SetProcessWindowStation(original) != FALSE, "restore borrowed process windowstation");
        switched = false;
        std::printf("PRIVATE_OBJECTS created=1 station_visible=0 input_switches=0 original_acl_changes=0\n");
    }
    bool close() {
        bool ok = true;
        if (switched) {
            if (!SetProcessWindowStation(original)) ok = false;
            else switched = false;
        }
        if (desktop) {
            if (!CloseDesktop(desktop)) ok = false;
            desktop = nullptr;
            if (station && SetProcessWindowStation(station)) {
                HDESK probe = OpenDesktopW(L"NonInput", 0, FALSE, DESKTOP_READOBJECTS);
                const DWORD error = probe ? ERROR_SUCCESS : GetLastError();
                if (probe) CloseDesktop(probe);
                if (error != ERROR_FILE_NOT_FOUND) ok = false;
                if (!SetProcessWindowStation(original)) { ok = false; switched = true; }
                std::printf("CLEANUP desktop_reopen_error=%lu\n", error);
            } else ok = false;
        }
        if (station) {
            if (!CloseWindowStation(station)) ok = false;
            station = nullptr;
            if (boundName) {
                HWINSTA probe = OpenWindowStationW(name.c_str(), FALSE, WINSTA_ENUMDESKTOPS);
                const DWORD error = probe ? ERROR_SUCCESS : GetLastError();
                if (probe && !CloseWindowStation(probe)) ok = false;
                if (error != ERROR_FILE_NOT_FOUND) ok = false;
                std::printf("CLEANUP station_reopen_error=%lu original_station_restored=%u\n", error,
                    GetProcessWindowStation() == original ? 1U : 0U);
            } else {
                // Never pass an empty/unverified name to OpenWindowStation.
                std::printf("CLEANUP station_reference_closed=1 disappearance_unverified=1\n");
                ok = false;
            }
        }
        if (GetProcessWindowStation() != original) ok = false;
        if (!ok) cleanupOk = false;
        return ok;
    }
    ~PrivateObjects() { close(); }
};
struct Attributes {
    std::vector<BYTE> storage;
    LPPROC_THREAD_ATTRIBUTE_LIST value = nullptr;
    explicit Attributes(DWORD count) {
        SIZE_T size = 0;
        InitializeProcThreadAttributeList(nullptr, count, 0, &size);
        require(size > 0 && size <= 65536, "process attributes size");
        storage.resize(size);
        auto candidate = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
        require(InitializeProcThreadAttributeList(candidate, count, 0, &size) != FALSE, "process attributes initialize");
        value = candidate;
    }
    void set(DWORD_PTR attribute, void* data, SIZE_T size) {
        require(UpdateProcThreadAttribute(value, 0, attribute, data, size, nullptr, nullptr) != FALSE,
            "set required process attribute");
    }
    ~Attributes() { if (value) DeleteProcThreadAttributeList(value); } // Releases memory, not a file/profile.
};
struct ChildLifetime {
    Handle job, process, thread;
    bool assigned = false;
    bool closed = false;
    void createJob() {
        job.value = CreateJobObjectW(nullptr, nullptr);
        require(job.value != nullptr, "create owned job");
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE |
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_PROCESS_TIME |
            JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.BasicLimitInformation.PerProcessUserTimeLimit.QuadPart = 2 * 10000000LL;
        limits.ProcessMemoryLimit = 128 * 1024 * 1024;
        require(SetInformationJobObject(job.value, JobObjectExtendedLimitInformation, &limits, sizeof(limits)) != FALSE,
            "job memory/time/active-one/kill-close limits");
        JOBOBJECT_BASIC_UI_RESTRICTIONS ui{};
        ui.UIRestrictionsClass = JOB_OBJECT_UILIMIT_ALL;
        require(SetInformationJobObject(job.value, JobObjectBasicUIRestrictions, &ui, sizeof(ui)) != FALSE,
            "job UI restrictions");
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION actual{};
        require(QueryInformationJobObject(job.value, JobObjectExtendedLimitInformation, &actual, sizeof(actual), nullptr) != FALSE &&
            actual.BasicLimitInformation.LimitFlags == limits.BasicLimitInformation.LimitFlags &&
            actual.BasicLimitInformation.ActiveProcessLimit == 1 && actual.ProcessMemoryLimit == limits.ProcessMemoryLimit &&
            actual.BasicLimitInformation.PerProcessUserTimeLimit.QuadPart == limits.BasicLimitInformation.PerProcessUserTimeLimit.QuadPart,
            "verify actual job limits");
        std::printf("JOB active_limit=1 breakaway=0 kill_on_close=1 memory_bytes=134217728 cpu_seconds=2 wall_ms=5000\n");
    }
    bool close() {
        if (closed) return true;
        closed = true;
        bool ok = true;
        if (process.value) {
            if (WaitForSingleObject(process.value, 0) != WAIT_OBJECT_0) {
                const BOOL stopped = assigned && job.value ? TerminateJobObject(job.value, 96) : TerminateProcess(process.value, 96);
                if (!stopped || WaitForSingleObject(process.value, 3000) != WAIT_OBJECT_0) ok = false;
            }
            if (WaitForSingleObject(process.value, 0) != WAIT_OBJECT_0) ok = false;
        }
        if (job.value) {
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION accounting{};
            // Job accounting can trail the process signal briefly; a bounded poll
            // verifies actual zero active processes before releasing the job.
            bool empty = false;
            for (unsigned i = 0; i < 100; ++i) {
                if (!QueryInformationJobObject(job.value, JobObjectBasicAccountingInformation, &accounting,
                    sizeof(accounting), nullptr)) break;
                if (!accounting.ActiveProcesses) { empty = true; break; }
                Sleep(10);
            }
            if (!empty) ok = false;
            std::printf("CLEANUP job_active=%lu\n", accounting.ActiveProcesses);
        }
        if (!thread.close()) ok = false;
        if (!process.close()) ok = false;
        if (!job.close()) ok = false;
        std::printf("CLEANUP child_and_job_handles=%s\n", ok ? "closed" : "failed");
        if (!ok) cleanupOk = false;
        return ok;
    }
    ~ChildLifetime() { close(); }
};
void verifyToken(HANDLE process, PSID expectedSid, PSID originalUser) {
    Handle token;
    require(OpenProcessToken(process, TOKEN_QUERY, &token.value) != FALSE, "open suspended child token");
    const DWORD app = tokenDword(token.value, TokenIsAppContainer);
    auto sid = tokenData(token.value, TokenAppContainerSid);
    auto caps = tokenData(token.value, TokenCapabilities);
    auto user = tokenData(token.value, TokenUser);
    auto label = tokenData(token.value, TokenIntegrityLevel);
    auto privileges = tokenData(token.value, TokenPrivileges);
    const PSID appSid = reinterpret_cast<TOKEN_APPCONTAINER_INFORMATION*>(sid.data())->TokenAppContainer;
    const PSID il = reinterpret_cast<TOKEN_MANDATORY_LABEL*>(label.data())->Label.Sid;
    const DWORD integrity = *GetSidSubAuthority(il, *GetSidSubAuthorityCount(il) - 1);
    const DWORD capCount = reinterpret_cast<TOKEN_GROUPS*>(caps.data())->GroupCount;
    const DWORD elevated = tokenDword(token.value, TokenElevation);
    LUID traverse{};
    require(LookupPrivilegeValueW(nullptr, SE_CHANGE_NOTIFY_NAME, &traverse) != FALSE, "traverse privilege ID");
    DWORD enabled = 0;
    auto rights = reinterpret_cast<TOKEN_PRIVILEGES*>(privileges.data());
    for (DWORD i = 0; i < rights->PrivilegeCount; ++i) {
        const auto& right = rights->Privileges[i];
        if (right.Attributes & SE_PRIVILEGE_ENABLED) {
            ++enabled;
            require(right.Luid.LowPart == traverse.LowPart && right.Luid.HighPart == traverse.HighPart,
                "child enabled privilege allowlist", ERROR_PRIVILEGE_NOT_HELD);
        }
    }
    const bool sidMatches = appSid && EqualSid(appSid, expectedSid);
    std::printf("TOKEN appcontainer=%lu sid_matches=%u capabilities=%lu integrity=%lu elevated=%lu enabled_privileges=%lu\n",
        app, sidMatches ? 1U : 0U, capCount, integrity, elevated, enabled);
    require(app == 1 && sidMatches && capCount == 0 && integrity == SECURITY_MANDATORY_LOW_RID && elevated == 0 &&
        EqualSid(reinterpret_cast<TOKEN_USER*>(user.data())->User.Sid, originalUser), "exact suspended AppContainer token", ERROR_ACCESS_DENIED);
    PROCESS_MITIGATION_CHILD_PROCESS_POLICY policy{};
    require(GetProcessMitigationPolicy(process, ProcessChildProcessPolicy, &policy, sizeof(policy)) != FALSE &&
        policy.NoChildProcessCreation && !policy.AllowSecureProcessCreation, "actual restricted child policy");
    std::printf("TOKEN child_process_restricted=1 allow_secure_child=0\n");
}
struct Listener {
    Winsock winsock;
    Socket socketHandle;
    unsigned short port = 0;
    Listener() {
        socketHandle.value = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        require(socketHandle.value != INVALID_SOCKET, "host loopback socket", static_cast<DWORD>(WSAGetLastError()));
        BOOL exclusive = TRUE;
        require(setsockopt(socketHandle.value, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, reinterpret_cast<char*>(&exclusive), sizeof(exclusive)) == 0,
            "exclusive loopback listener");
        sockaddr_in address{}; address.sin_family = AF_INET;
        address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        require(bind(socketHandle.value, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0 &&
            listen(socketHandle.value, 1) == 0, "bind/listen own loopback", static_cast<DWORD>(WSAGetLastError()));
        int size = sizeof(address);
        require(getsockname(socketHandle.value, reinterpret_cast<sockaddr*>(&address), &size) == 0, "listener own address");
        port = ntohs(address.sin_port);
        u_long nonblocking = 1;
        require(ioctlsocket(socketHandle.value, FIONBIO, &nonblocking) == 0, "nonblocking listener");
        Socket control;
        control.value = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        require(control.value != INVALID_SOCKET && ioctlsocket(control.value, FIONBIO, &nonblocking) == 0,
            "host loopback positive control socket");
        const int result = connect(control.value, reinterpret_cast<sockaddr*>(&address), sizeof(address));
        require(result == 0 || WSAGetLastError() == WSAEWOULDBLOCK, "host loopback positive control connect");
        fd_set readable{}; FD_ZERO(&readable); FD_SET(socketHandle.value, &readable);
        timeval timeout{1, 0};
        require(select(0, &readable, nullptr, nullptr, &timeout) == 1, "host loopback positive control deadline");
        Socket accepted;
        accepted.value = accept(socketHandle.value, nullptr, nullptr);
        require(accepted.value != INVALID_SOCKET, "host loopback positive control accept");
        std::printf("NETWORK positive_control_accepted=1 destination=own_ipv4_loopback\n");
    }
    void assertNoChildConnection() {
        Socket accepted;
        accepted.value = accept(socketHandle.value, nullptr, nullptr);
        const int error = accepted.value == INVALID_SOCKET ? WSAGetLastError() : 0;
        std::printf("NETWORK child_connections_accepted=%u listener_error=%d\n", accepted.value == INVALID_SOCKET ? 0U : 1U, error);
        require(accepted.value == INVALID_SOCKET && error == WSAEWOULDBLOCK, "no child loopback connection accepted", static_cast<DWORD>(error));
    }
};
std::vector<wchar_t> childEnvironment(const std::wstring& allowed) {
    wchar_t windows[MAX_PATH]{};
    require(GetWindowsDirectoryW(windows, ARRAYSIZE(windows)) > 0, "OS directory");
    std::vector<wchar_t> result;
    for (const auto& item : {std::wstring(L"SystemRoot=") + windows, L"TEMP=" + allowed, L"TMP=" + allowed}) {
        result.insert(result.end(), item.begin(), item.end()); result.push_back(L'\0');
    }
    result.push_back(L'\0');
    return result;
}
int host(int argc, wchar_t** argv) {
    require(argc == 2, "host requires a fixed test mode", ERROR_BAD_ARGUMENTS);
    const std::wstring mode(argv[1]);
    const bool baseline = mode == L"--baseline";
    const bool timeoutTest = mode == L"--timeout";
    const bool killCloseTest = mode == L"--kill-close";
    const bool constructorTest = mode == L"--fail-objects";
    const bool diagnosticOnly = mode == L"--diagnose-objects-only";
    require(baseline || mode == L"--verify" || timeoutTest || killCloseTest || constructorTest || diagnosticOnly,
        "host fixed test mode", ERROR_BAD_ARGUMENTS);
    const auto buildRoot = parent(modulePath());
    require(buildRoot == LOCUST_OUTPUT_ROOT, "host bound to its compiled owned output directory", ERROR_INVALID_NAME);
    realDirectoryAncestors(buildRoot);
    if (diagnosticOnly) return diagnoseObjectsOnly(); // No fallthrough to launch-capable code.
    Handle originalToken;
    require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &originalToken.value) != FALSE, "original host token");
    require(tokenDword(originalToken.value, TokenIsAppContainer) == 0, "host must be outside AppContainer", ERROR_ACCESS_DENIED);
    auto originalUserData = tokenData(originalToken.value, TokenUser);
    const PSID originalUser = reinterpret_cast<TOKEN_USER*>(originalUserData.data())->User.Sid;
    const auto userText = sidText(originalUser);
    std::printf("ORIGINAL user_sid=%ls elevated=%lu\n", userText.c_str(), tokenDword(originalToken.value, TokenElevation));
    auto groupsData = tokenData(originalToken.value, TokenGroups);
    auto groups = reinterpret_cast<TOKEN_GROUPS*>(groupsData.data());
    bool logonFound = false;
    for (DWORD i = 0; i < groups->GroupCount; ++i) {
        if ((groups->Groups[i].Attributes & SE_GROUP_LOGON_ID) == SE_GROUP_LOGON_ID) {
            std::printf("ORIGINAL logon_sid=%ls\n", sidText(groups->Groups[i].Sid).c_str()); logonFound = true;
        }
    }
    require(logonFound, "original logon SID recorded", ERROR_NO_SUCH_LOGON_SESSION);
    const auto id = nonce();
    DerivedSid sid(L"Locust.UniS." + id);
    const auto appText = sidText(sid.value);
    std::printf("PROFILELESS nonce=%ls appcontainer_sid=%ls profile_created=0 registry_writes=0 file_deletions=0\n", id.c_str(), appText.c_str());
    Descriptor hostOnly(L"D:P(A;OICI;FA;;;" + userText + L")");
    Descriptor allowedFiles(L"D:P(A;OICI;FA;;;" + userText + L")(A;OICI;FA;;;" + appText + L")S:(ML;OICI;NW;;;LW)");
    Descriptor userObjects(L"D:P(A;;GA;;;" + userText + L")(A;;GA;;;" + appText + L")S:(ML;;NW;;;LW)");
    auto hostSecurity = hostOnly.attributes(), fileSecurity = allowedFiles.attributes(), objectSecurity = userObjects.attributes();
    const auto root = buildRoot + L"\\case-" + id;
    const auto allowed = root + L"\\allowed";
    require(CreateDirectoryW(root.c_str(), &hostSecurity) != FALSE, "new case directory");
    require(CreateDirectoryW(allowed.c_str(), &fileSecurity) != FALSE, "new allowed fixture directory");
    const auto source = buildRoot + L"\\fixture.exe", payload = allowed + L"\\fixture.exe";
    const auto payloadBytes = readExact(source, 4 * 1024 * 1024);
    require(hashBytes(payloadBytes) == LOCUST_FIXTURE_SHA, "fixed compiled fixture hash", ERROR_CRC);
    writeExact(payload, payloadBytes.data(), static_cast<DWORD>(payloadBytes.size()), CREATE_NEW, &fileSecurity);
    Handle payloadLock(CreateFileW(payload.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
    require(payloadLock.value != INVALID_HANDLE_VALUE && hashBytes(readExact(payload, 4 * 1024 * 1024)) == LOCUST_FIXTURE_SHA,
        "copied fixture pinned and write-locked", ERROR_CRC);
    std::printf("PAYLOAD sha256=%s copy_verified=1 write_locked=1\n", LOCUST_FIXTURE_SHA);
    writeExact(root + L"\\outside.sentinel", pristine, sizeof(pristine), CREATE_NEW, &hostSecurity);
    Listener listener;
    if (constructorTest) {
        try {
            PrivateObjects partialObjects;
            partialObjects.create(&objectSecurity, originalUser, sid.value);
            ChildLifetime partialChild;
            partialChild.createJob();
            throw Failure{"injected setup failure after owned objects", ERROR_CANCELLED};
        } catch (const Failure& failure) {
            if (failure.code != ERROR_CANCELLED ||
                std::strcmp(failure.operation, "injected setup failure after owned objects") != 0) throw;
        }
        require(cleanupOk, "partial construction unwinding verified", ERROR_INVALID_HANDLE);
        std::printf("PASS injected_constructor_failure owned_objects_released=1 files_retained=1\n");
        return 0;
    }
    PrivateObjects objects;
    // The negative control also uses private non-input objects and a bounded job;
    // it omits only AppContainer and the child restriction, never runs on WinSta0.
    objects.create(&objectSecurity, originalUser, sid.value);
    ChildLifetime child; child.createJob();
    SECURITY_ATTRIBUTES inheritable{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    Handle inheritanceProbe(CreateEventW(&inheritable, TRUE, FALSE, nullptr));
    require(inheritanceProbe.value != nullptr, "owned inheritable handle probe");
    SECURITY_CAPABILITIES capabilities{}; capabilities.AppContainerSid = sid.value;
    DWORD policy = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    Attributes attributes(baseline ? 1U : 2U);
    // Baseline has an innocuous mitigation attribute to use the same startup path.
    DWORD64 mitigation = PROCESS_CREATION_MITIGATION_POLICY_DEP_ENABLE;
    if (baseline) attributes.set(PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, &mitigation, sizeof(mitigation));
    else {
        attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &capabilities, sizeof(capabilities));
        attributes.set(PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, &policy, sizeof(policy));
    }
    STARTUPINFOEXW startup{}; startup.StartupInfo.cb = sizeof(startup); startup.lpAttributeList = attributes.value;
    auto desktopPath = objects.name + L"\\NonInput";
    startup.StartupInfo.lpDesktop = desktopPath.data();
    auto command = quote(payload) + (baseline ? L" --baseline " : (timeoutTest || killCloseTest ? L" --hang " : L" --probe ")) +
        id + L" " + appText + L" " + std::to_wstring(listener.port) + L" " +
        std::to_wstring(reinterpret_cast<ULONG_PTR>(inheritanceProbe.value)) + L" owned-v1 " + quote(objects.name);
    auto environment = childEnvironment(allowed);
    PROCESS_INFORMATION created{};
    const BOOL launched = CreateProcessW(payload.c_str(), command.data(), nullptr, nullptr, FALSE,
        EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
        environment.data(), allowed.c_str(), &startup.StartupInfo, &created);
    const DWORD launchError = launched ? ERROR_SUCCESS : GetLastError();
    std::printf("LAUNCH suspended=1 inherit_handles=0 appcontainer_requested=%u error=%lu\n", baseline ? 0U : 1U, launchError);
    require(launched != FALSE, "profileless/private-desktop CreateProcess", launchError);
    child.process.value = created.hProcess; child.thread.value = created.hThread;
    require(AssignProcessToJobObject(child.job.value, child.process.value) != FALSE, "assign suspended fixture to job");
    child.assigned = true;
    BOOL inJob = FALSE;
    require(IsProcessInJob(child.process.value, child.job.value, &inJob) != FALSE && inJob, "verify job before resume");
    if (!baseline) verifyToken(child.process.value, sid.value, originalUser);
    require(ResumeThread(child.thread.value) == 1, "resume only after job/token checks");
    if (timeoutTest || killCloseTest) {
        bool ready = false;
        for (unsigned i = 0; i < 200; ++i) {
            if (GetFileAttributesW((allowed + L"\\hang-ready.bin").c_str()) != INVALID_FILE_ATTRIBUTES) {
                try {
                    const auto marker = readExact(allowed + L"\\hang-ready.bin", sizeof(reportMagic));
                    ready = marker.size() == sizeof(reportMagic) && std::memcmp(marker.data(), &reportMagic, sizeof(reportMagic)) == 0;
                    break;
                } catch (const Failure& failure) {
                    if (failure.code != ERROR_SHARING_VIOLATION) throw;
                    // Creation can become visible just before the fixture closes
                    // its exclusive writer. Retry only that bounded race.
                }
            }
            if (WaitForSingleObject(child.process.value, 10) == WAIT_OBJECT_0) break;
        }
        require(ready, "owned hang fixture reached its readiness marker", WAIT_TIMEOUT);
    }
    DWORD wait = WAIT_FAILED;
    if (killCloseTest) {
        require(child.job.close(), "close last owned job handle");
        wait = WaitForSingleObject(child.process.value, 3000);
        require(wait == WAIT_OBJECT_0, "kill-on-close terminated owned fixture", WAIT_TIMEOUT);
    } else {
        wait = WaitForSingleObject(child.process.value, timeoutTest ? 200 : 5000);
        if (timeoutTest || wait != WAIT_OBJECT_0) {
            require(TerminateJobObject(child.job.value, 97) != FALSE && WaitForSingleObject(child.process.value, 3000) == WAIT_OBJECT_0,
                "deadline terminated owned job");
        }
        require(timeoutTest ? wait == WAIT_TIMEOUT : wait == WAIT_OBJECT_0, "expected fixture wait result", wait);
    }
    DWORD exitCode = MAXDWORD;
    require(GetExitCodeProcess(child.process.value, &exitCode) != FALSE, "owned fixture exit code");
    std::printf("CHILD exit=%lu exit_hex=0x%08lx initial_wait=%lu\n", exitCode, exitCode, wait);
    listener.assertNoChildConnection();
    const auto sentinel = readExact(root + L"\\outside.sentinel", 128);
    const char* expected = baseline ? changed : pristine;
    const bool sentinelMatches = sentinel.size() == sizeof(pristine) && std::memcmp(sentinel.data(), expected, sizeof(pristine)) == 0;
    std::printf("FILE outside_sentinel_expected=%s matches=%u payload_unchanged=%u\n", baseline ? "changed" : "pristine",
        sentinelMatches ? 1U : 0U, hashBytes(readExact(payload, 4 * 1024 * 1024)) == LOCUST_FIXTURE_SHA ? 1U : 0U);
    require(sentinelMatches && hashBytes(readExact(payload, 4 * 1024 * 1024)) == LOCUST_FIXTURE_SHA,
        "sentinel/payload independent host comparison", ERROR_CRC);
    if (!timeoutTest && !killCloseTest) {
        require(exitCode == 0, "owned fixture completed its probes", exitCode);
        const auto bytes = readExact(allowed + L"\\report.bin", sizeof(Report));
        require(bytes.size() == sizeof(Report), "fixture report exact length", ERROR_INVALID_DATA);
        Report report{}; std::memcpy(&report, bytes.data(), sizeof(report));
        require(report.magic == reportMagic && report.size == sizeof(Report), "fixture report identity", ERROR_INVALID_DATA);
        const auto positive = readExact(allowed + L"\\positive.txt", sizeof(pristine));
        require(positive.size() == sizeof(pristine) && std::memcmp(positive.data(), pristine, sizeof(pristine)) == 0 && report.allowedWrite == 1,
            "independent allowed-folder write bytes", ERROR_CRC);
        std::printf("PROBES outside_error=%lu registry_write_handle_error=%lu network_error=%lu spawn_error=%lu allowed_write=%lu inherited_handle_absent=%lu private_objects=%lu\n",
            report.outsideError, report.registryError, report.networkError, report.spawnError, report.allowedWrite,
            report.inheritedHandleAbsent, report.privateObjects);
        if (baseline) require(report.appContainer == 0 && report.outsideError == 0, "baseline uncontained write observed", ERROR_INVALID_DATA);
        else require(report.appContainer == 1 && report.sidMatches == 1 && report.capabilities == 0 &&
            report.integrity == SECURITY_MANDATORY_LOW_RID && report.childPolicy == 1 &&
            report.outsideError == ERROR_ACCESS_DENIED && report.registryError == ERROR_ACCESS_DENIED &&
            report.networkError == WSAEACCES && report.spawnError == ERROR_ACCESS_DENIED &&
            report.inheritedHandleAbsent == 1 && report.privateObjects == 1, "all actual containment probes", ERROR_ACCESS_DENIED);
    } else require(exitCode != 0, "terminated fixture cannot claim normal completion", exitCode);
    require(child.close() && objects.close(), "verify owned kernel object release");
    if (baseline) {
        std::printf("RED observed: containment assertion failed; appcontainer=0 outside_write=allowed owned_sentinel=changed allowed_write=1\n");
        return 23;
    }
    std::printf("PASS %ls profileless=1 files_retained=1 registry_writes=0 filesystem_deletions=0\n", mode.c_str());
    return 0;
}
#endif
} // namespace
int wmain(int argc, wchar_t** argv) {
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    try {
#ifdef LOCUST_FIXTURE
        return fixture(argc, argv);
#else
        const int result = host(argc, argv);
        if (!cleanupOk) { std::printf("FAIL handle_cleanup error=%lu\n", GetLastError()); return 1; }
        return result;
#endif
    } catch (const Failure& failure) {
        std::printf("FAIL operation=%s error=%lu cleanup=%s\n", failure.operation, failure.code, cleanupOk ? "ok" : "failed");
        return 1;
    } catch (const std::exception&) {
        std::printf("FAIL C++ exception cleanup=%s\n", cleanupOk ? "ok" : "failed");
        return 1;
    }
}

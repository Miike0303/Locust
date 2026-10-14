#pragma once
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <bcrypt.h>
#include <array>
#include <cstdint>
#include <cstring>
#include <vector>

#if !defined(_M_IX86)
#error This isolated proof supports only MSVC x86.
#endif
static_assert(sizeof(void*) == 4 && sizeof(wchar_t) == 2, "PE32 / UTF-16 ABI required");

namespace uni {
constexpr DWORD Version = 1;
constexpr size_t HeaderSize = 64;
constexpr size_t Capacity = 3422;
constexpr int MaxText = 65536;
constexpr unsigned char Magic[8] = {'L', 'O', 'C', 'U', 'N', 'I', '1', 0};
using Generation = std::array<unsigned char, 16>;
using Digest = std::array<unsigned char, 32>;
using Initialize = BOOL (__cdecl*)(const wchar_t*, const Generation*, const Digest*, DWORD);
using Uninstall = BOOL (__cdecl*)(DWORD);
using ProbeImage = BOOL (__cdecl*)(const void*, DWORD);

inline uint16_t u16(const unsigned char* p) {
    return static_cast<uint16_t>(p[0] | (static_cast<unsigned>(p[1]) << 8));
}
inline uint32_t u32(const unsigned char* p) {
    return p[0] | (static_cast<uint32_t>(p[1]) << 8) |
        (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}
inline bool candidate(unsigned char lead, unsigned char trail) {
    return ((lead >= 0x81 && lead <= 0x9f) || (lead >= 0xe0 && lead <= 0xfb)) &&
        trail >= 1 && trail <= 0x3f && trail != 9 && trail != 10 &&
        trail != 13 && trail != 0x20 && trail != 0x2c;
}
inline bool sha256(const unsigned char* bytes, size_t size, Digest& out) {
    if (size > 0xffffffffu) return false;
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    if (BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) < 0)
        return false;
    const NTSTATUS result = BCryptHash(algorithm, nullptr, 0,
        const_cast<PUCHAR>(bytes), static_cast<ULONG>(size), out.data(),
        static_cast<ULONG>(out.size()));
    BCryptCloseAlgorithmProvider(algorithm, 0);
    return result >= 0;
}
// Hash the complete header except its digest field, followed by every entry.
inline bool mapDigest(const std::vector<unsigned char>& bytes, Digest& digest) {
    if (bytes.size() < HeaderSize) return false;
    std::vector<unsigned char> content(bytes.begin(), bytes.begin() + 32);
    content.insert(content.end(), bytes.begin() + HeaderSize, bytes.end());
    return sha256(content.data(), content.size(), digest);
}
} // namespace uni

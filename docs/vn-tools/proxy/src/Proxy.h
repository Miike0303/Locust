#pragma once

// winmm-proxy variant: exports are forwarded via exports.def to winmm_orig.dll,
// so no Original* pointers are needed. Init() is a no-op kept for main.cpp.
class Proxy
{
public:
	static void Init();
};

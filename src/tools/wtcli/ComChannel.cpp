#include "ComChannel.h"

#include <combaseapi.h>
#include <fmt/format.h>

// Helper: convert wstring to BSTR for [in] params. Returns null for empty.
static BSTR ToBstr(const std::wstring& s)
{
    return s.empty() ? nullptr : SysAllocString(s.c_str());
}

// RAII wrapper to free a BSTR on scope exit.
struct BstrGuard
{
    BSTR bstr;
    BstrGuard(BSTR b) : bstr(b) {}
    ~BstrGuard() { SysFreeString(bstr); }
    operator BSTR() const { return bstr; }
};

std::unique_ptr<ComChannel> ComChannel::Connect(const std::wstring& clsidStr, const std::wstring& token)
{
    CLSID clsid{};
    HRESULT hr = CLSIDFromString(clsidStr.c_str(), &clsid);
    if (FAILED(hr))
    {
        fprintf(stderr, "[wtcli] Invalid CLSID: %ls (0x%08lX)\n", clsidStr.c_str(), hr);
        return nullptr;
    }

    Microsoft::WRL::ComPtr<ITerminalProtocolServer> server;
    hr = CoCreateInstance(clsid, nullptr, CLSCTX_LOCAL_SERVER, IID_PPV_ARGS(&server));
    if (FAILED(hr))
    {
        fprintf(stderr, "[wtcli] CoCreateInstance failed: 0x%08lX\n", hr);
        return nullptr;
    }

    auto channel = std::make_unique<ComChannel>();
    channel->_server = server;

    // Authenticate
    bool authenticated = false;
    std::wstring version;
    hr = channel->Authenticate(token, authenticated, version);
    if (FAILED(hr) || !authenticated)
    {
        fprintf(stderr, "[wtcli] Authentication failed (0x%08lX)\n", hr);
        return nullptr;
    }

    return channel;
}

HRESULT ComChannel::Authenticate(const std::wstring& token, bool& authenticated, std::wstring& protocolVersion)
{
    BstrGuard bstrToken(ToBstr(token));
    BOOL auth = FALSE;
    BSTR version = nullptr;

    HRESULT hr = _server->Authenticate(bstrToken, &auth, &version);
    if (SUCCEEDED(hr))
    {
        authenticated = (auth != FALSE);
        protocolVersion = BstrToWstring(version);
        SysFreeString(version);
    }
    return hr;
}

HRESULT ComChannel::GetCapabilities(std::wstring& protocolVersion, std::wstring& supportedMethodsJson)
{
    BSTR ver = nullptr;
    BSTR methods = nullptr;
    HRESULT hr = _server->GetCapabilities(&ver, &methods);
    if (SUCCEEDED(hr))
    {
        protocolVersion = BstrToWstring(ver);
        supportedMethodsJson = BstrToWstring(methods);
        SysFreeString(ver);
        SysFreeString(methods);
    }
    return hr;
}

HRESULT ComChannel::GetActivePane(PROTOCOL_PANE_INFO& result)
{
    memset(&result, 0, sizeof(result));
    return _server->GetActivePane(&result);
}

HRESULT ComChannel::ListWindows(std::vector<PROTOCOL_WINDOW_INFO>& results)
{
    UINT32 count = 0;
    PROTOCOL_WINDOW_INFO* raw = nullptr;
    HRESULT hr = _server->ListWindows(&count, &raw);
    if (SUCCEEDED(hr) && raw)
    {
        results.assign(raw, raw + count);
        CoTaskMemFree(raw);
    }
    return hr;
}

HRESULT ComChannel::ListTabs(const std::wstring& windowIdFilter, std::vector<PROTOCOL_TAB_INFO>& results)
{
    BstrGuard filter(ToBstr(windowIdFilter));
    UINT32 count = 0;
    PROTOCOL_TAB_INFO* raw = nullptr;
    HRESULT hr = _server->ListTabs(filter, &count, &raw);
    if (SUCCEEDED(hr) && raw)
    {
        results.assign(raw, raw + count);
        CoTaskMemFree(raw);
    }
    return hr;
}

HRESULT ComChannel::ListPanes(const std::wstring& windowIdFilter, const std::wstring& tabIdFilter, std::vector<PROTOCOL_PANE_INFO>& results)
{
    BstrGuard wf(ToBstr(windowIdFilter));
    BstrGuard tf(ToBstr(tabIdFilter));
    UINT32 count = 0;
    PROTOCOL_PANE_INFO* raw = nullptr;
    HRESULT hr = _server->ListPanes(wf, tf, &count, &raw);
    if (SUCCEEDED(hr) && raw)
    {
        results.assign(raw, raw + count);
        CoTaskMemFree(raw);
    }
    return hr;
}

HRESULT ComChannel::ReadPaneOutput(const std::wstring& paneId, const std::wstring& source, int maxLines, PROTOCOL_PANE_OUTPUT& result)
{
    BstrGuard pid(ToBstr(paneId));
    BstrGuard src(ToBstr(source));
    memset(&result, 0, sizeof(result));
    return _server->ReadPaneOutput(pid, src, maxLines, &result);
}

HRESULT ComChannel::GetProcessStatus(const std::wstring& paneId, PROTOCOL_PROCESS_STATUS& result)
{
    BstrGuard pid(ToBstr(paneId));
    memset(&result, 0, sizeof(result));
    return _server->GetProcessStatus(pid, &result);
}

HRESULT ComChannel::GetSessionVariable(const std::wstring& paneId, const std::wstring& name, PROTOCOL_SESSION_VARIABLE& result)
{
    BstrGuard pid(ToBstr(paneId));
    BstrGuard n(ToBstr(name));
    memset(&result, 0, sizeof(result));
    return _server->GetSessionVariable(pid, n, &result);
}

HRESULT ComChannel::GetSettings(std::wstring& settingsJson)
{
    BSTR json = nullptr;
    HRESULT hr = _server->GetSettings(&json);
    if (SUCCEEDED(hr))
    {
        settingsJson = BstrToWstring(json);
        SysFreeString(json);
    }
    return hr;
}

HRESULT ComChannel::CreateTab(const std::wstring& windowId, const std::wstring& profile,
                               const std::wstring& commandline, const std::wstring& title,
                               bool suppressAppTitle, bool injectMcpCredentials, bool background,
                               PROTOCOL_TAB_CREATION_RESULT& result)
{
    BstrGuard wid(ToBstr(windowId));
    BstrGuard prof(ToBstr(profile));
    BstrGuard cmd(ToBstr(commandline));
    BstrGuard ttl(ToBstr(title));
    memset(&result, 0, sizeof(result));
    return _server->CreateTab(wid, prof, cmd, ttl,
                              suppressAppTitle ? TRUE : FALSE,
                              injectMcpCredentials ? TRUE : FALSE,
                              background ? TRUE : FALSE,
                              &result);
}

HRESULT ComChannel::SplitPane(const std::wstring& paneId, const std::wstring& direction, float size,
                               const std::wstring& profile, const std::wstring& commandline,
                               bool injectMcpCredentials, bool background,
                               PROTOCOL_TAB_CREATION_RESULT& result)
{
    BstrGuard pid(ToBstr(paneId));
    BstrGuard dir(ToBstr(direction));
    BstrGuard prof(ToBstr(profile));
    BstrGuard cmd(ToBstr(commandline));
    memset(&result, 0, sizeof(result));
    return _server->SplitPane(pid, dir, size, prof, cmd,
                              injectMcpCredentials ? TRUE : FALSE,
                              background ? TRUE : FALSE,
                              &result);
}

HRESULT ComChannel::ClosePane(const std::wstring& paneId)
{
    BstrGuard pid(ToBstr(paneId));
    return _server->ClosePane(pid);
}

HRESULT ComChannel::SendInput(const std::wstring& paneId, const std::wstring& text)
{
    BstrGuard pid(ToBstr(paneId));
    BstrGuard t(ToBstr(text));
    return _server->SendInput(pid, t);
}

HRESULT ComChannel::SetSessionVariable(const std::wstring& paneId, const std::wstring& name, const std::wstring& value)
{
    BstrGuard pid(ToBstr(paneId));
    BstrGuard n(ToBstr(name));
    BstrGuard v(ToBstr(value));
    return _server->SetSessionVariable(pid, n, v);
}

HRESULT ComChannel::SetSettings(const std::wstring& settingsContent, std::wstring& backupPath)
{
    BstrGuard content(ToBstr(settingsContent));
    BSTR backup = nullptr;
    HRESULT hr = _server->SetSettings(content, &backup);
    if (SUCCEEDED(hr))
    {
        backupPath = BstrToWstring(backup);
        SysFreeString(backup);
    }
    return hr;
}

// Channel::Connect factory — connects via COM using WT_COM_CLSID.
std::unique_ptr<Channel> Channel::Connect()
{
    wchar_t clsid[128]{};
    if (!GetEnvironmentVariableW(L"WT_COM_CLSID", clsid, ARRAYSIZE(clsid)))
    {
        fprintf(stderr, "[wtcli] WT_COM_CLSID not set. Must run inside a Windows Terminal pane.\n");
        return nullptr;
    }

    wchar_t token[256]{};
    GetEnvironmentVariableW(L"WT_MCP_TOKEN", token, ARRAYSIZE(token));

    return ComChannel::Connect(clsid, token);
}

// String conversion utilities
std::string WideToUtf8(const std::wstring& wide)
{
    if (wide.empty())
        return {};
    int len = WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), nullptr, 0, nullptr, nullptr);
    std::string result(len, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), result.data(), len, nullptr, nullptr);
    return result;
}

std::wstring Utf8ToWide(const std::string& utf8)
{
    if (utf8.empty())
        return {};
    int len = MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), nullptr, 0);
    std::wstring result(len, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), result.data(), len);
    return result;
}

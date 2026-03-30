#pragma once

#include <memory>
#include <string>
#include <vector>

#include <Windows.h>
#include <objbase.h>
#include "ITerminalProtocolServer.h"

// Abstract channel interface for communicating with Windows Terminal.
// Equivalent to Rust's WtChannel trait.
//
// COM channel implements methods directly via typed COM calls.
// Pipe channel implements via JSON wire protocol as a fallback.
struct Channel
{
    virtual ~Channel() = default;

    // Meta
    virtual HRESULT Authenticate(const std::wstring& token, bool& authenticated, std::wstring& protocolVersion) = 0;
    virtual HRESULT GetCapabilities(std::wstring& protocolVersion, std::wstring& supportedMethodsJson) = 0;

    // Queries
    virtual HRESULT GetActivePane(PROTOCOL_PANE_INFO& result) = 0;
    virtual HRESULT ListWindows(std::vector<PROTOCOL_WINDOW_INFO>& results) = 0;
    virtual HRESULT ListTabs(const std::wstring& windowIdFilter, std::vector<PROTOCOL_TAB_INFO>& results) = 0;
    virtual HRESULT ListPanes(const std::wstring& windowIdFilter, const std::wstring& tabIdFilter, std::vector<PROTOCOL_PANE_INFO>& results) = 0;
    virtual HRESULT ReadPaneOutput(const std::wstring& paneId, const std::wstring& source, int maxLines, PROTOCOL_PANE_OUTPUT& result) = 0;
    virtual HRESULT GetProcessStatus(const std::wstring& paneId, PROTOCOL_PROCESS_STATUS& result) = 0;
    virtual HRESULT GetSessionVariable(const std::wstring& paneId, const std::wstring& name, PROTOCOL_SESSION_VARIABLE& result) = 0;
    virtual HRESULT GetSettings(std::wstring& settingsJson) = 0;

    // Mutations
    virtual HRESULT CreateTab(const std::wstring& windowId, const std::wstring& profile,
                              const std::wstring& commandline, const std::wstring& title,
                              bool suppressAppTitle, bool injectMcpCredentials, bool background,
                              PROTOCOL_TAB_CREATION_RESULT& result) = 0;
    virtual HRESULT SplitPane(const std::wstring& paneId, const std::wstring& direction, float size,
                              const std::wstring& profile, const std::wstring& commandline,
                              bool injectMcpCredentials, bool background,
                              PROTOCOL_TAB_CREATION_RESULT& result) = 0;
    virtual HRESULT ClosePane(const std::wstring& paneId) = 0;
    virtual HRESULT SendInput(const std::wstring& paneId, const std::wstring& text) = 0;
    virtual HRESULT SetSessionVariable(const std::wstring& paneId, const std::wstring& name, const std::wstring& value) = 0;
    virtual HRESULT SetSettings(const std::wstring& settingsContent, std::wstring& backupPath) = 0;

    // Connect to Windows Terminal via COM (WT_COM_CLSID env var).
    static std::unique_ptr<Channel> Connect();
};

// Helper: free all BSTRs in a PROTOCOL_WINDOW_INFO and zero the struct.
inline void FreeWindowInfo(PROTOCOL_WINDOW_INFO& info)
{
    SysFreeString(info.WindowId);
    SysFreeString(info.Title);
    memset(&info, 0, sizeof(info));
}

inline void FreeTabInfo(PROTOCOL_TAB_INFO& info)
{
    SysFreeString(info.TabId);
    SysFreeString(info.WindowId);
    SysFreeString(info.Title);
    memset(&info, 0, sizeof(info));
}

inline void FreePaneInfo(PROTOCOL_PANE_INFO& info)
{
    SysFreeString(info.PaneId);
    SysFreeString(info.TabId);
    SysFreeString(info.WindowId);
    SysFreeString(info.Title);
    SysFreeString(info.Profile);
    memset(&info, 0, sizeof(info));
}

inline void FreePaneOutput(PROTOCOL_PANE_OUTPUT& info)
{
    SysFreeString(info.PaneId);
    SysFreeString(info.Content);
    memset(&info, 0, sizeof(info));
}

inline void FreeProcessStatus(PROTOCOL_PROCESS_STATUS& info)
{
    SysFreeString(info.PaneId);
    SysFreeString(info.State);
    memset(&info, 0, sizeof(info));
}

inline void FreeSessionVariable(PROTOCOL_SESSION_VARIABLE& info)
{
    SysFreeString(info.PaneId);
    SysFreeString(info.Name);
    SysFreeString(info.Value);
    memset(&info, 0, sizeof(info));
}

inline void FreeTabCreationResult(PROTOCOL_TAB_CREATION_RESULT& info)
{
    SysFreeString(info.TabId);
    SysFreeString(info.PaneId);
    SysFreeString(info.WindowId);
    memset(&info, 0, sizeof(info));
}

// Helper: convert BSTR to std::wstring (null-safe).
inline std::wstring BstrToWstring(BSTR bstr)
{
    return bstr ? std::wstring(bstr, SysStringLen(bstr)) : std::wstring{};
}

// Helper: convert std::wstring to narrow string (UTF-8).
std::string WideToUtf8(const std::wstring& wide);

// Helper: convert narrow string (UTF-8) to wide.
std::wstring Utf8ToWide(const std::string& utf8);

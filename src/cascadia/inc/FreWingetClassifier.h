// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// FreWingetClassifier.h
//
// Pure-function HRESULT → user-facing failure-kind classification for the
// FRE winget install path. Extracted from FreOverlay so unit tests in
// `src/cascadia/ut_app` can pin the contract without dragging in
// `TerminalApp` / WinRT / XAML.
//
// The mapping has two consumers inside FreOverlay::_WingetInstallAsync:
//  * the catch block for `winrt::hresult_error` thrown by the winget COM
//    surface (this is how policy blocks surface — winget throws
//    APPINSTALLER_CLI_ERROR_BLOCKED_BY_POLICY *before* it ever returns
//    an InstallResult), and
//  * the CatalogError install-status branch, where the structured Status
//    is generic but ExtendedErrorCode tells us why.

#pragma once

#include <cstdint>

namespace Microsoft::Terminal::FreWinget
{
    // Categorization of why a winget install failed. The Success sentinel
    // (-1) lets `_WingetInstallAsync` encode success/failure in a single
    // `IAsyncOperation<int32_t>` return value (WinRT projection can't
    // carry a richer struct without an IDL type).
    //
    // Values are part of the FreOverlay → _ShowWingetProblem cross-thread
    // contract; reordering them silently changes which user-facing
    // message is shown for an unchanged failure. If you add a new kind,
    // append it.
    enum class FailureKind : int32_t
    {
        Success = -1, // install completed OK
        Network = 0, // connect / download failed with a network-like HRESULT
        BlockedByPolicy = 1, // winget GP / org policy blocked the install
        PackageNotFound = 2, // catalog has no manifest with this ID
        NoCompatibleInstaller = 3, // manifest exists but no installer matches this OS/arch
        InstallerFailed = 4, // installer ran but reported an error (e.g. MSI 1603)
        Timeout = 5, // we hit our own 20-min hard timeout
        Generic = 6, // everything else (catalog corruption, internal error, unknown HRESULT, …)
    };

    // Decide whether an HRESULT looks like a network-class failure
    // (WinINet / WinHTTP / Winsock). Conservative whitelist of specific
    // codes rather than facility-range scans, to avoid misclassifying
    // HTTP-status HRESULTs (HTTP 404 is 0x80190194 — not a "check your
    // VPN" situation) or RPC failures as network issues.
    //
    // Names in trailing comments are the macros from winhttp.h / wininet.h
    // / winsock2.h, kept here so we don't need to pull those headers in.
    inline bool IsNetworkLikeHResult(int32_t hr) noexcept
    {
        switch (static_cast<uint32_t>(hr))
        {
        // FACILITY_INTERNET (12xxx range) — WinINet & WinHTTP share these
        case 0x80072EE2: // ERROR_INTERNET_TIMEOUT / ERROR_WINHTTP_TIMEOUT       (12002)
        case 0x80072EE7: // ERROR_INTERNET_NAME_NOT_RESOLVED                    (12007)
        case 0x80072EFD: // ERROR_INTERNET_CANNOT_CONNECT                       (12029)
        case 0x80072EFE: // ERROR_INTERNET_CONNECTION_ABORTED                   (12030)
        case 0x80072EFF: // ERROR_INTERNET_CONNECTION_RESET                     (12031)
        case 0x80072F8F: // ERROR_INTERNET_SECURITY_CHANNEL_ERROR (TLS)         (12175)
        // FACILITY_WIN32 (Winsock 100xx, mapped via HRESULT_FROM_WIN32)
        case 0x80072742: // WSAENETDOWN          (10050)
        case 0x80072743: // WSAENETUNREACH       (10051)
        case 0x80072744: // WSAENETRESET         (10052)
        case 0x80072745: // WSAECONNABORTED      (10053)
        case 0x80072746: // WSAECONNRESET        (10054)
        case 0x8007274C: // WSAETIMEDOUT         (10060)
        case 0x8007274D: // WSAECONNREFUSED      (10061)
        case 0x80072751: // WSAEHOSTUNREACH      (10065)
        case 0x80072AF9: // WSAHOST_NOT_FOUND    (11001)
        case 0x80072AFC: // WSANO_DATA           (11004)
            return true;
        default:
            return false;
        }
    }

    // Map a raw HRESULT to the most-specific FailureKind we can infer.
    //
    // The match order matters. APPINSTALLER_CLI_ERROR_* codes are
    // checked first because their meaning is unambiguous; the network
    // whitelist comes last as a transport-level fallback.
    //
    // Code names come from
    // https://github.com/microsoft/winget-cli/blob/master/src/AppInstallerSharedLib/Public/AppInstallerErrors.h
    // and are kept here as comments so we don't need to take a header
    // dependency on the winget-cli repo.
    inline FailureKind ClassifyWingetHResult(int32_t hr) noexcept
    {
        switch (static_cast<uint32_t>(hr))
        {
        // BlockedByPolicy family — group policy disabled winget or a
        // specific source/feature. Triggered by setting
        // HKLM\SOFTWARE\Policies\Microsoft\Windows\AppInstaller\EnableAppInstaller
        // (and friends) to 0.
        case 0x8A15003A: // APPINSTALLER_CLI_ERROR_BLOCKED_BY_POLICY
        case 0x8A15001B: // APPINSTALLER_CLI_ERROR_MSSTORE_BLOCKED_BY_POLICY
        case 0x8A15001C: // APPINSTALLER_CLI_ERROR_MSSTORE_APP_BLOCKED_BY_POLICY
        case 0x8A15001D: // APPINSTALLER_CLI_ERROR_EXPERIMENTAL_FEATURE_DISABLED
        case 0x8A15010F: // APPINSTALLER_CLI_ERROR_INSTALL_BLOCKED_BY_POLICY (install-phase variant of 0x8A15003A)
            return FailureKind::BlockedByPolicy;

        // Network / download failure codes that winget itself attaches
        // (separate from the generic WinINet/Winsock whitelist below).
        // INSTALL_NO_NETWORK is winget self-diagnosing "no network";
        // DOWNLOAD_FAILED is the install-phase wrapper around any
        // transport error during package download.
        case 0x8A150008: // APPINSTALLER_CLI_ERROR_DOWNLOAD_FAILED
        case 0x8A150107: // APPINSTALLER_CLI_ERROR_INSTALL_NO_NETWORK
            return FailureKind::Network;

        // Manifest was found but no installer entry matches this
        // machine's OS / architecture / scope. Usually surfaces as
        // InstallResultStatus::NoApplicableInstallers, but cover the
        // exception form for older winget versions / unusual flows.
        case 0x8A150010: // APPINSTALLER_CLI_ERROR_NO_APPLICABLE_INSTALLER
            return FailureKind::NoCompatibleInstaller;

        // No manifest with the requested package ID exists in any
        // configured source. Usually surfaces as
        // findResult.Matches().Size() == 0, but defensive coverage for
        // the exception form.
        case 0x8A150014: // APPINSTALLER_CLI_ERROR_NO_APPLICATIONS_FOUND
            return FailureKind::PackageNotFound;
        }

        // No winget-specific match — fall back to the transport-level
        // network whitelist (DNS / connect / TLS), then Generic.
        if (IsNetworkLikeHResult(hr))
        {
            return FailureKind::Network;
        }
        return FailureKind::Generic;
    }
}

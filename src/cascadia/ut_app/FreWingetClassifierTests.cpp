// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// FreWingetClassifierTests.cpp
//
// Pure-function tests for the FRE winget failure classifier at
// src/cascadia/inc/FreWingetClassifier.h. The classifier maps a raw
// HRESULT from the winget COM surface into one of the
// `FreWingetFailureKind` values that drive the FRE error-template
// selection inside `FreOverlay::_ShowWingetProblem`.
//
// Why each test exists is keyed to a checklist item in
// `doc/release-check-list.md § 0` (FRE winget install — failure-kind
// messages) so that a release tester can trace each kind's UI message
// back to the case that pins its classifier branch.
//
// No XAML, no winrt, no subprocess — just int32_t in, FailureKind out.

#include "precomp.h"

#include "../inc/FreWingetClassifier.h"

using namespace WEX::Logging;
using namespace WEX::TestExecution;
using namespace WEX::Common;
using namespace Microsoft::Terminal::FreWinget;

namespace TerminalAppUnitTests
{
    class FreWingetClassifierTests
    {
        TEST_CLASS(FreWingetClassifierTests);

        // _IsNetworkLikeHResult — checklist item: Network (transport whitelist).
        TEST_METHOD(NetworkWhitelistRecognizesEveryWinINetCode);
        TEST_METHOD(NetworkWhitelistRecognizesEveryWinsockCode);
        TEST_METHOD(NetworkWhitelistRejectsNonNetworkCodes);
        TEST_METHOD(NetworkWhitelistRejectsHttpStatusHResults);
        TEST_METHOD(NetworkWhitelistRejectsBlockedByPolicyCodes);
        TEST_METHOD(NetworkWhitelistRejectsSuccess);

        // _ClassifyWingetHResult — one TEST_METHOD per checklist kind.
        TEST_METHOD(ClassifiesBlockedByPolicyFamily);
        TEST_METHOD(ClassifiesWingetNativeNetworkCodes);
        TEST_METHOD(ClassifiesPackageNotFound);
        TEST_METHOD(ClassifiesNoCompatibleInstaller);
        TEST_METHOD(ClassifierFallsBackToNetworkWhitelist);
        TEST_METHOD(ClassifierFallsBackToGeneric);

        // Ordering / precedence — pins the contract that winget-specific
        // codes win over the transport-level network whitelist.
        TEST_METHOD(BlockedByPolicyBeatsNetworkFallback);

        // Sentinel — FailureKind::Success is a return-channel-only value;
        // 0 (S_OK) must classify as Generic, not Success.
        TEST_METHOD(SuccessSentinelIsReturnChannelOnly);
    };

    // ── _IsNetworkLikeHResult ───────────────────────────────────────────

    // Every WinINet / WinHTTP code in the whitelist must classify as
    // network. Listed individually so a regression on any one surfaces
    // with the specific HRESULT in the failure log, not a one-bit
    // "something in the loop broke" summary.
    void FreWingetClassifierTests::NetworkWhitelistRecognizesEveryWinINetCode()
    {
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072EE2)), L"ERROR_INTERNET_TIMEOUT");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072EE7)), L"ERROR_INTERNET_NAME_NOT_RESOLVED");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072EFD)), L"ERROR_INTERNET_CANNOT_CONNECT");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072EFE)), L"ERROR_INTERNET_CONNECTION_ABORTED");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072EFF)), L"ERROR_INTERNET_CONNECTION_RESET");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072F8F)), L"ERROR_INTERNET_SECURITY_CHANNEL_ERROR");
    }

    void FreWingetClassifierTests::NetworkWhitelistRecognizesEveryWinsockCode()
    {
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072742)), L"WSAENETDOWN");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072743)), L"WSAENETUNREACH");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072744)), L"WSAENETRESET");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072745)), L"WSAECONNABORTED");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072746)), L"WSAECONNRESET");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x8007274C)), L"WSAETIMEDOUT");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x8007274D)), L"WSAECONNREFUSED");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072751)), L"WSAEHOSTUNREACH");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072AF9)), L"WSAHOST_NOT_FOUND");
        VERIFY_IS_TRUE(IsNetworkLikeHResult(static_cast<int32_t>(0x80072AFC)), L"WSANO_DATA");
    }

    void FreWingetClassifierTests::NetworkWhitelistRejectsNonNetworkCodes()
    {
        // Common COM / generic failures that must NOT be misclassified as
        // network — otherwise an ACL denial or a bad-arg bug would send the
        // user chasing their VPN.
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x80004005)), L"E_FAIL");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x80070057)), L"E_INVALIDARG");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x80070005)), L"E_ACCESSDENIED");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8007000E)), L"E_OUTOFMEMORY");
    }

    void FreWingetClassifierTests::NetworkWhitelistRejectsHttpStatusHResults()
    {
        // HTTP-status HRESULTs (0x80190xxx family) mean the request reached
        // the server — these are NOT "check your VPN" situations. The
        // classifier-header comment calls this out explicitly; this test
        // pins that decision so we never regress to range-scanning.
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x80190194)), L"HTTP 404 NOT FOUND");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x80190193)), L"HTTP 403 FORBIDDEN");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x801901F4)), L"HTTP 500 INTERNAL SERVER ERROR");
    }

    void FreWingetClassifierTests::NetworkWhitelistRejectsBlockedByPolicyCodes()
    {
        // The five BlockedByPolicy HRESULTs share their facility with
        // winget-CLI errors, not with WinINet. If any of them ever
        // accidentally got into the network whitelist, the classifier
        // ordering would still produce BlockedByPolicy (winget-specific
        // codes are matched first) — but the whitelist itself must reject
        // them so the win-net fallback path stays clean.
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8A15003A)), L"BLOCKED_BY_POLICY");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8A15010F)), L"INSTALL_BLOCKED_BY_POLICY");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8A15001B)), L"MSSTORE_BLOCKED_BY_POLICY");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8A15001C)), L"MSSTORE_APP_BLOCKED_BY_POLICY");
        VERIFY_IS_FALSE(IsNetworkLikeHResult(static_cast<int32_t>(0x8A15001D)), L"EXPERIMENTAL_FEATURE_DISABLED");
    }

    void FreWingetClassifierTests::NetworkWhitelistRejectsSuccess()
    {
        // S_OK isn't a failure HRESULT at all. The whitelist must say
        // "not network" so callers (`_WingetInstallAsync` DownloadError
        // branch) don't show the user a network error for a 0 HRESULT.
        VERIFY_IS_FALSE(IsNetworkLikeHResult(0));
    }

    // ── _ClassifyWingetHResult ──────────────────────────────────────────

    // Checklist item: BlockedByPolicy.
    // All five APPINSTALLER_CLI_ERROR_*_BLOCKED_BY_POLICY family codes
    // (plus EXPERIMENTAL_FEATURE_DISABLED, which surfaces the same UI
    // message) must classify as BlockedByPolicy.
    void FreWingetClassifierTests::ClassifiesBlockedByPolicyFamily()
    {
        VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(0x8A15003A)), L"BLOCKED_BY_POLICY");
        VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(0x8A15001B)), L"MSSTORE_BLOCKED_BY_POLICY");
        VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(0x8A15001C)), L"MSSTORE_APP_BLOCKED_BY_POLICY");
        VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(0x8A15001D)), L"EXPERIMENTAL_FEATURE_DISABLED");
        VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(0x8A15010F)), L"INSTALL_BLOCKED_BY_POLICY");
    }

    // Checklist item: Network (winget-CLI native codes, separate from the
    // transport-level fallback below).
    void FreWingetClassifierTests::ClassifiesWingetNativeNetworkCodes()
    {
        VERIFY_ARE_EQUAL(FailureKind::Network, ClassifyWingetHResult(static_cast<int32_t>(0x8A150008)), L"DOWNLOAD_FAILED");
        VERIFY_ARE_EQUAL(FailureKind::Network, ClassifyWingetHResult(static_cast<int32_t>(0x8A150107)), L"INSTALL_NO_NETWORK");
    }

    // Checklist item: PackageNotFound.
    void FreWingetClassifierTests::ClassifiesPackageNotFound()
    {
        VERIFY_ARE_EQUAL(FailureKind::PackageNotFound, ClassifyWingetHResult(static_cast<int32_t>(0x8A150014)), L"NO_APPLICATIONS_FOUND");
    }

    // Checklist item: NoCompatibleInstaller.
    void FreWingetClassifierTests::ClassifiesNoCompatibleInstaller()
    {
        VERIFY_ARE_EQUAL(FailureKind::NoCompatibleInstaller, ClassifyWingetHResult(static_cast<int32_t>(0x8A150010)), L"NO_APPLICABLE_INSTALLER");
    }

    // Checklist item: Network (transport-level fallback). When an HRESULT
    // is NOT a winget-specific code, the classifier must fall through to
    // the WinINet/Winsock whitelist. Two distinct codes used here so a
    // bug that hardcodes one specific value still surfaces.
    void FreWingetClassifierTests::ClassifierFallsBackToNetworkWhitelist()
    {
        VERIFY_ARE_EQUAL(FailureKind::Network, ClassifyWingetHResult(static_cast<int32_t>(0x80072EE7)), L"ERROR_INTERNET_NAME_NOT_RESOLVED");
        VERIFY_ARE_EQUAL(FailureKind::Network, ClassifyWingetHResult(static_cast<int32_t>(0x8007274D)), L"WSAECONNREFUSED");
    }

    // Checklist item: Generic / GenericNoCode. Unrecognized HRESULTs
    // (including general COM errors and otherwise-unknown winget facility
    // codes) must classify as Generic so `_ShowWingetProblem` picks the
    // generic template instead of mis-labeling the failure.
    void FreWingetClassifierTests::ClassifierFallsBackToGeneric()
    {
        VERIFY_ARE_EQUAL(FailureKind::Generic, ClassifyWingetHResult(static_cast<int32_t>(0x80004005)), L"E_FAIL");
        VERIFY_ARE_EQUAL(FailureKind::Generic, ClassifyWingetHResult(static_cast<int32_t>(0x80070057)), L"E_INVALIDARG");
        // 0x8A150003 is a real but currently-unrecognized winget HRESULT
        // (APPINSTALLER_CLI_ERROR_COMMAND_FAILED). Until/unless we add a
        // kind for it, it must Generic — pinning this avoids surprising
        // users with the wrong actionable hint.
        VERIFY_ARE_EQUAL(FailureKind::Generic, ClassifyWingetHResult(static_cast<int32_t>(0x8A150003)), L"COMMAND_FAILED (currently unmapped)");
        VERIFY_ARE_EQUAL(FailureKind::Generic, ClassifyWingetHResult(static_cast<int32_t>(0x80190194)), L"HTTP 404 (not a transport error)");
    }

    // Classifier ordering contract: winget-CLI codes match BEFORE the
    // network whitelist runs. If we ever swap the order, a
    // BlockedByPolicy code could accidentally classify as Network when
    // its numeric value happens to overlap with a future whitelist
    // expansion. None of the 5 BlockedByPolicy codes is in the current
    // whitelist, so today this is purely a contract assertion.
    void FreWingetClassifierTests::BlockedByPolicyBeatsNetworkFallback()
    {
        for (const uint32_t code : { 0x8A15003AU, 0x8A15001BU, 0x8A15001CU, 0x8A15001DU, 0x8A15010FU })
        {
            VERIFY_ARE_EQUAL(FailureKind::BlockedByPolicy, ClassifyWingetHResult(static_cast<int32_t>(code)));
        }
    }

    // FailureKind::Success is reserved as the IAsyncOperation<int32_t>
    // return-channel "no failure" sentinel. The classifier is only ever
    // called on a failed install, so it must never return Success — and
    // S_OK (0) specifically must classify as Generic, not Success, so a
    // caller that accidentally feeds in `installResult.ExtendedErrorCode()`
    // on a successful install doesn't get a "no failure" answer that
    // would suppress the actionable template.
    void FreWingetClassifierTests::SuccessSentinelIsReturnChannelOnly()
    {
        VERIFY_ARE_EQUAL(FailureKind::Generic, ClassifyWingetHResult(0));
    }
}

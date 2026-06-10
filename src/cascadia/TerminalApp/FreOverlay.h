// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include "FreAgentEntry.g.h"
#include "FreOverlay.g.h"

namespace winrt::TerminalApp::implementation
{
    struct FreAgentEntry : FreAgentEntryT<FreAgentEntry>
    {
        FreAgentEntry() = default;

        winrt::hstring Id() const { return _id; }
        void Id(const winrt::hstring& value) { _id = value; }
        winrt::hstring DisplayLabel() const { return _displayLabel; }
        void DisplayLabel(const winrt::hstring& value) { _displayLabel = value; }

    private:
        winrt::hstring _id;
        winrt::hstring _displayLabel;
    };

    struct FreOverlay : FreOverlayT<FreOverlay>
    {
        FreOverlay();

        // Initialize with settings to populate controls.
        void Initialize(const winrt::Microsoft::Terminal::Settings::Model::CascadiaSettings& settings);

        // Event — sender must be the WinRT projected type.
        til::typed_event<winrt::TerminalApp::FreOverlay, winrt::Windows::Foundation::IInspectable> Completed;

        // XAML event handlers — must be public for generated code access.
        void _OnNextButtonClick(const winrt::Windows::Foundation::IInspectable& sender,
                                const winrt::Windows::UI::Xaml::RoutedEventArgs& args);
        void _OnSaveButtonClick(const winrt::Windows::Foundation::IInspectable& sender,
                                const winrt::Windows::UI::Xaml::RoutedEventArgs& args);
        void _OnCloseButtonClick(const winrt::Windows::Foundation::IInspectable& sender,
                                 const winrt::Windows::UI::Xaml::RoutedEventArgs& args);
        void _OnAgentSelectionChanged(const winrt::Windows::Foundation::IInspectable& sender,
                                      const winrt::Windows::UI::Xaml::Controls::SelectionChangedEventArgs& args);
        void _OnSessionManagementToggled(const winrt::Windows::Foundation::IInspectable& sender,
                                         const winrt::Windows::UI::Xaml::RoutedEventArgs& args);
        void _OnAutoDetectToggled(const winrt::Windows::Foundation::IInspectable& sender,
                                  const winrt::Windows::UI::Xaml::RoutedEventArgs& args);

        // No-op kept for IDL compatibility.
        void ResetDragOffset();

    private:
        winrt::Microsoft::Terminal::Settings::Model::CascadiaSettings _settings{ nullptr };

        // Things that can block FRE completion, in priority order (lower value
        // = higher priority). Only the highest-priority problem is surfaced in
        // the bottom-left error area at a time (see _ShowProblem).
        enum class FreProblemKind
        {
            WingetMissing = 0, // hard prerequisite — winget itself unavailable
            CopilotInstall = 1, // hard prerequisite — winget GitHub.Copilot
            NodeInstall = 2, // hard prerequisite — winget OpenJS.NodeJS.LTS
            ShellIntegrationExecutionPolicy = 3, // optional feature — error detection blocked by PowerShell execution policy
            ShellIntegration = 4, // optional feature — error detection (generic install failure)
            Hooks = 5, // optional feature — session management
        };

        // Show a single problem: set the error message + manual-fix link, then
        // apply that problem's remediation (toggle off the affected feature, if
        // any) and re-enable the Save button. Does not raise Completed.
        void _ShowProblem(FreProblemKind kind);

        // Apply the detection→suggestion master-detail dependency: detection
        // off turns the suggestion toggle off and disables it; detection on
        // re-enables it (preserving the stored value).
        void _UpdateSuggestionEnabledState();

        // (Re)build the agent dropdown from the GPO-filtered registry, labeling
        // each entry with its live install state. Safe to call repeatedly (e.g.
        // after a save) and preserves the current selection.
        void _PopulateAgentComboBox();

        // Detect whether an executable is on PATH.
        static bool _IsAgentInstalled(const wchar_t* name);
        static bool _IsNodeInstalled();
        static bool _IsWingetInstalled();

        // ── WinGet source pre-warm coordination ─────────────────────
        // While the FRE overlay is on screen (Welcome + Settings pages),
        // pre-warm winget's source manifest cache in the background so
        // the on-Save `winget install` skips the 3-20s source refresh.
        // Single-flight per process — reentrant Initialize() calls and
        // multi-window FRE coalesce onto one running prewarm. The Save
        // handler awaits s_prewarmAction before its own winget call to
        // guarantee the two winget operations never run concurrently
        // (winget's intra-process locking is not a guaranteed contract).
        static std::mutex s_prewarmMutex;
        static winrt::Windows::Foundation::IAsyncAction s_prewarmAction;

        static void _MaybeStartPrewarm(bool copilotMissing, bool nodeMissing);
        static winrt::Windows::Foundation::IAsyncAction _RunPrewarmAsync();

        // Run a winget install synchronously on a background thread.
        // Returns true on success.
        static winrt::Windows::Foundation::IAsyncOperation<bool> _WingetInstallAsync(winrt::hstring packageId);

        // Run wta.exe hooks install on a background thread.
        // Returns true on success.
        static winrt::Windows::Foundation::IAsyncOperation<bool> _InstallHooksAsync(winrt::hstring agentId);


        // Perform the full save + install flow asynchronously.
        winrt::Windows::Foundation::IAsyncAction _SaveAndInstallAsync();

        // Flip the overlay between "saving / installing in progress" and
        // "idle / editable" states. While saving: a modal SavingOverlay
        // covers the settings form with a centered ProgressRing +
        // "Setting up..." text, the form underneath is disabled
        // (blocks keyboard too — pointer is caught by the overlay's
        // Background), and the Save button is disabled. On error or
        // completion the inverse is applied so the user can edit and
        // retry (or click Save again).
        void _SetSavingState(bool saving);
    };
}

namespace winrt::TerminalApp::factory_implementation
{
    BASIC_FACTORY(FreAgentEntry);
    BASIC_FACTORY(FreOverlay);
}

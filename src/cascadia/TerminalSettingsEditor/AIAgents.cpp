// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#include "pch.h"

// The agent page subtitle uses inline <Run> + <Hyperlink> elements; we
// populate their Text from code-behind because x:Uid on inline Run is not
// reliably honored by ResourceLoader in this UWP/WinUI 2 build.
#include <winrt/Windows.UI.Xaml.Documents.h>

#include "AIAgents.h"
#include "AIAgents.g.cpp"

using namespace winrt::Windows::UI::Xaml;
using namespace winrt::Windows::UI::Xaml::Controls;
using namespace winrt::Windows::UI::Xaml::Documents;
using namespace winrt::Windows::UI::Xaml::Navigation;
using namespace winrt::Microsoft::Terminal::Settings::Model;

namespace winrt::Microsoft::Terminal::Settings::Editor::implementation
{
    AIAgents::AIAgents()
    {
        InitializeComponent();

        PageSubtitlePrefix().Text(RS_(L"AIAgents_PageSubtitlePrefix"));
        PageSubtitlePrivacyLink().Text(RS_(L"AIAgents_PageSubtitlePrivacyLink"));

        // The Agent card is hand-built (not a SettingContainer) so we can put
        // the "Learn more about ACP" link in Column 0 directly below the
        // description text. Re-use the existing AIAgents_AcpAgent resw keys
        // (originally targeting SettingContainer's Header/HelpText DPs) via
        // ResourceLoader path syntax so we don't have to add new keys + 89
        // locale translations for the same strings.
        const auto agentHeader = RS_(L"AIAgents_AcpAgent/Header");
        AcpAgentHeaderText().Text(agentHeader);
        AcpAgentDescriptionText().Text(RS_(L"AIAgents_AcpAgent/HelpText"));
        Automation::AutomationProperties::SetName(AcpAgent(), agentHeader);
    }

    void AIAgents::OnNavigatedTo(const NavigationEventArgs& e)
    {
        const auto args = e.Parameter().as<Editor::NavigateToPageArgs>();
        _ViewModel = args.ViewModel().as<Editor::AIAgentsViewModel>();
        BringIntoViewWhenLoaded(args.ElementToFocus());
    }
}

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

        // Description: render "ACP" as an inline Hyperlink by splitting the
        // localized string on the literal "ACP" token (locked in every
        // locale's resw via {Locked="ACP"}). Mirrors the PageSubtitlePrivacyLink
        // approach above (x:Uid on inline Run is not reliably honored by
        // ResourceLoader, so we set Run.Text from code).
        {
            const std::wstring_view desc{ RS_(L"AIAgents_AcpAgent/HelpText") };
            constexpr std::wstring_view token{ L"ACP" };
            const auto pos = desc.find(token);
            if (pos != std::wstring_view::npos)
            {
                AcpAgentDescriptionBefore().Text(winrt::hstring{ desc.substr(0, pos) });
                AcpAgentDescriptionAcpToken().Text(winrt::hstring{ token });
                AcpAgentDescriptionAfter().Text(winrt::hstring{ desc.substr(pos + token.size()) });
            }
            else
            {
                // Fallback (shouldn't happen — ACP is locked): degrade to plain text.
                AcpAgentDescriptionBefore().Text(winrt::hstring{ desc });
            }
        }

        Automation::AutomationProperties::SetName(AcpAgent(), agentHeader);
    }

    void AIAgents::OnNavigatedTo(const NavigationEventArgs& e)
    {
        const auto args = e.Parameter().as<Editor::NavigateToPageArgs>();
        _ViewModel = args.ViewModel().as<Editor::AIAgentsViewModel>();
        BringIntoViewWhenLoaded(args.ElementToFocus());
    }
}

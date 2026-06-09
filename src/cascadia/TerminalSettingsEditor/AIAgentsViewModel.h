// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#pragma once

#include "AIAgentsViewModel.g.h"
#include "AcpModelEntry.g.h"
#include "AgentEntry.g.h"
#include "ViewModelHelpers.h"
#include "Utils.h"

namespace winrt::Microsoft::Terminal::Settings::Editor::implementation
{
    struct AgentEntry : AgentEntryT<AgentEntry>
    {
        AgentEntry(winrt::hstring id, winrt::hstring displayName, bool isInstalled);

        winrt::hstring Id() const { return _id; }
        winrt::hstring DisplayName() const { return _displayName; }
        winrt::hstring DisplayLabel() const;
        bool IsInstalled() const { return _isInstalled; }
        bool IsAddNew() const { return _isAddNew; }

        void SetAddNew(bool value) { _isAddNew = value; }

    private:
        winrt::hstring _id;
        winrt::hstring _displayName;
        bool _isInstalled;
        bool _isAddNew{ false };
    };

    struct AcpModelEntry : AcpModelEntryT<AcpModelEntry>
    {
        AcpModelEntry(winrt::hstring id, winrt::hstring displayName, winrt::hstring description) :
            _id{ std::move(id) },
            _displayName{ std::move(displayName) },
            _description{ std::move(description) }
        {
        }

        winrt::hstring Id() const { return _id; }
        winrt::hstring DisplayName() const { return _displayName; }
        winrt::hstring Description() const { return _description; }

    private:
        winrt::hstring _id;
        winrt::hstring _displayName;
        winrt::hstring _description;
    };

    struct AIAgentsViewModel : AIAgentsViewModelT<AIAgentsViewModel>, ViewModelHelper<AIAgentsViewModel>
    {
    public:
        AIAgentsViewModel(Model::GlobalAppSettings globalSettings);
        ~AIAgentsViewModel();

        using ViewModelHelper<AIAgentsViewModel>::PropertyChanged;

        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry> AcpAgentList() const { return _acpAgentList; }
        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry> DelegateAgentList() const { return _delegateAgentList; }

        Editor::AgentEntry CurrentAcpAgent();
        void CurrentAcpAgent(const Editor::AgentEntry& value);
        Editor::AgentEntry CurrentDelegateAgent();
        void CurrentDelegateAgent(const Editor::AgentEntry& value);

        // Custom agent preview
        bool IsCustomAcpAgentSelected();
        winrt::hstring CustomAcpCommandPreview();
        void EditCustomAcpAgent();
        bool IsCustomDelegateAgentSelected();
        winrt::hstring CustomDelegateCommandPreview();
        void EditCustomDelegateAgent();

        // Edit mode
        bool IsAddingCustomAcpAgent() const { return _isAddingCustomAcpAgent; }
        bool IsAddingCustomDelegateAgent() const { return _isAddingCustomDelegateAgent; }

        winrt::hstring CustomAcpCommand() const { return _customAcpCommand; }
        void CustomAcpCommand(const winrt::hstring& value);
        winrt::hstring CustomDelegateCommand() const { return _customDelegateCommand; }
        void CustomDelegateCommand(const winrt::hstring& value);

        void SaveCustomAcpAgent();
        void SaveCustomDelegateAgent();
        void CancelCustomAcpAgent();
        void DeleteCustomAcpAgent();
        void CancelCustomDelegateAgent();
        void DeleteCustomDelegateAgent();

        bool ShowAcpModel();
        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AcpModelEntry> AcpModelList() const { return _acpModelList; }
        // Probe in flight counts as "present" so the ComboBox stays
        // visible (PlaceholderText="Default") instead of flashing the
        // free-form textbox during the probe window.
        bool HasAcpModelList() const { return _acpModelList && (_acpModelList.Size() > 0 || _acpProbing); }
        bool ShowAcpModelTextBox() const { return !HasAcpModelList(); }
        Editor::AcpModelEntry CurrentAcpModelEntry();
        void CurrentAcpModelEntry(const Editor::AcpModelEntry& value);
        PERMANENT_OBSERVABLE_PROJECTED_SETTING(_GlobalSettings, AcpModel);
        bool ShowDelegateModel();
        PERMANENT_OBSERVABLE_PROJECTED_SETTING(_GlobalSettings, DelegateModel);
        bool AutoErrorDetectionEnabled() const;
        void AutoErrorDetectionEnabled(bool value);
        bool HasAutoErrorDetectionEnabled() const;
        bool AutoFixEnabled() const;
        void AutoFixEnabled(bool value);
        bool HasAutoFixEnabled() const;
        bool CanSuggestErrors() const;

        // GPO policy lock indicators
        bool IsAgentPolicyLocked() const { return _GlobalSettings.IsAgentPolicyLocked(); }
        bool IsCustomAgentPolicyLocked() const { return _GlobalSettings.IsCustomAgentPolicyLocked(); }
        bool IsAutoFixPolicyLocked() const { return _GlobalSettings.IsAutoFixPolicyLocked(); }
        bool IsAgentSessionHooksPolicyLocked() const { return _GlobalSettings.IsAgentSessionHooksPolicyLocked(); }

        winrt::Windows::Foundation::Collections::IObservableVector<winrt::Microsoft::Terminal::Settings::Editor::EnumEntry> AgentPanePositionList();
        winrt::Windows::Foundation::IInspectable CurrentAgentPanePosition();
        void CurrentAgentPanePosition(const winrt::Windows::Foundation::IInspectable& value);

        til::typed_event<Editor::AIAgentsViewModel, Model::ShellIntegrationTarget> InitShellIntegrationRequested;

    private:
        Model::GlobalAppSettings _GlobalSettings;
        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry> _acpAgentList;
        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry> _delegateAgentList;
        winrt::Windows::Foundation::Collections::IObservableVector<Editor::AcpModelEntry> _acpModelList;

        winrt::Windows::Foundation::Collections::IObservableVector<winrt::Microsoft::Terminal::Settings::Editor::EnumEntry> _agentPanePositionList;
        winrt::Windows::Foundation::Collections::IMap<winrt::hstring, winrt::Microsoft::Terminal::Settings::Editor::EnumEntry> _agentPanePositionMap;

        bool _isAddingCustomAcpAgent{ false };
        bool _isAddingCustomDelegateAgent{ false };
        winrt::hstring _customAcpCommand;
        winrt::hstring _customDelegateCommand;

        winrt::event_token _acpRuntimeChangedToken{};
        void _RebuildAcpModelListFromCache();

        // ── ACP model probe ──
        // A background `wta probe-models --agent <cmd>` invocation that
        // populates the dropdown after the user picks a new agent in
        // Settings, without waiting for the agent pane to be rebuilt.
        // See `_TriggerAcpModelProbe` in the .cpp for the full flow.
        bool _acpProbing{ false };
        // Generation counter: bumped each time _TriggerAcpModelProbe
        // fires. An in-flight probe checks this before publishing its
        // result and bails if a newer trigger has superseded it (user
        // picked a different agent while we were still talking to the
        // previous one).
        uint64_t _acpProbeGeneration{ 0 };
        void _TriggerAcpModelProbe();
        winrt::fire_and_forget _RunAcpModelProbeAsync(std::wstring agentCmdline, uint64_t generation);
        // Mirror of TerminalPage::_ResolveEffectiveAgentCliPath. Kept
        // here (rather than in inc/) because the Settings UI sits in
        // a separate project and can't include TerminalApp headers.
        std::wstring _ResolveEffectiveAcpAgentCmdline() const;

        static bool _IsAgentInstalled(const wchar_t* name);
        static bool _IsKnownAgent(const winrt::hstring& id);
        static winrt::hstring _DeriveId(const winrt::hstring& command);
        Editor::AgentEntry _FindEntryById(
            const winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry>& list,
            const winrt::hstring& id) const;
        void _AppendAddNewEntry(
            winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry>& list);
        void _MaybeAppendCustomEntry(
            winrt::Windows::Foundation::Collections::IObservableVector<Editor::AgentEntry>& list,
            const winrt::hstring& customCommand,
            const winrt::hstring& currentAgentId);
    };
};

namespace winrt::Microsoft::Terminal::Settings::Editor::factory_implementation
{
    BASIC_FACTORY(AIAgentsViewModel);
    BASIC_FACTORY(AgentEntry);
    BASIC_FACTORY(AcpModelEntry);
}

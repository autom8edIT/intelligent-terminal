@echo off
cd /d "%~dp0"
set MSBUILD="C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe"
set SOLUTION_DIR=%CD%\
rem Pin the SDK to 10.0.22621.0. Without this, VS's Microsoft.Cpp.WindowsSDK
rem props default WindowsTargetPlatformVersion to the LATEST installed SDK
rem (10.0.26100.0 on this machine), which wins over the repo's conditional
rem default in common.build.pre.props. The 26100 XAML compiler then throws
rem WMC9999 on the Settings Editor. A command-line /p: global property
rem overrides the per-project default everywhere, forcing the 22621
rem XamlCompiler that this repo's XAML is validated against.
set COMMON=/p:Platform=x64 /p:Configuration=Release /p:WindowsTerminalBranding=Dev /p:WindowsTargetPlatformVersion=10.0.22621.0 /p:SolutionDir=%SOLUTION_DIR% /m /nologo

rem Wipe the wapproj's Release intermediates so glob-based Content items
rem (like wt-agent-hooks\**) get re-evaluated. Without this, an incremental
rem MSIX build keeps the cached file list and silently drops freshly-added
rem files from the package.
if exist "src\cascadia\CascadiaPackage\obj\x64\Release" rmdir /s /q "src\cascadia\CascadiaPackage\obj\x64\Release"
if exist "src\cascadia\CascadiaPackage\bin\x64\Release\AppX" rmdir /s /q "src\cascadia\CascadiaPackage\bin\x64\Release\AppX"

rem Build OpenConsoleProxy first. It MIDL-generates ITerminalHandoff.h /
rem IConsoleHandoff.h into obj\x64\Release\OpenConsoleProxy, which
rem TerminalConnection includes via $(IntDir)..\OpenConsoleProxy but does
rem NOT carry as a ProjectReference. Building the Settings projects below
rem reaches TerminalConnection transitively, so without this the header is
rem missing and the build fails with C1083: 'ITerminalHandoff.h' not found.
%MSBUILD% src\host\proxy\Host.Proxy.vcxproj %COMMON% >> _build_msix_x64.log 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo OpenConsoleProxy build failed: %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)

rem Build Settings Model first. Its winmd is the source-of-truth for the
rem Profile / Globals WinRT projection. If we don't pin its build ahead
rem of consumer projects, cppwinrt can scan a stale older winmd elsewhere
rem and generate consumer projections missing newer members (e.g.
rem DragDropDelimiter), producing C2039 in TerminalSettingsAppAdapterLib.
%MSBUILD% src\cascadia\TerminalSettingsModel\Microsoft.Terminal.Settings.ModelLib.vcxproj %COMMON% >> _build_msix_x64.log 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Settings Model build failed: %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)

rem Build Settings Editor next (generates XBF files)
%MSBUILD% src\cascadia\TerminalSettingsEditor\Microsoft.Terminal.Settings.Editor.vcxproj %COMMON% >> _build_msix_x64.log 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Settings Editor build failed: %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)

rem Now build the full package
%MSBUILD% src\cascadia\CascadiaPackage\CascadiaPackage.wapproj %COMMON% /p:GenerateAppxPackageOnBuild=true /p:AppxBundle=Never >> _build_msix_x64.log 2>&1
set BUILD_EXIT=%ERRORLEVEL%
echo Exit code: %BUILD_EXIT%
exit /b %BUILD_EXIT%

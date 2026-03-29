[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PayloadZip,

    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\AgenticTerminal",

    [switch]$NoPathUpdate,

    [switch]$NoShortcuts,

    [string]$StartMenuDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Agentic Terminal",

    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'

function Write-Status {
    param([string]$Message)

    if (-not $Quiet) {
        Write-Host $Message
    }
}

function Ensure-Directory {
    param([string]$Path)

    if (-not (Test-Path $Path -PathType Container)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
}

function Remove-DirectoryContents {
    param([string]$Path)

    if (Test-Path $Path -PathType Container) {
        Get-ChildItem $Path -Force | Remove-Item -Recurse -Force
    }
}

function Add-InstallDirToUserPath {
    param([string]$PathToAdd)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @()
    if (-not [string]::IsNullOrWhiteSpace($current)) {
        $parts = $current.Split(';') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    if ($parts -contains $PathToAdd) {
        return
    }

    $updated = @($parts + $PathToAdd) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
}

function New-Shortcut {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ShortcutPath,

        [Parameter(Mandatory = $true)]
        [string]$TargetPath,

        [string]$WorkingDirectory
    )

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetPath
    if ($WorkingDirectory) {
        $shortcut.WorkingDirectory = $WorkingDirectory
    }
    $shortcut.Save()
}

if (-not (Test-Path $PayloadZip -PathType Leaf)) {
    throw "Payload zip not found: $PayloadZip"
}

$payloadRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("agentic-terminal-install-" + [Guid]::NewGuid().ToString("N"))
$expandedRoot = Join-Path $payloadRoot 'expanded'

try {
    Ensure-Directory $payloadRoot
    Ensure-Directory $expandedRoot

    Write-Status "Extracting installer payload..."
    Expand-Archive -Path $PayloadZip -DestinationPath $expandedRoot -Force

    $sourceRoot = $expandedRoot
    $children = @(Get-ChildItem $expandedRoot)
    if ($children.Count -eq 1 -and $children[0].PSIsContainer) {
        $sourceRoot = $children[0].FullName
    }

    Ensure-Directory $InstallDir
    Write-Status "Installing to $InstallDir ..."
    Remove-DirectoryContents $InstallDir
    Copy-Item -Path (Join-Path $sourceRoot '*') -Destination $InstallDir -Recurse -Force

    $terminalExe = Join-Path $InstallDir 'WindowsTerminal.exe'
    $wtaExe = Join-Path $InstallDir 'wta.exe'

    if (-not $NoShortcuts) {
        Ensure-Directory $StartMenuDir

        if (Test-Path $terminalExe -PathType Leaf) {
            New-Shortcut -ShortcutPath (Join-Path $StartMenuDir 'Agentic Terminal.lnk') -TargetPath $terminalExe -WorkingDirectory $InstallDir
        }
        if (Test-Path $wtaExe -PathType Leaf) {
            New-Shortcut -ShortcutPath (Join-Path $StartMenuDir 'WTA.lnk') -TargetPath $wtaExe -WorkingDirectory $InstallDir
        }
    }

    if (-not $NoPathUpdate) {
        Write-Status "Adding install directory to user PATH ..."
        Add-InstallDirToUserPath -PathToAdd $InstallDir
    }

    Write-Status "Installation complete."
}
finally {
    if (Test-Path $payloadRoot -PathType Container) {
        Remove-Item $payloadRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

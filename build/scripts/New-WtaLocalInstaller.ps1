[CmdletBinding()]
param(
    [ValidateSet('ARM64', 'x64', 'x86')]
    [string]$Platform = 'ARM64',

    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',

    [string]$Destination = (Join-Path $PSScriptRoot '..\..\artifacts\local-installer'),

    [string]$TerminalMsix,

    [string]$XamlAppx,

    [switch]$BuildTerminal,

    [switch]$SkipWtaBuild,

    [string]$WtaExePath
)

$ErrorActionPreference = 'Stop'

function Write-Status {
    param([string]$Message)

    Write-Host "[local-installer] $Message"
}

function Ensure-Directory {
    param([string]$Path)

    if (-not (Test-Path $Path -PathType Container)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
}

function Resolve-AbsolutePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [string]$BasePath = (Get-Location).Path
    )

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }

    return [System.IO.Path]::GetFullPath((Join-Path $BasePath $Path))
}

function Get-RustTarget {
    param([string]$PlatformName)

    switch ($PlatformName) {
        'ARM64' { return 'aarch64-pc-windows-msvc' }
        'x64' { return 'x86_64-pc-windows-msvc' }
        'x86' { return 'i686-pc-windows-msvc' }
        default { throw "Unsupported platform: $PlatformName" }
    }
}

function Get-XamlDependencyArch {
    param([string]$PlatformName)

    switch ($PlatformName) {
        'ARM64' { return 'arm64' }
        'x64' { return 'x64' }
        'x86' { return 'x86' }
        default { throw "Unsupported platform: $PlatformName" }
    }
}

function Find-CargoPath {
    $command = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $fallback = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path $fallback -PathType Leaf) {
        return $fallback
    }

    throw 'Could not find cargo.exe. Install Rust or add cargo.exe to PATH.'
}

function Get-InstalledRustTargets {
    $rustupPath = Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe'
    if (-not (Test-Path $rustupPath -PathType Leaf)) {
        return @()
    }

    $targets = & $rustupPath target list --installed
    if ($LASTEXITCODE -ne 0) {
        throw 'rustup target list --installed failed.'
    }

    return @($targets | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Invoke-RustBuild {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CargoPath,

        [Parameter(Mandatory = $true)]
        [string]$ManifestPath,

        [Parameter(Mandatory = $true)]
        [string]$RustTarget
    )

    $previousRustFlags = $env:RUSTFLAGS
    try {
        $crtFlags = '-C target-feature=+crt-static'
        if ([string]::IsNullOrWhiteSpace($previousRustFlags)) {
            $env:RUSTFLAGS = $crtFlags
        } else {
            $env:RUSTFLAGS = '{0} {1}' -f $previousRustFlags, $crtFlags
        }

        & $CargoPath build --manifest-path $ManifestPath --release --target $RustTarget
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed for $ManifestPath."
        }
    }
    finally {
        if ([string]::IsNullOrWhiteSpace($previousRustFlags)) {
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:RUSTFLAGS = $previousRustFlags
        }
    }
}

function New-SelfExtractingInstaller {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BootstrapExe,

        [Parameter(Mandatory = $true)]
        [string]$PayloadRoot,

        [Parameter(Mandatory = $true)]
        [string]$OutputPath
    )

    $bundleFiles = @(
        'install.cmd',
        'install-local-terminal.ps1',
        'payload.zip'
    )
    $footerMagic = [System.Text.Encoding]::ASCII.GetBytes('WTA-INSTALLER-V1')

    Copy-Item -Path $BootstrapExe -Destination $OutputPath -Force

    $outputStream = [System.IO.File]::Open($OutputPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
    try {
        $outputStream.Seek(0, [System.IO.SeekOrigin]::End) | Out-Null
        $manifestEntries = New-Object System.Collections.Generic.List[string]

        foreach ($fileName in $bundleFiles) {
            $sourcePath = Join-Path $PayloadRoot $fileName
            if (-not (Test-Path $sourcePath -PathType Leaf)) {
                throw "Installer bundle input not found: $sourcePath"
            }

            $offset = [UInt64]$outputStream.Position
            $inputStream = [System.IO.File]::OpenRead($sourcePath)
            try {
                $inputStream.CopyTo($outputStream)
            }
            finally {
                $inputStream.Dispose()
            }

            $length = [UInt64]($outputStream.Position - [Int64]$offset)
            $manifestEntries.Add(("file|{0}|{1}|{2}" -f $fileName, $offset, $length))
        }

        $manifestText = ($manifestEntries -join "`n") + "`n"
        $manifestBytes = [System.Text.Encoding]::UTF8.GetBytes($manifestText)
        $outputStream.Write($manifestBytes, 0, $manifestBytes.Length)

        $manifestLengthBytes = [BitConverter]::GetBytes([UInt64]$manifestBytes.Length)
        $outputStream.Write($footerMagic, 0, $footerMagic.Length)
        $outputStream.Write($manifestLengthBytes, 0, $manifestLengthBytes.Length)
        $outputStream.Flush()
    }
    finally {
        $outputStream.Dispose()
    }
}

function Find-TerminalMsix {
    param(
        [Parameter(Mandatory = $true)]
        [string]$AppPackagesRoot,

        [Parameter(Mandatory = $true)]
        [string]$PlatformName,

        [Parameter(Mandatory = $true)]
        [string]$ConfigurationName
    )

    $patterns = @()
    if ($ConfigurationName -eq 'Release') {
        $patterns += "CascadiaPackage_.*_{0}\.(msix|appx)$" -f $PlatformName
    }
    $patterns += "CascadiaPackage_.*_{0}_{1}\.(msix|appx)$" -f $PlatformName, $ConfigurationName

    $candidate = Get-ChildItem -Path $AppPackagesRoot -Recurse -File |
        Where-Object {
            if ($_.FullName -match '\\Dependencies\\') {
                return $false
            }

            foreach ($pattern in $patterns) {
                if ($_.Name -match $pattern) {
                    return $true
                }
            }

            return $false
        } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if (-not $candidate) {
        throw "Could not find a Cascadia package for $PlatformName/$ConfigurationName under $AppPackagesRoot."
    }

    return $candidate.FullName
}

function Find-XamlAppx {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TerminalPackagePath,

        [Parameter(Mandatory = $true)]
        [string]$PlatformName
    )

    $dependencyArch = Get-XamlDependencyArch -PlatformName $PlatformName
    $dependencyRoot = Join-Path (Split-Path $TerminalPackagePath -Parent) ("Dependencies\{0}" -f $dependencyArch)

    if (-not (Test-Path $dependencyRoot -PathType Container)) {
        throw "Could not find the dependency folder $dependencyRoot."
    }

    $candidate = Get-ChildItem -Path $dependencyRoot -File -Filter 'Microsoft.UI.Xaml*.appx' |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if (-not $candidate) {
        throw "Could not find a Microsoft.UI.Xaml dependency package under $dependencyRoot."
    }

    return $candidate.FullName
}

function Get-SingleChildDirectoryOrSelf {
    param([string]$RootPath)

    $children = @(Get-ChildItem -Path $RootPath -Force)
    if ($children.Count -eq 1 -and $children[0].PSIsContainer) {
        return $children[0].FullName
    }

    return $RootPath
}

function Build-TerminalPackage {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepoRoot,

        [Parameter(Mandatory = $true)]
        [string]$PlatformName,

        [Parameter(Mandatory = $true)]
        [string]$ConfigurationName
    )

    $openConsoleModule = Join-Path $RepoRoot 'tools\OpenConsole.psm1'
    Write-Status "Building CascadiaPackage for $PlatformName/$ConfigurationName ..."

    Import-Module $openConsoleModule -Force
    Set-MsbuildDevEnvironment
    Invoke-OpenConsoleBuild /t:CascadiaPackage "/p:Platform=$PlatformName" "/p:Configuration=$ConfigurationName" /m /nologo
}

$repoRoot = Resolve-AbsolutePath -Path (Join-Path $PSScriptRoot '..\..')
$destinationRoot = Resolve-AbsolutePath -Path $Destination
$appPackagesRoot = Join-Path $repoRoot 'src\cascadia\CascadiaPackage\AppPackages'
$unpackagedScript = Join-Path $repoRoot 'build\scripts\New-UnpackagedTerminalDistribution.ps1'
$installerScript = Join-Path $repoRoot 'installer\install-local-terminal.ps1'
$installerCmd = Join-Path $repoRoot 'installer\install.cmd'
$installerBootstrapManifest = Join-Path $repoRoot 'installer\bootstrap\Cargo.toml'

if (-not (Test-Path $unpackagedScript -PathType Leaf)) {
    throw "Could not find $unpackagedScript."
}
if (-not (Test-Path $installerScript -PathType Leaf)) {
    throw "Could not find $installerScript."
}
if (-not (Test-Path $installerCmd -PathType Leaf)) {
    throw "Could not find $installerCmd."
}
if (-not (Test-Path $installerBootstrapManifest -PathType Leaf)) {
    throw "Could not find $installerBootstrapManifest."
}

Ensure-Directory -Path $destinationRoot

if ($BuildTerminal) {
    Build-TerminalPackage -RepoRoot $repoRoot -PlatformName $Platform -ConfigurationName $Configuration
}

if ($TerminalMsix) {
    $TerminalMsix = Resolve-AbsolutePath -Path $TerminalMsix
} else {
    $TerminalMsix = Find-TerminalMsix -AppPackagesRoot $appPackagesRoot -PlatformName $Platform -ConfigurationName $Configuration
}

if ($XamlAppx) {
    $XamlAppx = Resolve-AbsolutePath -Path $XamlAppx
} else {
    $XamlAppx = Find-XamlAppx -TerminalPackagePath $TerminalMsix -PlatformName $Platform
}

if (-not (Test-Path $TerminalMsix -PathType Leaf)) {
    throw "Terminal package not found: $TerminalMsix"
}
if (-not (Test-Path $XamlAppx -PathType Leaf)) {
    throw "XAML package not found: $XamlAppx"
}

$cargoPath = Find-CargoPath
$rustTarget = Get-RustTarget -PlatformName $Platform
$installedTargets = Get-InstalledRustTargets

if ($installedTargets.Count -gt 0 -and $installedTargets -notcontains $rustTarget) {
    throw "Rust target $rustTarget is not installed. Install it with rustup target add $rustTarget."
}

$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$stageRoot = Join-Path $destinationRoot ("stage-{0}-{1}-{2}" -f $Platform.ToLowerInvariant(), $Configuration.ToLowerInvariant(), $timestamp)
$terminalZipRoot = Join-Path $stageRoot 'terminal-zip'
$payloadExtractRoot = Join-Path $stageRoot 'payload-extracted'
$installerSourceRoot = Join-Path $stageRoot 'installer-source'
$payloadZip = Join-Path $stageRoot 'payload.zip'
$setupExeName = "agentic-terminal-{0}-{1}-setup.exe" -f $Platform.ToLowerInvariant(), $Configuration.ToLowerInvariant()
$setupExePath = Join-Path $destinationRoot $setupExeName

Ensure-Directory -Path $stageRoot
Ensure-Directory -Path $terminalZipRoot
Ensure-Directory -Path $payloadExtractRoot
Ensure-Directory -Path $installerSourceRoot

Write-Status "Creating unpackaged Terminal distribution from:"
Write-Status "  Terminal package: $TerminalMsix"
Write-Status "  XAML dependency:  $XamlAppx"
$unpackagedZip = & $unpackagedScript -TerminalAppX $TerminalMsix -XamlAppX $XamlAppx -Destination $terminalZipRoot -PortableMode

if (-not $unpackagedZip) {
    throw 'New-UnpackagedTerminalDistribution.ps1 did not return an output ZIP.'
}

$unpackagedZipPath = $unpackagedZip.FullName
if (-not (Test-Path $unpackagedZipPath -PathType Leaf)) {
    throw "Unpackaged Terminal ZIP not found: $unpackagedZipPath"
}

Write-Status "Expanding unpackaged Terminal layout ..."
Expand-Archive -Path $unpackagedZipPath -DestinationPath $payloadExtractRoot -Force
$payloadRoot = Get-SingleChildDirectoryOrSelf -RootPath $payloadExtractRoot

if ($SkipWtaBuild) {
    if (-not $WtaExePath) {
        throw 'Use -WtaExePath when -SkipWtaBuild is set.'
    }
    $resolvedWtaExePath = Resolve-AbsolutePath -Path $WtaExePath
} else {
    Write-Status "Building wta.exe for $rustTarget with a static CRT ..."
    $manifestPath = Join-Path $repoRoot 'wta\Cargo.toml'
    Invoke-RustBuild -CargoPath $cargoPath -ManifestPath $manifestPath -RustTarget $rustTarget
    $resolvedWtaExePath = Join-Path $repoRoot ("wta\target\{0}\release\wta.exe" -f $rustTarget)
}

if (-not (Test-Path $resolvedWtaExePath -PathType Leaf)) {
    throw "wta.exe not found: $resolvedWtaExePath"
}

Write-Status "Injecting wta.exe into the unpackaged payload ..."
Copy-Item -Path $resolvedWtaExePath -Destination (Join-Path $payloadRoot 'wta.exe') -Force

if (Test-Path $payloadZip -PathType Leaf) {
    Remove-Item $payloadZip -Force
}

Write-Status "Packing installer payload ..."
& tar.exe -c --format=zip -f $payloadZip -C (Split-Path $payloadRoot -Parent) (Split-Path $payloadRoot -Leaf)
if ($LASTEXITCODE -ne 0) {
    throw 'Creating payload.zip failed.'
}

Copy-Item -Path $installerScript -Destination (Join-Path $installerSourceRoot 'install-local-terminal.ps1') -Force
Copy-Item -Path $installerCmd -Destination (Join-Path $installerSourceRoot 'install.cmd') -Force
Copy-Item -Path $payloadZip -Destination (Join-Path $installerSourceRoot 'payload.zip') -Force

Write-Status "Building installer bootstrap for $rustTarget ..."
Invoke-RustBuild -CargoPath $cargoPath -ManifestPath $installerBootstrapManifest -RustTarget $rustTarget
$bootstrapExePath = Join-Path $repoRoot ("installer\bootstrap\target\{0}\release\agentic-terminal-installer-bootstrap.exe" -f $rustTarget)
if (-not (Test-Path $bootstrapExePath -PathType Leaf)) {
    throw "Installer bootstrap not found: $bootstrapExePath"
}

if (Test-Path $setupExePath -PathType Leaf) {
    Remove-Item $setupExePath -Force
}

Write-Status "Creating target-architecture setup executable ..."
New-SelfExtractingInstaller -BootstrapExe $bootstrapExePath -PayloadRoot $installerSourceRoot -OutputPath $setupExePath

Write-Status "Installer created: $setupExePath"
Get-Item $setupExePath

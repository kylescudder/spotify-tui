[CmdletBinding()]
param(
    [string]$Version = $env:SPOTIFY_TUI_VERSION,
    [string]$InstallDir = $env:SPOTIFY_TUI_INSTALL_DIR,
    [string]$ConfigDir = $env:SPOTIFY_TUI_SPOTIFYD_CONFIG_DIR,
    [string]$Repository = $env:SPOTIFY_TUI_REPOSITORY,
    [string]$ReleaseBaseUrl = $env:SPOTIFY_TUI_RELEASE_BASE_URL,
    [switch]$NoModifyPath,
    [switch]$NoDependencies,
    [switch]$ForceDependencies,
    [switch]$NoService,
    [switch]$AllowInsecureForTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$repositoryPlaceholder = "@" + "REPOSITORY@"

function Start-SpotifydForCurrentSession {
    param(
        [Parameter(Mandatory)][string]$Program,
        [Parameter(Mandatory)][string]$Configuration
    )

    if (-not [string]::IsNullOrWhiteSpace($env:SPOTIFY_TUI_TEST_START_LOG)) {
        [System.IO.File]::WriteAllLines(
            $env:SPOTIFY_TUI_TEST_START_LOG,
            @($Program, $Configuration),
            [System.Text.UTF8Encoding]::new($false)
        )
        return
    }
    if ($null -ne (Get-Process -Name "spotifyd" -ErrorAction SilentlyContinue | Select-Object -First 1)) {
        Write-Host "Spotifyd is already running."
        return
    }

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Program
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $escapedConfiguration = $Configuration.Replace('"', '\"')
    $startInfo.Arguments = "--config-path `"$escapedConfiguration`" --no-daemon"
    [System.Diagnostics.Process]::Start($startInfo) | Out-Null
    Write-Host "Started Spotifyd for the current session."
}

if ($NoDependencies -and $ForceDependencies) {
    throw "-NoDependencies and -ForceDependencies cannot be used together."
}

if ([string]::IsNullOrWhiteSpace($Repository)) {
    $Repository = "@REPOSITORY@"
}
if ($Repository -eq $repositoryPlaceholder) {
    throw "The source installer has no repository. Use -Repository OWNER/REPO or a release-stamped installer."
}
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "Repository must use the OWNER/REPO form."
}

if (-not [string]::IsNullOrWhiteSpace($Version)) {
    $Version = $Version.TrimStart('v')
    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$') {
        throw "Invalid semantic version: $Version"
    }
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\spotify-tui\bin"
}
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
if (-not $NoDependencies -and [string]::IsNullOrWhiteSpace($ConfigDir)) {
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) {
        throw "APPDATA is unset. Pass -ConfigDir or -NoDependencies."
    }
    $ConfigDir = Join-Path $env:APPDATA "spotifyd"
}
if (-not $NoDependencies) {
    $ConfigDir = [System.IO.Path]::GetFullPath($ConfigDir)
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($architecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw "Unsupported Windows architecture: $architecture. The initial Windows release supports x86_64."
}

$artifact = "spotify-tui-x86_64-pc-windows-msvc.zip"
if ([string]::IsNullOrWhiteSpace($ReleaseBaseUrl)) {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $ReleaseBaseUrl = "https://github.com/$Repository/releases/latest/download"
    } else {
        $ReleaseBaseUrl = "https://github.com/$Repository/releases/download/v$Version"
    }
}

$isLocalDirectory = Test-Path -LiteralPath $ReleaseBaseUrl -PathType Container
if (-not $isLocalDirectory -and -not $ReleaseBaseUrl.StartsWith("https://", [System.StringComparison]::OrdinalIgnoreCase)) {
    if (-not $AllowInsecureForTests) {
        throw "Refusing a non-HTTPS release URL."
    }
}

$temporaryDir = Join-Path ([System.IO.Path]::GetTempPath()) ("spotify-tui-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $temporaryDir | Out-Null

function Get-ReleaseFile {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Destination
    )

    if ($isLocalDirectory) {
        Copy-Item -LiteralPath (Join-Path $ReleaseBaseUrl $Name) -Destination $Destination
    } else {
        Invoke-WebRequest -UseBasicParsing -Uri ($ReleaseBaseUrl.TrimEnd('/') + "/" + $Name) -OutFile $Destination
    }
}

try {
    $manifestPath = Join-Path $temporaryDir "SHA256SUMS"
    $archivePath = Join-Path $temporaryDir $artifact
    Get-ReleaseFile -Name "SHA256SUMS" -Destination $manifestPath
    Get-ReleaseFile -Name $artifact -Destination $archivePath

    $expectedChecksum = $null
    foreach ($line in Get-Content -LiteralPath $manifestPath) {
        if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+)$' -and $Matches[2] -eq $artifact) {
            $expectedChecksum = $Matches[1]
            break
        }
    }
    if ([string]::IsNullOrWhiteSpace($expectedChecksum)) {
        throw "$artifact is absent from SHA256SUMS."
    }

    $actualChecksum = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash
    if ($actualChecksum -ne $expectedChecksum) {
        throw "Checksum verification failed for $artifact."
    }

    $unpackedDir = Join-Path $temporaryDir "unpacked"
    [System.IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $unpackedDir)
    foreach ($binary in @("spotify-tui.exe", "spotify-tui-diagnose.exe")) {
        if (-not (Test-Path -LiteralPath (Join-Path $unpackedDir $binary) -PathType Leaf)) {
            throw "Release archive is missing $binary."
        }
    }

    $spotifydPath = $null
    $installBundledSpotifyd = $false
    if (-not $NoDependencies) {
        $installedSpotifyd = Join-Path $InstallDir "spotifyd.exe"
        foreach ($dependencyFile in @("spotifyd.exe", "SPOTIFYD-LICENSE")) {
            if (-not (Test-Path -LiteralPath (Join-Path $unpackedDir $dependencyFile) -PathType Leaf)) {
                throw "Release archive is missing bundled $dependencyFile."
            }
        }
        $spotifydPath = $installedSpotifyd
        $installBundledSpotifyd = $true
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "spotify-tui.exe") -Destination $InstallDir
    Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "spotify-tui-diagnose.exe") -Destination $InstallDir

    if ($installBundledSpotifyd) {
        Get-Process -Name "spotifyd" -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction SilentlyContinue
        Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "spotifyd.exe") -Destination $spotifydPath
        $shareDir = Join-Path (Split-Path -Parent $InstallDir) "share\spotify-tui"
        New-Item -ItemType Directory -Force -Path $shareDir | Out-Null
        Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "SPOTIFYD-LICENSE") -Destination $shareDir
        Write-Host "Installed the bundled Spotifyd runtime in $InstallDir"
    } elseif (-not $NoDependencies) {
        Write-Host "Preserved existing Spotifyd at $spotifydPath"
    }

    if (-not $NoDependencies) {
        $configPath = Join-Path $ConfigDir "spotifyd.conf"
        if (-not (Test-Path -LiteralPath $configPath)) {
            New-Item -ItemType Directory -Force -Path $ConfigDir | Out-Null
            $config = @"
[global]
volume_controller = "softvol"
initial_volume = 90
"@
            [System.IO.File]::WriteAllText(
                $configPath,
                $config + [Environment]::NewLine,
                [System.Text.UTF8Encoding]::new($false)
            )
            Write-Host "Created Spotifyd configuration at $configPath"
        } else {
            Write-Host "Preserved existing Spotifyd configuration at $configPath"
        }
    }

    if (-not $NoDependencies -and -not $NoService) {
        $startupDir = if ([string]::IsNullOrWhiteSpace($env:SPOTIFY_TUI_WINDOWS_STARTUP_DIR)) {
            [Environment]::GetFolderPath([Environment+SpecialFolder]::Startup)
        } else {
            $env:SPOTIFY_TUI_WINDOWS_STARTUP_DIR
        }
        if ([string]::IsNullOrWhiteSpace($startupDir)) {
            Write-Warning "Could not determine the user Startup directory; Spotify TUI will start Spotifyd automatically when it launches."
        } else {
            New-Item -ItemType Directory -Force -Path $startupDir | Out-Null
            $startupPath = Join-Path $startupDir "spotifyd.cmd"
            $escapedSpotifydPath = $spotifydPath.Replace("%", "%%")
            $escapedConfigPath = $configPath.Replace("%", "%%")
            [System.IO.File]::WriteAllText(
                $startupPath,
                "@start `"`" `"$escapedSpotifydPath`" --config-path `"$escapedConfigPath`" --no-daemon" + [Environment]::NewLine,
                [System.Text.UTF8Encoding]::new($false)
            )
            Write-Host "Installed Spotifyd user startup entry at $startupPath"
        }
        Start-SpotifydForCurrentSession -Program $spotifydPath -Configuration $configPath
    }

    if (-not $NoModifyPath) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $pathEntries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if (-not ($pathEntries | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') })) {
            $newPath = (@($pathEntries) + $InstallDir) -join ';'
            [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
            Write-Host "Added $InstallDir to your user PATH. Open a new terminal before running spotify-tui."
        }
    } elseif (-not (($env:PATH -split ';') -contains $InstallDir)) {
        Write-Host "Add $InstallDir to PATH before running spotify-tui."
    }

    Write-Host "Installed spotify-tui and spotify-tui-diagnose in $InstallDir"
    if ($NoDependencies) {
        Write-Warning "Dependency installation was skipped; ensure a compatible spotifyd.exe is on PATH."
    }
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -LiteralPath $temporaryDir
}

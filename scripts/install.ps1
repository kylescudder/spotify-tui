[CmdletBinding()]
param(
    [string]$Version = $env:SPOTIFY_TUI_VERSION,
    [string]$InstallDir = $env:SPOTIFY_TUI_INSTALL_DIR,
    [string]$Repository = $env:SPOTIFY_TUI_REPOSITORY,
    [string]$ReleaseBaseUrl = $env:SPOTIFY_TUI_RELEASE_BASE_URL,
    [switch]$NoModifyPath,
    [switch]$AllowInsecureForTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Repository)) {
    $Repository = "@REPOSITORY@"
}
if ($Repository -eq "@REPOSITORY@") {
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

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "spotify-tui.exe") -Destination $InstallDir
    Copy-Item -Force -LiteralPath (Join-Path $unpackedDir "spotify-tui-diagnose.exe") -Destination $InstallDir

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
    if (-not (Get-Command "spotifyd.exe" -ErrorAction SilentlyContinue)) {
        Write-Warning "spotifyd.exe was not found on PATH; install it before authenticating or playing music."
    }
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -LiteralPath $temporaryDir
}

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("spotify-tui-installer-test-" + [System.Guid]::NewGuid())
$releaseDir = Join-Path $testRoot "release"
$stageDir = Join-Path $testRoot "stage"
$installDir = Join-Path $testRoot "bin"
$configDir = Join-Path $testRoot "config"
$artifact = "spotify-tui-x86_64-pc-windows-msvc.zip"

try {
    New-Item -ItemType Directory -Path $releaseDir, $stageDir | Out-Null
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui.exe") -Value "fixture spotify-tui"
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui-diagnose.exe") -Value "fixture diagnose"
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotifyd.exe") -Value "fixture spotifyd"
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "SPOTIFYD-LICENSE") -Value "fixture Spotifyd licence"
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stageDir, (Join-Path $releaseDir $artifact))

    $checksum = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $releaseDir $artifact)).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $releaseDir "SHA256SUMS") -Value "$checksum  $artifact"

    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -NoService `
        -ConfigDir $configDir `
        -InstallDir $installDir

    foreach ($binary in @("spotify-tui.exe", "spotify-tui-diagnose.exe", "spotifyd.exe")) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDir $binary) -PathType Leaf)) {
            throw "Installer did not install $binary."
        }
    }
    if ((Get-Content -Raw -LiteralPath (Join-Path $installDir "spotifyd.exe")) -ne "fixture spotifyd") {
        throw "Installer installed the wrong Spotifyd binary."
    }
    $licensePath = Join-Path (Split-Path -Parent $installDir) "share\spotify-tui\SPOTIFYD-LICENSE"
    if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) {
        throw "Installer did not install the Spotifyd licence."
    }
    $configPath = Join-Path $configDir "spotifyd.conf"
    $config = Get-Content -Raw -LiteralPath $configPath
    if ($config -notmatch 'volume_controller = "softvol"' -or $config -notmatch 'initial_volume = 90') {
        throw "Installer did not create a compatible Spotifyd configuration."
    }

    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui.exe") -Value "upgraded spotify-tui"
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotifyd.exe") -Value "upgraded spotifyd"
    Remove-Item -LiteralPath (Join-Path $releaseDir $artifact)
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stageDir, (Join-Path $releaseDir $artifact))
    $checksum = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $releaseDir $artifact)).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $releaseDir "SHA256SUMS") -Value "$checksum  $artifact"

    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -Version "1.2.3" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -NoService `
        -ConfigDir $configDir `
        -InstallDir $installDir
    if ((Get-Content -Raw -LiteralPath (Join-Path $installDir "spotify-tui.exe")) -ne "upgraded spotify-tui") {
        throw "Installer did not upgrade the existing binary."
    }
    if ((Get-Content -Raw -LiteralPath (Join-Path $installDir "spotifyd.exe")) -ne "fixture spotifyd") {
        throw "Installer overwrote an existing Spotifyd binary."
    }

    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -ForceDependencies `
        -NoService `
        -ConfigDir $configDir `
        -InstallDir $installDir
    if ((Get-Content -Raw -LiteralPath (Join-Path $installDir "spotifyd.exe")) -ne "upgraded spotifyd") {
        throw "-ForceDependencies did not replace Spotifyd."
    }

    $withoutDependencies = Join-Path $testRoot "no-dependencies"
    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -NoDependencies `
        -NoService `
        -ConfigDir (Join-Path $withoutDependencies "config") `
        -InstallDir (Join-Path $withoutDependencies "bin")
    if (Test-Path -LiteralPath (Join-Path $withoutDependencies "bin\spotifyd.exe")) {
        throw "-NoDependencies installed Spotifyd."
    }
    if (Test-Path -LiteralPath (Join-Path $withoutDependencies "config\spotifyd.conf")) {
        throw "-NoDependencies created a Spotifyd configuration."
    }

    $startupDir = Join-Path $testRoot "startup"
    $startLog = Join-Path $testRoot "spotifyd-started"
    $env:SPOTIFY_TUI_WINDOWS_STARTUP_DIR = $startupDir
    $env:SPOTIFY_TUI_TEST_START_LOG = $startLog
    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -ConfigDir $configDir `
        -InstallDir $installDir
    $startupPath = Join-Path $startupDir "spotifyd.cmd"
    if (-not (Test-Path -LiteralPath $startupPath -PathType Leaf)) {
        throw "Installer did not create the Spotifyd startup entry."
    }
    if ((Get-Content -Raw -LiteralPath $startupPath) -notmatch [regex]::Escape((Join-Path $installDir "spotifyd.exe"))) {
        throw "Spotifyd startup entry does not use the installed dependency."
    }
    if ((Get-Content -Raw -LiteralPath $startupPath) -notmatch [regex]::Escape((Join-Path $configDir "spotifyd.conf"))) {
        throw "Spotifyd startup entry does not use the generated configuration."
    }
    if ((Get-Content -Raw -LiteralPath $startupPath) -notmatch '--no-daemon') {
        throw "Spotifyd startup entry does not keep the daemon attached to its managed process."
    }
    if (-not (Test-Path -LiteralPath $startLog -PathType Leaf)) {
        throw "Installer did not start Spotifyd in the current session."
    }
    $startRecord = Get-Content -Raw -LiteralPath $startLog
    if ($startRecord -notmatch [regex]::Escape((Join-Path $installDir "spotifyd.exe")) -or
        $startRecord -notmatch [regex]::Escape((Join-Path $configDir "spotifyd.conf"))) {
        throw "Installer started Spotifyd without the installed binary and generated configuration."
    }
    Remove-Item Env:SPOTIFY_TUI_WINDOWS_STARTUP_DIR
    Remove-Item Env:SPOTIFY_TUI_TEST_START_LOG

    Set-Content -LiteralPath (Join-Path $releaseDir "SHA256SUMS") -Value "$('0' * 64)  $artifact"
    $rejected = $false
    try {
        & (Join-Path $repositoryRoot "scripts/install.ps1") `
            -Repository "example/spotify-tui" `
            -ReleaseBaseUrl $releaseDir `
            -AllowInsecureForTests `
            -NoModifyPath `
            -NoDependencies `
            -NoService `
            -InstallDir (Join-Path $testRoot "rejected")
    } catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Installer accepted a bad checksum."
    }

    $invalidVersionRejected = $false
    try {
        & (Join-Path $repositoryRoot "scripts/install.ps1") `
            -Repository "example/spotify-tui" `
            -Version "definitely-not-semver" `
            -ReleaseBaseUrl $releaseDir `
            -AllowInsecureForTests `
            -NoModifyPath `
            -NoDependencies `
            -NoService `
            -InstallDir (Join-Path $testRoot "rejected-version")
    } catch {
        $invalidVersionRejected = $true
    }
    if (-not $invalidVersionRejected) {
        throw "Installer accepted an invalid version."
    }

    Write-Host "PowerShell installer tests passed"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -LiteralPath $testRoot
}

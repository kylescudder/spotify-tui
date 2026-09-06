$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("spotify-tui-installer-test-" + [System.Guid]::NewGuid())
$releaseDir = Join-Path $testRoot "release"
$stageDir = Join-Path $testRoot "stage"
$installDir = Join-Path $testRoot "bin"
$artifact = "spotify-tui-x86_64-pc-windows-msvc.zip"

try {
    New-Item -ItemType Directory -Path $releaseDir, $stageDir | Out-Null
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui.exe") -Value "fixture spotify-tui"
    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui-diagnose.exe") -Value "fixture diagnose"
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stageDir, (Join-Path $releaseDir $artifact))

    $checksum = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $releaseDir $artifact)).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $releaseDir "SHA256SUMS") -Value "$checksum  $artifact"

    & (Join-Path $repositoryRoot "scripts/install.ps1") `
        -Repository "example/spotify-tui" `
        -ReleaseBaseUrl $releaseDir `
        -AllowInsecureForTests `
        -NoModifyPath `
        -InstallDir $installDir

    foreach ($binary in @("spotify-tui.exe", "spotify-tui-diagnose.exe")) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDir $binary) -PathType Leaf)) {
            throw "Installer did not install $binary."
        }
    }

    Set-Content -NoNewline -LiteralPath (Join-Path $stageDir "spotify-tui.exe") -Value "upgraded spotify-tui"
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
        -InstallDir $installDir
    if ((Get-Content -Raw -LiteralPath (Join-Path $installDir "spotify-tui.exe")) -ne "upgraded spotify-tui") {
        throw "Installer did not upgrade the existing binary."
    }

    Set-Content -LiteralPath (Join-Path $releaseDir "SHA256SUMS") -Value "$('0' * 64)  $artifact"
    $rejected = $false
    try {
        & (Join-Path $repositoryRoot "scripts/install.ps1") `
            -Repository "example/spotify-tui" `
            -ReleaseBaseUrl $releaseDir `
            -AllowInsecureForTests `
            -NoModifyPath `
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
